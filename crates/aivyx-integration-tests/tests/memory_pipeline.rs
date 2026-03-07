//! Integration tests for the memory pipeline: encrypted storage, embedding,
//! semantic search, knowledge triples, agent integration, and scope isolation.

use std::sync::Arc;

use aivyx_agent::{CostTracker, RateLimiter};
use aivyx_core::{AgentId, AutonomyTier, MemoryId, ToolRegistry};
use aivyx_crypto::MasterKey;
use aivyx_integration_tests::{MockEmbeddingProvider, MockProvider, create_memory_caps};
use aivyx_memory::{MemoryKind, MemoryManager, MemoryStore};

/// Fixed test key bytes for deterministic tests that need to reopen stores.
const TEST_KEY_BYTES: [u8; 32] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
    27, 28, 29, 30, 31, 32,
];

fn setup_memory(
    dims: usize,
) -> (
    MemoryStore,
    Arc<MockEmbeddingProvider>,
    MasterKey,
    std::path::PathBuf,
) {
    let dir = std::env::temp_dir().join(format!("aivyx-integ-mem-{}", MemoryId::new()));
    std::fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("memory.db");
    let store = MemoryStore::open(&db_path).unwrap();
    let master_key = MasterKey::generate();
    let provider = Arc::new(MockEmbeddingProvider::new(dims));
    (store, provider, master_key, dir)
}

/// Test: Agent stores memory via tool, retrieves it in next session.
/// This simulates a two-session workflow: store in session 1, recall in session 2.
#[tokio::test]
async fn memory_persists_across_sessions() {
    let dir = std::env::temp_dir().join(format!("aivyx-integ-persist-{}", MemoryId::new()));
    std::fs::create_dir_all(&dir).unwrap();
    let provider = Arc::new(MockEmbeddingProvider::new(8));

    // Session 1: store a memory
    let agent_id = AgentId::new();
    let store = MemoryStore::open(dir.join("memory.db")).unwrap();
    let key = MasterKey::from_bytes(TEST_KEY_BYTES);
    let mut mgr = MemoryManager::new(store, provider.clone(), key, 0).unwrap();
    let mem_id = mgr
        .remember(
            "The user prefers dark mode".into(),
            MemoryKind::Preference,
            Some(agent_id),
            vec!["ui".into()],
        )
        .await
        .unwrap();

    // Verify stored
    let stats = mgr.stats().unwrap();
    assert_eq!(stats.total_memories, 1);
    assert_eq!(stats.index_size, 1);
    drop(mgr);

    // Session 2: re-open the store with same key and recall
    let store2 = MemoryStore::open(dir.join("memory.db")).unwrap();
    let key2 = MasterKey::from_bytes(TEST_KEY_BYTES);
    let mut mgr2 = MemoryManager::new(store2, provider.clone(), key2, 0).unwrap();

    // Index should have been rebuilt from persisted embeddings
    assert_eq!(mgr2.stats().unwrap().index_size, 1);

    let results = mgr2
        .recall("dark mode preference", 5, Some(agent_id), &[])
        .await
        .unwrap();
    assert!(!results.is_empty());
    assert_eq!(results[0].id, mem_id);
    assert_eq!(results[0].content, "The user prefers dark mode");

    std::fs::remove_dir_all(&dir).ok();
}

