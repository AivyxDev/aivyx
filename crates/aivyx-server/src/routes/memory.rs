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

use std::sync::Arc;

use aivyx_core::{AivyxError, MemoryId};
use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use crate::app_state::AppState;
use crate::error::ServerError;

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
    Query(query): Query<ListMemoriesQuery>,
) -> Result<impl IntoResponse, ServerError> {
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
    axum::Json(req): axum::Json<SearchRequest>,
) -> Result<impl IntoResponse, ServerError> {
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
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ServerError> {
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
) -> Result<impl IntoResponse, ServerError> {
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
    Query(query): Query<TriplesQuery>,
) -> Result<impl IntoResponse, ServerError> {
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
) -> Result<impl IntoResponse, ServerError> {
    let mgr = require_memory(&state)?;
    let mgr = mgr.lock().await;
    let profile = mgr.get_profile()?;
    Ok(axum::Json(profile))
}

/// `PUT /memory/profile` — update the user profile (partial merge).
pub async fn update_profile(
    State(state): State<Arc<AppState>>,
    axum::Json(input): axum::Json<serde_json::Value>,
) -> Result<impl IntoResponse, ServerError> {
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
) -> Result<impl IntoResponse, ServerError> {
    let mgr = require_memory(&state)?;

    // Create a temporary LLM provider for the extraction request
    let enc_store = aivyx_crypto::EncryptedStore::open(state.dirs.store_path())?;
    let provider =
        aivyx_llm::create_provider(&state.config.provider, &enc_store, &state.master_key)?;

    let mgr = mgr.lock().await;
    let profile = mgr.extract_profile(provider.as_ref()).await?;
    Ok(axum::Json(profile))
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
}
