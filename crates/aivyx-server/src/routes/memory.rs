//! Memory management endpoints.
//!
//! `GET /memory` — list memories (optional `?kind=` filter).
//! `POST /memory/search` — semantic search.
//! `DELETE /memory/:id` — delete a memory.
//! `GET /memory/stats` — memory subsystem statistics.
//! `GET /memory/triples` — list knowledge triples (optional filters).
//! `GET /memory/profile` — get the user profile.
//! `PUT /memory/profile` — update the user profile (partial merge).
//! `POST /memory/profile/extract` — trigger LLM-driven profile extraction.
//! `GET /memory/graph` — full knowledge graph as nodes + edges.
//! `GET /memory/graph/entity/:name` — subgraph (neighborhood) around entity.
//! `GET /memory/graph/communities` — connected components.
//! `GET /memory/graph/path` — find paths between two entities.

use std::sync::Arc;

use aivyx_core::{AivyxError, MemoryId};
use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use crate::app_state::AppState;
use crate::error::ServerError;
use crate::extractors::AuthContextExt;
use aivyx_tenant::AivyxRole;

/// Query parameters for `GET /memory`.
#[derive(Debug, Deserialize)]
pub struct ListMemoriesQuery {
    /// Filter by memory kind (e.g., "Fact", "Preference").
    pub kind: Option<String>,
}

/// Request body for `POST /memory/search`.
#[derive(Debug, Deserialize)]
pub struct SearchRequest {
    /// Search query text.
    pub query: String,
    /// Maximum number of results (default: 5).
    pub top_k: Option<usize>,
}

/// Response for search results.
#[derive(Debug, Serialize)]
pub struct SearchResponse {
    /// Matching memories.
    pub results: Vec<MemoryResult>,
}

/// A single memory search result.
#[derive(Debug, Serialize)]
pub struct MemoryResult {
    /// Memory ID.
    pub id: String,
    /// Memory content.
    pub content: String,
    /// Memory kind.
    pub kind: String,
    /// Tags.
    pub tags: Vec<String>,
}

/// Query parameters for `GET /memory/triples`.
#[derive(Debug, Deserialize)]
pub struct TriplesQuery {
    /// Filter by subject.
    pub subject: Option<String>,
    /// Filter by predicate.
    pub predicate: Option<String>,
}

/// `GET /memory` — list stored memories.
pub async fn list_memories(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
    Query(query): Query<ListMemoriesQuery>,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Viewer)?;
    let mgr = require_memory(&state)?;
    let mgr = mgr.lock().await;

    // Access the store through the manager — we need master_key for loading
    // Since MemoryManager has no public list method, we use stats + recall
    // Actually, we need to list and load from the store directly
    // MemoryManager doesn't expose list_memories, but MemoryStore does.
    // We need to work through the store via the memory dir path.
    let store_path = state.dirs.memory_dir().join("memory.db");
    let store = aivyx_memory::MemoryStore::open(&store_path)?;
    let ids = store.list_memories()?;

    let mut memories = Vec::new();
    for id in ids {
        if let Some(entry) = store.load_memory(&id, &state.master_key)? {
            let kind_str = format!("{:?}", entry.kind);
            if let Some(ref filter) = query.kind
                && !kind_str.eq_ignore_ascii_case(filter)
            {
                continue;
            }
            memories.push(MemoryResult {
                id: entry.id.to_string(),
                content: entry.content,
                kind: kind_str,
                tags: entry.tags,
            });
        }
    }
    drop(mgr);

    Ok(axum::Json(memories))
}

/// `POST /memory/search` — semantic search over memories.
pub async fn search_memories(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
    axum::Json(req): axum::Json<SearchRequest>,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Viewer)?;
    let mgr = require_memory(&state)?;
    let mut mgr = mgr.lock().await;

    let top_k = req.top_k.unwrap_or(5).min(100);
    let entries = mgr.recall(&req.query, top_k, None, &[]).await?;

    let results: Vec<MemoryResult> = entries
        .into_iter()
        .map(|e| MemoryResult {
            id: e.id.to_string(),
            content: e.content,
            kind: format!("{:?}", e.kind),
            tags: e.tags,
        })
        .collect();

    Ok(axum::Json(SearchResponse { results }))
}