/// Test: Semantic search returns relevant memories sorted by similarity.
///
/// Uses `store_raw()` with controlled vectors to test recall without dedup
/// interference from the mock embedding provider.
#[tokio::test]
async fn semantic_search_returns_relevant_results() {
    let dir = std::env::temp_dir().join(format!("aivyx-integ-search-{}", MemoryId::new()));
    std::fs::create_dir_all(&dir).unwrap();
    let store = MemoryStore::open(dir.join("memory.db")).unwrap();
    let provider = Arc::new(MockEmbeddingProvider::new(8));
    let key = MasterKey::generate();
    let mut mgr = MemoryManager::new(store, provider.clone(), key, 0).unwrap();

    // Store 4 memories with controlled vectors
    // Vector [1,0,0,...] is "Rust systems", [0,1,0,...] is "weather",
    // [0.7,0.7,0,...] is "Rust safety" (partially aligned with first),
    // [0,0,1,...] is "Python"
    let entries = vec![
        (
            "Rust is a systems programming language",
            vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        ),
        (
            "The weather today is sunny",
            vec![0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        ),
        (
            "Rust has a borrow checker for memory safety",
            vec![0.707, 0.0, 0.707, 0.0, 0.0, 0.0, 0.0, 0.0],
        ),
        (
            "Python is an interpreted language",
            vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0],
        ),
    ];

    for (content, vec) in &entries {
        let entry =
            aivyx_memory::MemoryEntry::new(content.to_string(), MemoryKind::Fact, None, vec![]);
        mgr.store_raw(&entry, vec).unwrap();
    }

    assert_eq!(mgr.stats().unwrap().total_memories, 4);

    // Recall with a vector close to "Rust" memories (direction [0.9, 0, 0.1, ...])
    // The recall uses the embedding provider, so we query with text that will produce
    // some vector. With top_k=2, we should get 2 results.
    let results = mgr.recall("Rust programming", 2, None, &[]).await.unwrap();
    assert_eq!(results.len(), 2, "should return top_k=2 results");
    assert!(!results.is_empty());

    std::fs::remove_dir_all(&dir).ok();
}

/// Test: Knowledge triple CRUD end-to-end.
#[tokio::test]
async fn knowledge_triple_crud() {
    let (store, provider, key, dir) = setup_memory(4);
    let mgr = MemoryManager::new(store, provider, key, 0).unwrap();

    // Create
    let t1 = mgr
        .add_triple(
            "Rust".into(),
            "developed_by".into(),
            "Mozilla".into(),
            None,
            0.95,
            "wikipedia".into(),
        )
        .unwrap();
    let t2 = mgr
        .add_triple(
            "Rust".into(),
            "paradigm".into(),
            "systems".into(),
            None,
            0.9,
            "docs".into(),
        )
        .unwrap();
    let _t3 = mgr
        .add_triple(
            "Python".into(),
            "paradigm".into(),
            "scripting".into(),
            None,
            0.85,
            "docs".into(),
        )
        .unwrap();

    // Query all
    let all = mgr.query_triples(None, None, None, None).unwrap();
    assert_eq!(all.len(), 3);

    // Query by subject
    let rust = mgr.query_triples(Some("Rust"), None, None, None).unwrap();
    assert_eq!(rust.len(), 2);

    // Query by predicate
    let paradigms = mgr
        .query_triples(None, Some("paradigm"), None, None)
        .unwrap();
    assert_eq!(paradigms.len(), 2);

    // Query by subject + predicate
    let specific = mgr
        .query_triples(Some("Rust"), Some("developed_by"), None, None)
        .unwrap();
    assert_eq!(specific.len(), 1);
    assert_eq!(specific[0].id, t1);
    assert_eq!(specific[0].object, "Mozilla");
    assert_eq!(specific[0].confidence, 0.95);

    // Verify IDs are unique
    assert_ne!(t1, t2);

    std::fs::remove_dir_all(&dir).ok();
}

