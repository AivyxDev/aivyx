//! Federation route handlers — peer discovery, relay, and federated search.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;

use crate::app_state::AppState;

use aivyx_federation::types::{
    FederatedSearchRequest, PeerStatus, PingResponse, RelayChatRequest, RelayTaskRequest,
};

/// GET /federation/ping — respond to health probes from peers.
pub async fn ping(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let federation = match &state.federation {
        Some(f) => f,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "federation not configured"})),
            )
                .into_response();
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

    Json(PingResponse {
        instance_id: federation.instance_id().to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        agents,
    })
    .into_response()
}

/// GET /federation/peers — list all configured peers and their health status.
pub async fn list_peers(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let federation = match &state.federation {
        Some(f) => f,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "federation not configured"})),
            )
                .into_response();
        }
    };

    let peers: Vec<PeerStatus> = federation.list_peers().await;
    Json(serde_json::json!({ "peers": peers })).into_response()
}

/// GET /federation/peers/:id/agents — list agents on a specific peer.
pub async fn peer_agents(
    State(state): State<Arc<AppState>>,
    Path(peer_id): Path<String>,
) -> impl IntoResponse {
    let federation = match &state.federation {
        Some(f) => f,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "federation not configured"})),
            )
                .into_response();
        }
    };

    match federation.peer_agents(&peer_id).await {
        Ok(agents) => Json(serde_json::json!({ "agents": agents })).into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// POST /federation/relay/chat — relay a chat message to a peer's agent.
pub async fn relay_chat(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RelayChatRequest>,
) -> impl IntoResponse {
    let federation = match &state.federation {
        Some(f) => f,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "federation not configured"})),
            )
                .into_response();
        }
    };

    match federation.relay_chat(&req).await {
        Ok(resp) => Json(serde_json::json!(resp)).into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// POST /federation/relay/task — create a task on a peer instance.
pub async fn relay_task(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RelayTaskRequest>,
) -> impl IntoResponse {
    let federation = match &state.federation {
        Some(f) => f,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "federation not configured"})),
            )
                .into_response();
        }
    };

    match federation.relay_task(&req).await {
        Ok(resp) => Json(serde_json::json!(resp)).into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// POST /federation/search — federated memory search across peers.
pub async fn federated_search(
    State(state): State<Arc<AppState>>,
    Json(req): Json<FederatedSearchRequest>,
) -> impl IntoResponse {
    let federation = match &state.federation {
        Some(f) => f,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "federation not configured"})),
            )
                .into_response();
        }
    };

    match federation.federated_search(&req).await {
        Ok(results) => Json(serde_json::json!({ "results": results })).into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}
