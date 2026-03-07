//! Cross-agent memory sharing tool.
//!
//! Provides [`TeamMemoryQueryTool`], a tool that lets specialists query
//! memories visible to any agent in the team set, enabling cross-pollination
//! of knowledge between specialists.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use aivyx_core::{AgentId, CapabilityScope, Result, Tool, ToolId};
use aivyx_memory::MemoryManager;

/// Tool that lets specialists query shared team memories.
///
/// Queries memories visible to any agent in the team set, enabling
/// cross-pollination of knowledge between specialists.
pub struct TeamMemoryQueryTool {
    id: ToolId,
    memory_manager: Arc<Mutex<MemoryManager>>,
    /// Agent IDs of all team members (for visibility filtering).
    team_agent_ids: Vec<AgentId>,
}

impl TeamMemoryQueryTool {
    /// Create a new team memory query tool.
    pub fn new(
        memory_manager: Arc<Mutex<MemoryManager>>,
        team_agent_ids: Vec<AgentId>,
    ) -> Self {
        Self {
            id: ToolId::new(),
            memory_manager,
            team_agent_ids,
        }
    }
}

#[async_trait]
impl Tool for TeamMemoryQueryTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "team_memory_query"
    }

    fn description(&self) -> &str {
        "Query memories shared across team members. Returns relevant memories visible to any team member."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query"
                },
                "top_k": {
                    "type": "integer",
                    "description": "Maximum results to return (default: 5)",
                    "default": 5
                }
            },
            "required": ["query"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("memory".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let query = input["query"].as_str().unwrap_or("");
        let top_k = input["top_k"].as_u64().unwrap_or(5) as usize;

        let mut mgr = self.memory_manager.lock().await;

        // Query for each team agent and collect results
        let mut all_results = Vec::new();
        for agent_id in &self.team_agent_ids {
            let results = mgr.recall(query, top_k, Some(*agent_id), &[]).await?;
            all_results.extend(results);
        }
        // Also include global memories
        let global = mgr.recall(query, top_k, None, &[]).await?;
        all_results.extend(global);

        // Deduplicate by id and take top_k
        all_results.sort_by(|a, b| b.access_count.cmp(&a.access_count));
        all_results.dedup_by(|a, b| a.id == b.id);
        all_results.truncate(top_k);

        let results_json: Vec<serde_json::Value> = all_results
            .iter()
            .map(|m| {
                serde_json::json!({
                    "content": m.content,
                    "kind": format!("{:?}", m.kind),
                    "tags": m.tags,
                    "access_count": m.access_count,
                })
            })
            .collect();

        Ok(serde_json::json!({
            "results": results_json,
            "count": results_json.len(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use aivyx_memory::MemoryStore;

    struct MockEmbeddingProvider {
        dims: usize,
        call_count: AtomicUsize,
    }

    impl MockEmbeddingProvider {
        fn new(dims: usize) -> Self {
            Self {
                dims,
                call_count: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl aivyx_llm::EmbeddingProvider for MockEmbeddingProvider {
        fn name(&self) -> &str {
            "mock"
        }

        fn dimensions(&self) -> usize {
            self.dims
        }

        async fn embed(&self, text: &str) -> aivyx_core::Result<aivyx_llm::Embedding> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            let mut vector = vec![0.0_f32; self.dims];
            for (i, byte) in text.bytes().enumerate() {
                vector[i % self.dims] += byte as f32 / 255.0;
            }
            let norm: f32 = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                for v in &mut vector {
                    *v /= norm;
                }
            }
            Ok(aivyx_llm::Embedding {
                vector,
                dimensions: self.dims,
            })
        }
    }

    fn setup_tool() -> (TeamMemoryQueryTool, std::path::PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("aivyx-team-mem-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("memory.db");
        let store = MemoryStore::open(&db_path).unwrap();
        let key = aivyx_crypto::MasterKey::generate();
        let provider = Arc::new(MockEmbeddingProvider::new(4));
        let mgr = MemoryManager::new(store, provider, key, 0).unwrap();
        let agent_ids = vec![AgentId::new(), AgentId::new()];
        let tool = TeamMemoryQueryTool::new(Arc::new(Mutex::new(mgr)), agent_ids);
        (tool, dir)
    }

    #[test]
    fn tool_has_correct_name_and_schema() {
        let (tool, dir) = setup_tool();
        assert_eq!(tool.name(), "team_memory_query");

        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        let props = &schema["properties"];
        assert!(props.get("query").is_some());
        assert!(props.get("top_k").is_some());
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("query")));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tool_requires_memory_scope() {
        let (tool, dir) = setup_tool();
        let scope = tool.required_scope();
        assert_eq!(scope, Some(CapabilityScope::Custom("memory".into())));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn tool_returns_empty_results_for_empty_store() {
        let (tool, dir) = setup_tool();
        let input = serde_json::json!({
            "query": "something",
            "top_k": 5,
        });
        let result = tool.execute(input).await.unwrap();
        assert_eq!(result["count"], 0);
        assert!(result["results"].as_array().unwrap().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn tool_returns_memories_from_store() {
        let dir =
            std::env::temp_dir().join(format!("aivyx-team-mem-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("memory.db");
        let store = MemoryStore::open(&db_path).unwrap();
        let key = aivyx_crypto::MasterKey::generate();
        let provider = Arc::new(MockEmbeddingProvider::new(4));
        let mut mgr = MemoryManager::new(store, provider, key, 0).unwrap();

        // Store a global memory
        mgr.remember(
            "Rust is a systems language".into(),
            aivyx_memory::MemoryKind::Fact,
            None,
            vec![],
        )
        .await
        .unwrap();

        let mgr_arc = Arc::new(Mutex::new(mgr));
        let tool = TeamMemoryQueryTool::new(mgr_arc, vec![AgentId::new()]);

        let input = serde_json::json!({
            "query": "Rust language",
            "top_k": 5,
        });
        let result = tool.execute(input).await.unwrap();
        assert!(result["count"].as_u64().unwrap() > 0);

        std::fs::remove_dir_all(&dir).ok();
    }
}