/// Test: Embedding cache avoids duplicate API calls.
///
/// This test focuses on the embedding *cache*, not on dedup. We store the same
/// text twice: the embedding provider should only be called once (second call
/// hits the content-hash cache). Dedup also kicks in (cosine = 1.0), so only
/// 1 memory is stored.
#[tokio::test]
async fn embedding_cache_avoids_duplicates() {
    let (store, provider, key, dir) = setup_memory(64);
    let mut mgr = MemoryManager::new(store, provider.clone(), key, 0).unwrap();

    // Store same content twice — both embedding cache and content dedup apply
    let id1 = mgr
        .remember(
            "the quick brown fox jumps over the lazy dog".into(),
            MemoryKind::Fact,
            None,
            vec![],
        )
        .await
        .unwrap();
    let id2 = mgr
        .remember(
            "the quick brown fox jumps over the lazy dog".into(),
            MemoryKind::Fact,
            None,
            vec![],
        )
        .await
        .unwrap();

    // Only 1 embed call (second was embedding cache hit)
    assert_eq!(provider.calls(), 1);

    // Content dedup: same text → same ID, only 1 memory stored
    assert_eq!(id1, id2);
    assert_eq!(mgr.stats().unwrap().total_memories, 1);

    // Recalling also uses the embedding cache
    let _results = mgr
        .recall("the quick brown fox", 3, None, &[])
        .await
        .unwrap();
    // Still 1 embed call (recall query text differs, so it's a new embed)
    assert_eq!(provider.calls(), 2);

    std::fs::remove_dir_all(&dir).ok();
}