/// `DELETE /memory/:id` — delete a memory.
pub async fn delete_memory(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Operator)?;
    let mgr = require_memory(&state)?;
    let mut mgr = mgr.lock().await;

    let memory_id: MemoryId = id
        .parse()
        .map_err(|_| ServerError(AivyxError::Config(format!("invalid memory ID: {id}"))))?;

    mgr.forget(&memory_id)?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// `GET /memory/stats` — memory subsystem statistics.
pub async fn memory_stats(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Viewer)?;
    let mgr = require_memory(&state)?;
    let mgr = mgr.lock().await;
    let stats = mgr.stats()?;

    Ok(axum::Json(serde_json::json!({
        "total_memories": stats.total_memories,
        "total_triples": stats.total_triples,
        "index_size": stats.index_size,
    })))
}

/// `GET /memory/triples` — list knowledge triples.
pub async fn list_triples(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
    Query(query): Query<TriplesQuery>,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Viewer)?;
    let mgr = require_memory(&state)?;
    let mgr = mgr.lock().await;

    let triples = mgr.query_triples(
        query.subject.as_deref(),
        query.predicate.as_deref(),
        None,
        None,
    )?;

    let results: Vec<serde_json::Value> = triples
        .into_iter()
        .map(|t| {
            serde_json::json!({
                "id": t.id.to_string(),
                "subject": t.subject,
                "predicate": t.predicate,
                "object": t.object,
                "confidence": t.confidence,
                "source": t.source,
            })
        })
        .collect();

    Ok(axum::Json(results))
}

/// `GET /memory/profile` — get the user profile.
pub async fn get_profile(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Viewer)?;
    let mgr = require_memory(&state)?;
    let mgr = mgr.lock().await;
    let profile = mgr.get_profile()?;
    Ok(axum::Json(profile))
}

/// `PUT /memory/profile` — update the user profile (partial merge).
pub async fn update_profile(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
    axum::Json(input): axum::Json<serde_json::Value>,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Operator)?;
    let mgr = require_memory(&state)?;
    let mgr = mgr.lock().await;
    let mut profile = mgr.get_profile()?;

    // Merge scalar fields if present
    if let Some(name) = input["name"].as_str() {
        profile.name = Some(name.to_string());
    }
    if let Some(tz) = input["timezone"].as_str() {
        profile.timezone = Some(tz.to_string());
    }
    // Merge list fields if present (append with dedup)
    if let Some(arr) = input["tech_stack"].as_array() {
        for item in arr.iter().filter_map(|v| v.as_str()) {
            if !profile
                .tech_stack
                .iter()
                .any(|s| s.eq_ignore_ascii_case(item))
            {
                profile.tech_stack.push(item.to_string());
            }
        }
    }
    if let Some(arr) = input["style_preferences"].as_array() {
        for item in arr.iter().filter_map(|v| v.as_str()) {
            if !profile
                .style_preferences
                .iter()
                .any(|s| s.eq_ignore_ascii_case(item))
            {
                profile.style_preferences.push(item.to_string());
            }
        }
    }
    if let Some(arr) = input["schedule_hints"].as_array() {
        for item in arr.iter().filter_map(|v| v.as_str()) {
            if !profile
                .schedule_hints
                .iter()
                .any(|s| s.eq_ignore_ascii_case(item))
            {
                profile.schedule_hints.push(item.to_string());
            }
        }
    }
    if let Some(arr) = input["notes"].as_array() {
        for item in arr.iter().filter_map(|v| v.as_str()) {
            if !profile.notes.contains(&item.to_string()) {
                profile.notes.push(item.to_string());
            }
        }
    }

    mgr.update_profile(profile.clone())?;
    Ok(axum::Json(profile))
}

/// `POST /memory/profile/extract` — trigger LLM-driven profile extraction.
///
/// Gathers Fact and Preference memories, sends them to the LLM for profile
/// extraction, merges the result into the current profile, and returns it.
pub async fn extract_profile(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Operator)?;
    let mgr = require_memory(&state)?;

    // Create a temporary LLM provider for the extraction request
    let enc_store = aivyx_crypto::EncryptedStore::open(state.dirs.store_path())?;
    let config = state.config.read().await;
    let provider = aivyx_llm::create_provider(&config.provider, &enc_store, &state.master_key)?;
    drop(config);

    let mgr = mgr.lock().await;
    let profile = mgr.extract_profile(provider.as_ref()).await?;
    Ok(axum::Json(profile))
}

