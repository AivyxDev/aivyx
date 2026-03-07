//! System status endpoint.
//!
//! `GET /status` — system summary including provider, agent/team/session counts,
//! and memory stats.

use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;
use serde::Serialize;

use crate::app_state::AppState;
use crate::error::ServerError;
use crate::extractors::AuthContextExt;
use aivyx_tenant::AivyxRole;

/// Response body for `GET /status`.
#[derive(Debug, Serialize)]
pub struct StatusResponse {
    /// Provider type (e.g., "claude", "openai", "ollama").
    pub provider: String,
    /// Default autonomy tier.
    pub autonomy_tier: String,
    /// Number of agent profiles.
    pub agent_count: usize,
    /// Number of team configs.
    pub team_count: usize,
    /// Number of saved sessions.
    pub session_count: usize,
    /// Audit log entry count.
    pub audit_entries: usize,
    /// Memory statistics (if available).
    pub memory: Option<MemoryStatusInfo>,
    /// Federation subsystem status (if enabled).
    pub federation: Option<FederationStatusInfo>,
}

/// Memory subsystem status info.
#[derive(Debug, Serialize)]
pub struct MemoryStatusInfo {
    /// Total memories stored.
    pub total_memories: usize,
    /// Total knowledge triples.
    pub total_triples: usize,
    /// Vector index size.
    pub index_size: usize,
}

/// Federation subsystem status info.
#[derive(Debug, Serialize)]
pub struct FederationStatusInfo {
    /// This instance's federation ID.
    pub instance_id: String,
    /// This instance's Ed25519 public key (base64).
    pub public_key: String,
    /// Total number of configured peers.
    pub peer_count: usize,
    /// Number of healthy (reachable) peers.
    pub healthy_peers: usize,
    /// Per-peer health details.
    pub peers: Vec<PeerHealthInfo>,
}

/// Health info for a single federation peer.
#[derive(Debug, Serialize)]
pub struct PeerHealthInfo {
    pub id: String,
    pub url: String,
    pub healthy: bool,
    pub last_seen: Option<String>,
    pub capabilities: Vec<String>,
}

/// `GET /status` — system summary.
pub async fn system_status(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Viewer)?;
    let config = state.config.read().await;
    let provider = format!("{:?}", config.provider);
    let tier = format!("{:?}", config.autonomy.default_tier);
    drop(config);

    let agent_count = count_toml_files(&state.dirs.agents_dir());
    let team_count = count_toml_files(&state.dirs.teams_dir());

    let session_count = state
        .session_store
        .list(&state.master_key)
        .map(|s| s.len())
        .unwrap_or(0);

    let audit_entries = state.audit_log.len().unwrap_or(0);

    let memory = if let Some(ref mgr) = state.memory_manager {
        let mgr = mgr.lock().await;
        mgr.stats().ok().map(|s| MemoryStatusInfo {
            total_memories: s.total_memories,
            total_triples: s.total_triples,
            index_size: s.index_size,
        })
    } else {
        None
    };

    let federation = if let Some(ref fed) = state.federation {
        let peers = fed.list_peers().await;
        let healthy_count = peers.iter().filter(|p| p.healthy).count();
        Some(FederationStatusInfo {
            instance_id: fed.instance_id().to_string(),
            public_key: fed.public_key(),
            peer_count: peers.len(),
            healthy_peers: healthy_count,
            peers: peers
                .into_iter()
                .map(|p| PeerHealthInfo {
                    id: p.id,
                    url: p.url,
                    healthy: p.healthy,
                    last_seen: p.last_seen,
                    capabilities: p.capabilities,
                })
                .collect(),
        })
    } else {
        None
    };

    Ok(axum::Json(StatusResponse {
        provider,
        autonomy_tier: tier,
        agent_count,
        team_count,
        session_count,
        audit_entries,
        memory,
        federation,
    }))
}

/// Count `.toml` files in a directory.
fn count_toml_files(dir: &std::path::Path) -> usize {
    if !dir.exists() {
        return 0;
    }
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|ext| ext == "toml"))
                .count()
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_response_serializes() {
        let resp = StatusResponse {
            provider: "ollama".into(),
            autonomy_tier: "Trust".into(),
            agent_count: 3,
            team_count: 1,
            session_count: 5,
            audit_entries: 42,
            memory: Some(MemoryStatusInfo {
                total_memories: 10,
                total_triples: 5,
                index_size: 10,
            }),
            federation: None,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["agent_count"], 3);
        assert_eq!(json["memory"]["total_memories"], 10);
    }

    #[test]
    fn status_without_memory() {
        let resp = StatusResponse {
            provider: "claude".into(),
            autonomy_tier: "Free".into(),
            agent_count: 0,
            team_count: 0,
            session_count: 0,
            audit_entries: 0,
            memory: None,
            federation: None,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json["memory"].is_null());
    }

    #[test]
    fn status_with_federation() {
        let resp = StatusResponse {
            provider: "claude".into(),
            autonomy_tier: "Trust".into(),
            agent_count: 2,
            team_count: 0,
            session_count: 0,
            audit_entries: 0,
            memory: None,
            federation: Some(FederationStatusInfo {
                instance_id: "vps5-ops".into(),
                public_key: "AAAA".into(),
                peer_count: 1,
                healthy_peers: 1,
                peers: vec![PeerHealthInfo {
                    id: "vps1-studio".into(),
                    url: "https://api.studio.io".into(),
                    healthy: true,
                    last_seen: Some("2026-03-07T00:00:00Z".into()),
                    capabilities: vec!["chat".into(), "memory".into()],
                }],
            }),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["federation"]["instance_id"], "vps5-ops");
        assert_eq!(json["federation"]["healthy_peers"], 1);
        assert_eq!(json["federation"]["peers"][0]["id"], "vps1-studio");
    }

    #[test]
    fn count_toml_files_empty_dir() {
        let dir = std::env::temp_dir().join(format!("aivyx-count-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(count_toml_files(&dir), 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn count_toml_files_nonexistent() {
        let dir = std::path::Path::new("/nonexistent/path");
        assert_eq!(count_toml_files(dir), 0);
    }
}