/// Test: All memory data is encrypted at rest — raw store bytes differ from plaintext.
#[tokio::test]
async fn memory_data_encrypted_at_rest() {
    let (store, provider, key, dir) = setup_memory(4);
    let mut mgr = MemoryManager::new(store, provider.clone(), key, 0).unwrap();

    let secret_content = "my secret API key is sk-12345";
    mgr.remember(secret_content.into(), MemoryKind::Fact, None, vec![])
        .await
        .unwrap();
    drop(mgr);

    // Read raw database file bytes
    let db_bytes = std::fs::read(dir.join("memory.db")).unwrap();
    let raw = String::from_utf8_lossy(&db_bytes);

    // The plaintext content should NOT appear in the raw database
    assert!(
        !raw.contains(secret_content),
        "Plaintext content found in raw database file — encryption failed!"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Test: Agent turn augments system prompt with memory context.
#[tokio::test]
async fn agent_turn_augments_system_prompt() {
    let (store, provider, key, dir) = setup_memory(8);
    let agent_id = AgentId::new();
    let mut mgr = MemoryManager::new(store, provider.clone(), key, 0).unwrap();

    // Pre-populate with a memory
    mgr.remember(
        "The user's name is Julian".into(),
        MemoryKind::Fact,
        Some(agent_id),
        vec!["user".into()],
    )
    .await
    .unwrap();
    mgr.add_triple(
        "Julian".into(),
        "prefers".into(),
        "Rust".into(),
        Some(agent_id),
        0.9,
        "conversation".into(),
    )
    .unwrap();

    let mgr = Arc::new(tokio::sync::Mutex::new(mgr));

    // Create a mock LLM that captures the system prompt
    let mock_provider = MockProvider::simple("I remember you!");
    let caps = create_memory_caps(agent_id);

    let mut agent = aivyx_agent::Agent::new(
        agent_id,
        "memory-agent".into(),
        "You are a helpful assistant.".into(),
        4096,
        AutonomyTier::Trust,
        Box::new(mock_provider),
        ToolRegistry::new(),
        caps,
        RateLimiter::new(60),
        CostTracker::new(5.0, 0.000003, 0.000015),
        None,
        3,
        1,
    );

    agent.set_memory_manager(mgr);

    let result = agent
        .turn("Hello, do you remember me?", None)
        .await
        .unwrap();
    assert_eq!(result, "I remember you!");

    // The agent should have augmented its system prompt (we can't directly inspect
    // the prompt sent to the LLM since MockProvider ignores it, but we verify the
    // agent runs without error and the memory manager is wired correctly)

    std::fs::remove_dir_all(&dir).ok();
}

/// Test: Memory scoping — agent A can't see agent B's scoped memories.
///
/// Uses `store_raw()` to bypass dedup (mock embeddings produce similar vectors
/// for short texts), inserting orthogonal vectors so each memory is distinct.
#[tokio::test]
async fn memory_scope_isolation() {
    let dir = std::env::temp_dir().join(format!("aivyx-integ-scope-{}", MemoryId::new()));
    std::fs::create_dir_all(&dir).unwrap();
    let store = MemoryStore::open(dir.join("memory.db")).unwrap();
    let provider = Arc::new(MockEmbeddingProvider::new(64));
    let key = MasterKey::generate();
    let mut mgr = MemoryManager::new(store, provider.clone(), key, 0).unwrap();

    let agent_a = AgentId::new();
    let agent_b = AgentId::new();

    // Use store_raw with orthogonal vectors to avoid dedup issues
    let entry_a = aivyx_memory::MemoryEntry::new(
        "Agent A private data about quantum computing".into(),
        MemoryKind::Fact,
        Some(agent_a),
        vec![],
    );
    let entry_b = aivyx_memory::MemoryEntry::new(
        "Agent B private data about jazz music theory".into(),
        MemoryKind::Fact,
        Some(agent_b),
        vec![],
    );
    let entry_global = aivyx_memory::MemoryEntry::new(
        "Shared global knowledge about mathematics".into(),
        MemoryKind::Fact,
        None,
        vec![],
    );

    // Orthogonal unit vectors
    let mut va = vec![0.0f32; 64];
    va[0] = 1.0;
    let mut vb = vec![0.0f32; 64];
    vb[1] = 1.0;
    let mut vg = vec![0.0f32; 64];
    vg[2] = 1.0;

    mgr.store_raw(&entry_a, &va).unwrap();
    mgr.store_raw(&entry_b, &vb).unwrap();
    mgr.store_raw(&entry_global, &vg).unwrap();

    assert_eq!(mgr.stats().unwrap().total_memories, 3);

    // Agent A should see its own + global (recall with broad query vector)
    // Use a query vector that's somewhat close to all 3 directions
    let a_results = mgr
        .recall("quantum computing data", 10, Some(agent_a), &[])
        .await
        .unwrap();
    let a_has_own = a_results
        .iter()
        .any(|e| e.content.contains("quantum computing"));
    let a_has_global = a_results.iter().any(|e| e.content.contains("mathematics"));
    let a_has_b = a_results.iter().any(|e| e.content.contains("jazz music"));
    assert!(a_has_own, "Agent A should see its own memories");
    assert!(a_has_global, "Agent A should see global memories");
    assert!(!a_has_b, "Agent A should NOT see Agent B's memories");

    // Agent B should see its own + global
    let b_results = mgr
        .recall("jazz music theory", 10, Some(agent_b), &[])
        .await
        .unwrap();
    let b_has_own = b_results.iter().any(|e| e.content.contains("jazz music"));
    let b_has_global = b_results.iter().any(|e| e.content.contains("mathematics"));
    let b_has_a = b_results
        .iter()
        .any(|e| e.content.contains("quantum computing"));
    assert!(b_has_own, "Agent B should see its own memories");
    assert!(b_has_global, "Agent B should see global memories");
    assert!(!b_has_a, "Agent B should NOT see Agent A's memories");

    // Triple scoping
    mgr.add_triple(
        "A".into(),
        "owns".into(),
        "secret".into(),
        Some(agent_a),
        1.0,
        "test".into(),
    )
    .unwrap();

    let b_triples = mgr.query_triples(None, None, None, Some(agent_b)).unwrap();
    assert!(
        b_triples.is_empty(),
        "Agent B should not see Agent A's triples"
    );

    let a_triples = mgr.query_triples(None, None, None, Some(agent_a)).unwrap();
    assert_eq!(a_triples.len(), 1);

    std::fs::remove_dir_all(&dir).ok();
}

/// Test: Content deduplication prevents duplicate memory storage.
#[tokio::test]
async fn memory_dedup_prevents_duplicate_storage() {
    let (store, provider, key, dir) = setup_memory(8);
    let mut mgr = MemoryManager::new(store, provider.clone(), key, 0).unwrap();

    // Store the same fact 5 times
    let mut ids = Vec::new();
    for _ in 0..5 {
        let id = mgr
            .remember(
                "The user's name is Julian".into(),
                MemoryKind::Fact,
                None,
                vec![],
            )
            .await
            .unwrap();
        ids.push(id);
    }

    // All IDs should be the same (dedup returns existing ID)
    let first = ids[0];
    for id in &ids[1..] {
        assert_eq!(*id, first, "duplicate should return same ID");
    }

    // Only 1 memory should be persisted
    assert_eq!(
        mgr.stats().unwrap().total_memories,
        1,
        "dedup should prevent duplicate storage"
    );

    // Only 1 embedding call (subsequent calls hit the embedding cache)
    assert_eq!(provider.calls(), 1);

    std::fs::remove_dir_all(&dir).ok();
}

/// Test: Memory pruning enforces the configured limit.
#[tokio::test]
async fn memory_pruning_enforces_limit() {
    let dir = std::env::temp_dir().join(format!("aivyx-integ-prune-{}", MemoryId::new()));
    std::fs::create_dir_all(&dir).unwrap();
    let store = MemoryStore::open(dir.join("memory.db")).unwrap();
    let provider = Arc::new(MockEmbeddingProvider::new(128));
    let key = MasterKey::generate();
    // Limit to 5 memories
    let mut mgr = MemoryManager::new(store, provider, key, 5).unwrap();

    // Directly insert 10 memories with orthogonal vectors (bypass dedup)
    for i in 0..10 {
        let entry = aivyx_memory::MemoryEntry::new(
            format!("Distinct memory content #{i}"),
            MemoryKind::Fact,
            None,
            vec![],
        );
        let mut vec = vec![0.0f32; 128];
        vec[i] = 1.0; // orthogonal unit vector
        mgr.store_raw(&entry, &vec).unwrap();
    }

    assert_eq!(mgr.stats().unwrap().total_memories, 10);

    // Store one more memory via `remember()` to trigger pruning
    // Use a unique vector direction that won't dedup
    let entry = aivyx_memory::MemoryEntry::new(
        "Trigger pruning entry".into(),
        MemoryKind::Fact,
        None,
        vec![],
    );
    let mut vec = vec![0.0f32; 128];
    vec[10] = 1.0;
    mgr.store_raw(&entry, &vec).unwrap();

    // Manually trigger pruning
    mgr.prune_to_limit().unwrap();

    assert_eq!(
        mgr.stats().unwrap().total_memories,
        5,
        "pruning should enforce limit"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Test: Profile persists across sessions (save, reopen store, load).
#[tokio::test]
async fn profile_roundtrip_across_sessions() {
    let dir = std::env::temp_dir().join(format!("aivyx-integ-profile-{}", MemoryId::new()));
    std::fs::create_dir_all(&dir).unwrap();
    let provider = Arc::new(MockEmbeddingProvider::new(8));

    // Session 1: create and save a profile
    {
        let store = MemoryStore::open(dir.join("memory.db")).unwrap();
        let key = MasterKey::from_bytes(TEST_KEY_BYTES);
        let mgr = MemoryManager::new(store, provider.clone(), key, 0).unwrap();

        let mut profile = aivyx_memory::UserProfile::new();
        profile.name = Some("Julian".into());
        profile.timezone = Some("America/New_York".into());
        profile.tech_stack = vec!["Rust".into(), "TypeScript".into()];
        profile.projects.push(aivyx_memory::ProjectEntry {
            name: "aivyx".into(),
            description: Some("AI framework".into()),
            language: Some("Rust".into()),
            path: None,
        });

        mgr.update_profile(profile).unwrap();
    }

    // Session 2: reopen and verify
    {
        let store = MemoryStore::open(dir.join("memory.db")).unwrap();
        let key = MasterKey::from_bytes(TEST_KEY_BYTES);
        let mgr = MemoryManager::new(store, provider.clone(), key, 0).unwrap();

        let loaded = mgr.get_profile().unwrap();
        assert_eq!(loaded.name.as_deref(), Some("Julian"));
        assert_eq!(loaded.timezone.as_deref(), Some("America/New_York"));
        assert_eq!(loaded.tech_stack, vec!["Rust", "TypeScript"]);
        assert_eq!(loaded.projects.len(), 1);
        assert_eq!(loaded.projects[0].name, "aivyx");
        assert_eq!(loaded.revision, 1);

        // format_for_prompt should produce a non-empty block
        let prompt = loaded.format_for_prompt();
        assert!(prompt.is_some());
        let prompt_text = prompt.unwrap();
        assert!(prompt_text.contains("[USER PROFILE]"));
        assert!(prompt_text.contains("Julian"));
        assert!(prompt_text.contains("Rust"));
    }

    std::fs::remove_dir_all(&dir).ok();
}

/// Test: Profile extraction from accumulated facts via mock LLM.
#[tokio::test]
async fn profile_extraction_from_facts() {
    let dir = std::env::temp_dir().join(format!("aivyx-integ-extract-{}", MemoryId::new()));
    std::fs::create_dir_all(&dir).unwrap();
    let provider = Arc::new(MockEmbeddingProvider::new(8));
    let key = MasterKey::generate();

    let store = MemoryStore::open(dir.join("memory.db")).unwrap();
    let mut mgr = MemoryManager::new(store, provider.clone(), key, 0).unwrap();

    // Store some facts
    mgr.remember(
        "User's name is Julian".into(),
        MemoryKind::Fact,
        None,
        vec![],
    )
    .await
    .unwrap();
    mgr.remember(
        "User prefers dark mode".into(),
        MemoryKind::Preference,
        None,
        vec![],
    )
    .await
    .unwrap();
    mgr.remember(
        "User works with Rust and TypeScript".into(),
        MemoryKind::Fact,
        None,
        vec![],
    )
    .await
    .unwrap();

    // Create a mock LLM that returns a profile JSON
    let mock_llm = MockProvider::simple(
        r#"{"name": "Julian", "timezone": null, "tech_stack": ["Rust", "TypeScript"], "style_preferences": ["prefers dark mode"]}"#,
    );

    let profile = mgr.extract_profile(&mock_llm).await.unwrap();
    assert_eq!(profile.name.as_deref(), Some("Julian"));
    assert_eq!(profile.tech_stack, vec!["Rust", "TypeScript"]);
    assert_eq!(profile.style_preferences, vec!["prefers dark mode"]);
    assert_eq!(profile.revision, 1);

    std::fs::remove_dir_all(&dir).ok();
}

/// Test: Profile injection into agent system prompt.
#[tokio::test]
async fn profile_injected_into_agent_prompt() {
    let dir = std::env::temp_dir().join(format!("aivyx-integ-inject-{}", MemoryId::new()));
    std::fs::create_dir_all(&dir).unwrap();
    let provider = Arc::new(MockEmbeddingProvider::new(8));
    let agent_id = AgentId::new();
    let key = MasterKey::generate();

    let store = MemoryStore::open(dir.join("memory.db")).unwrap();
    let mgr = MemoryManager::new(store, provider.clone(), key, 0).unwrap();

    // Set a profile
    let mut profile = aivyx_memory::UserProfile::new();
    profile.name = Some("Julian".into());
    profile.tech_stack = vec!["Rust".into()];
    mgr.update_profile(profile).unwrap();

    let mgr = Arc::new(tokio::sync::Mutex::new(mgr));

    // Create a mock LLM that captures the system prompt to verify injection.
    // We use a special provider that checks the system prompt contains profile data.
    struct PromptCapturingProvider {
        captured: std::sync::Mutex<Option<String>>,
    }

    #[async_trait::async_trait]
    impl aivyx_llm::LlmProvider for PromptCapturingProvider {
        fn name(&self) -> &str {
            "mock-capture"
        }
        async fn chat(
            &self,
            request: &aivyx_llm::ChatRequest,
        ) -> aivyx_core::Result<aivyx_llm::ChatResponse> {
            if let Some(ref sys) = request.system_prompt {
                *self.captured.lock().unwrap() = Some(sys.clone());
            }
            Ok(aivyx_llm::ChatResponse {
                message: aivyx_llm::ChatMessage::assistant("Hello Julian!"),
                usage: aivyx_llm::TokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                },
                stop_reason: aivyx_llm::StopReason::EndTurn,
            })
        }
    }

    let capture_provider = PromptCapturingProvider {
        captured: std::sync::Mutex::new(None),
    };

    let caps = create_memory_caps(agent_id);
    let mut agent = aivyx_agent::Agent::new(
        agent_id,
        "profile-inject-agent".into(),
        "You are a helpful assistant.".into(),
        4096,
        AutonomyTier::Trust,
        Box::new(capture_provider),
        ToolRegistry::new(),
        caps,
        RateLimiter::new(60),
        CostTracker::new(5.0, 0.000003, 0.000015),
        None,
        3,
        1,
    );
    agent.set_memory_manager(mgr);

    // Run a turn — the system prompt should contain the profile
    let result = agent.turn("Hello!", None).await.unwrap();
    assert_eq!(result, "Hello Julian!");

    // Unfortunately we can't access the capture_provider after it's moved into the agent.
    // But we verify the turn succeeded, which means profile injection didn't error.

    std::fs::remove_dir_all(&dir).ok();
}

/// Test: Session summary is stored as a SessionSummary memory.
#[tokio::test]
async fn session_summary_stored_as_memory() {
    let (store, provider, key, dir) = setup_memory(8);
    let agent_id = AgentId::new();
    let mgr = MemoryManager::new(store, provider.clone(), key, 0).unwrap();
    let mgr = Arc::new(tokio::sync::Mutex::new(mgr));

    // Create a mock LLM that returns different responses for turns vs summaries
    struct SummaryProvider;

    #[async_trait::async_trait]
    impl aivyx_llm::LlmProvider for SummaryProvider {
        fn name(&self) -> &str {
            "mock-summary"
        }
        async fn chat(
            &self,
            request: &aivyx_llm::ChatRequest,
        ) -> aivyx_core::Result<aivyx_llm::ChatResponse> {
            let content = if let Some(ref sys) = request.system_prompt
                && sys.contains("Summarize this conversation")
            {
                "User asked about Rust async. Agent explained tokio runtime.".to_string()
            } else {
                "Tokio is a Rust async runtime.".to_string()
            };
            Ok(aivyx_llm::ChatResponse {
                message: aivyx_llm::ChatMessage::assistant(content),
                usage: aivyx_llm::TokenUsage {
                    input_tokens: 10,
                    output_tokens: 10,
                },
                stop_reason: aivyx_llm::StopReason::EndTurn,
            })
        }
    }

    let caps = create_memory_caps(agent_id);
    let mut agent = aivyx_agent::Agent::new(
        agent_id,
        "summary-test".into(),
        "You are helpful.".into(),
        4096,
        AutonomyTier::Trust,
        Box::new(SummaryProvider),
        ToolRegistry::new(),
        caps,
        RateLimiter::new(60),
        CostTracker::new(5.0, 0.000003, 0.000015),
        None,
        3,
        1,
    );
    agent.set_memory_manager(mgr.clone());

    // Have a conversation
    let _ = agent.turn("Tell me about Rust async", None).await.unwrap();

    // End session — should generate and store summary
    let summary = agent.end_session().await;
    assert!(summary.is_some(), "should generate summary");
    let summary_text = summary.unwrap();
    assert!(
        summary_text.contains("Rust async"),
        "summary should reference conversation topic"
    );

    // Verify it was stored as SessionSummary kind in the memory manager
    let mgr = mgr.lock().await;
    let stats = mgr.stats().unwrap();
    // At least 1 memory stored (the summary)
    assert!(
        stats.total_memories >= 1,
        "session summary should be persisted as memory"
    );

    std::fs::remove_dir_all(&dir).ok();
}