// -----------------------------------------------------------------------
// Knowledge graph visualization endpoints
// -----------------------------------------------------------------------

/// A node in the graph response.
#[derive(Debug, Serialize)]
pub struct GraphNode {
    /// Entity name.
    pub name: String,
}

/// An edge in the graph response.
#[derive(Debug, Serialize)]
pub struct GraphResponseEdge {
    /// Source entity.
    pub source: String,
    /// Relationship.
    pub predicate: String,
    /// Target entity.
    pub target: String,
    /// Confidence score.
    pub confidence: f32,
}

/// Response for `GET /memory/graph`.
#[derive(Debug, Serialize)]
pub struct GraphResponse {
    /// All entities in the graph.
    pub nodes: Vec<GraphNode>,
    /// All edges in the graph.
    pub edges: Vec<GraphResponseEdge>,
}

/// Query parameters for `GET /memory/graph/path`.
#[derive(Debug, Deserialize)]
pub struct PathQuery {
    /// Source entity.
    pub from: String,
    /// Target entity.
    pub to: String,
    /// Maximum path depth (default: 5).
    pub max_depth: Option<usize>,
}

/// `GET /memory/graph` — returns the full knowledge graph as nodes + edges.
pub async fn graph_full(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Viewer)?;
    let mgr = require_memory(&state)?;
    let mgr = mgr.lock().await;

    // Verify the graph is available (even though we use query_triples for data)
    let _graph = mgr.graph().ok_or_else(|| {
        ServerError(AivyxError::NotInitialized(
            "knowledge graph not available".into(),
        ))
    })?;

    let mut nodes = Vec::new();
    let mut edges = Vec::new();

    // Get all triples to build edges
    let triples = mgr.query_triples(None, None, None, None)?;
    let mut entity_set = std::collections::HashSet::new();

    for t in &triples {
        entity_set.insert(t.subject.clone());
        entity_set.insert(t.object.clone());
        edges.push(GraphResponseEdge {
            source: t.subject.clone(),
            predicate: t.predicate.clone(),
            target: t.object.clone(),
            confidence: t.confidence,
        });
    }

    for name in entity_set {
        nodes.push(GraphNode { name });
    }

    Ok(axum::Json(GraphResponse { nodes, edges }))
}

/// `GET /memory/graph/entity/:name` — returns the neighborhood around an entity.
pub async fn graph_entity(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Viewer)?;
    let mgr = require_memory(&state)?;
    let mgr = mgr.lock().await;

    let graph = mgr.graph().ok_or_else(|| {
        ServerError(AivyxError::NotInitialized(
            "knowledge graph not available".into(),
        ))
    })?;

    let nb = graph.neighborhood(&name);

    let mut nodes = std::collections::HashSet::new();
    let mut edges = Vec::new();

    nodes.insert(name.clone());

    for edge in &nb.outbound {
        nodes.insert(edge.target.clone());
        edges.push(GraphResponseEdge {
            source: name.clone(),
            predicate: edge.predicate.clone(),
            target: edge.target.clone(),
            confidence: edge.confidence,
        });
    }

    for edge in &nb.inbound {
        nodes.insert(edge.target.clone());
        edges.push(GraphResponseEdge {
            source: edge.target.clone(),
            predicate: edge.predicate.clone(),
            target: name.clone(),
            confidence: edge.confidence,
        });
    }

    let nodes: Vec<GraphNode> = nodes.into_iter().map(|n| GraphNode { name: n }).collect();

    Ok(axum::Json(GraphResponse { nodes, edges }))
}

/// `GET /memory/graph/communities` — returns connected components.
pub async fn graph_communities(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Viewer)?;
    let mgr = require_memory(&state)?;
    let mgr = mgr.lock().await;

    let graph = mgr.graph().ok_or_else(|| {
        ServerError(AivyxError::NotInitialized(
            "knowledge graph not available".into(),
        ))
    })?;

    let communities = graph.detect_communities();

    let results: Vec<serde_json::Value> = communities
        .into_iter()
        .map(|c| {
            let entities: Vec<&str> = c.entities.iter().map(|s| s.as_str()).collect();
            serde_json::json!({
                "entities": entities,
                "edge_count": c.edge_count,
            })
        })
        .collect();

    Ok(axum::Json(results))
}

