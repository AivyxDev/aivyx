//! Federation route handlers — peer discovery, relay, and federated search.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;

use crate::app_state::AppState;
use crate::error::ServerError;
use crate::extractors::AuthContextExt;
use aivyx_tenant::AivyxRole;

use aivyx_federation::types::{
    FederatedSearchRequest, PeerStatus, PingResponse, RelayChatRequest, RelayTaskRequest,
};

/// GET /federation/ping — respond to health probes from peers.
pub async fn ping(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Viewer)?;
    let federation = match &state.federation {
        Some(f) => f,
        None => {
            return Ok((
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "federation not configured"})),
            )
                .into_response());
        }
    };

    // Collect agent names from the agents directory
    let agents_dir = state.dirs.agents_dir();
    let agents: Vec<String> = if agents_dir.exists() {
        std::fs::read_dir(&agents_dir)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "toml"))
            .filter_map(|e| {
                e.path()
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
            })
            .collect()
    } else {
        Vec::new()
    };

    Ok(Json(PingResponse {
        instance_id: federation.instance_id().to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        agents,
    })
    .into_response())
}

/// GET /federation/peers — list all configured peers and their health status.
pub async fn list_peers(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Viewer)?;
    let federation = match &state.federation {
        Some(f) => f,
        None => {
            return Ok((
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "federation not configured"})),
            )
                .into_response());
        }
    };

    let peers: Vec<PeerStatus> = federation.list_peers().await;
    Ok(Json(serde_json::json!({ "peers": peers })).into_response())
}

/// GET /federation/peers/:id/agents — list agents on a specific peer.
pub async fn peer_agents(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
    Path(peer_id): Path<String>,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Viewer)?;
    let federation = match &state.federation {
        Some(f) => f,
        None => {
            return Ok((
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "federation not configured"})),
            )
                .into_response());
        }
    };

    Ok(match federation.peer_agents(&peer_id).await {
        Ok(agents) => Json(serde_json::json!({ "agents": agents })).into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    })
}

/// POST /federation/relay/chat — relay a chat message to a peer's agent.
///
/// Enforces trust policy: the target peer must have a configured trust policy.
/// Without a trust policy, relay requests are denied (principle of least privilege).
pub async fn relay_chat(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
    Json(req): Json<RelayChatRequest>,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Operator)?;
    let federation = match &state.federation {
        Some(f) => f,
        None => {
            return Ok((
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "federation not configured"})),
            )
                .into_response());
        }
    };

    // Enforce trust policy — peer must have an explicit policy to allow relay
    if federation.peer_trust_policy(&req.peer_id).await.is_none() {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": format!("no trust policy configured for peer '{}'", req.peer_id)
            })),
        )
            .into_response());
    }

    Ok(match federation.relay_chat(&req).await {
        Ok(resp) => Json(serde_json::json!(resp)).into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    })
}

/// POST /federation/relay/task — create a task on a peer instance.
///
/// Enforces trust policy: the target peer must have a configured trust policy.
pub async fn relay_task(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
    Json(req): Json<RelayTaskRequest>,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Operator)?;
    let federation = match &state.federation {
        Some(f) => f,
        None => {
            return Ok((
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "federation not configured"})),
            )
                .into_response());
        }
    };

    // Enforce trust policy — peer must have an explicit policy to allow relay
    if federation.peer_trust_policy(&req.peer_id).await.is_none() {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": format!("no trust policy configured for peer '{}'", req.peer_id)
            })),
        )
            .into_response());
    }

    Ok(match federation.relay_task(&req).await {
        Ok(resp) => Json(serde_json::json!(resp)).into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    })
}

/// POST /federation/search — federated memory search across peers.
pub async fn federated_search(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
    Json(req): Json<FederatedSearchRequest>,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Viewer)?;
    let federation = match &state.federation {
        Some(f) => f,
        None => {
            return Ok((
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "federation not configured"})),
            )
                .into_response());
        }
    };

    Ok(match federation.federated_search(&req).await {
        Ok(results) => Json(serde_json::json!({ "results": results })).into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    })
}