/// `GET /memory/graph/path` — find paths between two entities.
pub async fn graph_path(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
    Query(query): Query<PathQuery>,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Viewer)?;
    let mgr = require_memory(&state)?;
    let mgr = mgr.lock().await;

    let graph = mgr.graph().ok_or_else(|| {
        ServerError(AivyxError::NotInitialized(
            "knowledge graph not available".into(),
        ))
    })?;

    let max_depth = query.max_depth.unwrap_or(5).min(10);
    let paths = graph.find_paths(&query.from, &query.to, max_depth);

    let results: Vec<Vec<serde_json::Value>> = paths
        .into_iter()
        .map(|p| {
            p.hops
                .into_iter()
                .map(|(s, pred, o)| {
                    serde_json::json!({
                        "subject": s,
                        "predicate": pred,
                        "object": o,
                    })
                })
                .collect()
        })
        .collect();

    Ok(axum::Json(results))
}

/// Returns the memory manager or a 503 error if memory is not configured.
fn require_memory(
    state: &AppState,
) -> Result<&Arc<tokio::sync::Mutex<aivyx_memory::MemoryManager>>, ServerError> {
    state.memory_manager.as_ref().ok_or_else(|| {
        ServerError(AivyxError::NotInitialized(
            "memory system not configured (no embedding provider)".into(),
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_request_deserializes() {
        let json = r#"{"query":"rust programming"}"#;
        let req: SearchRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.query, "rust programming");
        assert_eq!(req.top_k, None);
    }

    #[test]
    fn search_request_with_top_k() {
        let json = r#"{"query":"test","top_k":10}"#;
        let req: SearchRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.top_k, Some(10));
    }

    #[test]
    fn memory_result_serializes() {
        let r = MemoryResult {
            id: "abc".into(),
            content: "test content".into(),
            kind: "Fact".into(),
            tags: vec!["tag1".into()],
        };
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["kind"], "Fact");
        assert_eq!(json["tags"][0], "tag1");
    }

    #[test]
    fn triples_query_deserializes() {
        let json = r#"{"subject":"Rust","predicate":"is_a"}"#;
        let q: TriplesQuery = serde_json::from_str(json).unwrap();
        assert_eq!(q.subject.as_deref(), Some("Rust"));
    }

    #[test]
    fn list_memories_query_optional() {
        let json = r#"{}"#;
        let q: ListMemoriesQuery = serde_json::from_str(json).unwrap();
        assert!(q.kind.is_none());
    }

    #[test]
    fn profile_update_json_deserializes() {
        let json = r#"{"name": "Julian", "tech_stack": ["Rust"]}"#;
        let val: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(val["name"].as_str(), Some("Julian"));
        assert!(val["tech_stack"].as_array().is_some());
    }

    #[test]
    fn graph_response_serializes() {
        let resp = GraphResponse {
            nodes: vec![
                GraphNode {
                    name: "Rust".into(),
                },
                GraphNode {
                    name: "language".into(),
                },
            ],
            edges: vec![GraphResponseEdge {
                source: "Rust".into(),
                predicate: "is_a".into(),
                target: "language".into(),
                confidence: 0.95,
            }],
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["nodes"].as_array().unwrap().len(), 2);
        assert_eq!(json["edges"].as_array().unwrap().len(), 1);
        assert_eq!(json["edges"][0]["source"], "Rust");
        assert_eq!(json["edges"][0]["predicate"], "is_a");
        assert_eq!(json["edges"][0]["target"], "language");
    }

    #[test]
    fn path_query_deserializes() {
        let json = r#"{"from":"A","to":"B"}"#;
        let q: PathQuery = serde_json::from_str(json).unwrap();
        assert_eq!(q.from, "A");
        assert_eq!(q.to, "B");
        assert_eq!(q.max_depth, None);
    }

    #[test]
    fn path_query_with_max_depth() {
        let json = r#"{"from":"A","to":"B","max_depth":3}"#;
        let q: PathQuery = serde_json::from_str(json).unwrap();
        assert_eq!(q.max_depth, Some(3));
    }
}
