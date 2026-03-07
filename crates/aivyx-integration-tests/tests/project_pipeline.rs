//! Integration tests for the project context pipeline: project config roundtrip,
//! project-scoped memory, CWD auto-detection, and codebase navigation tools.

use std::sync::Arc;

use aivyx_agent::built_in_tools::{ProjectOutlineTool, ProjectTreeTool};
use aivyx_config::{AivyxConfig, ProjectConfig};
use aivyx_core::{AgentId, Tool};
use aivyx_integration_tests::MockEmbeddingProvider;

/// Test 1: ProjectConfig add/find/remove roundtrip through AivyxConfig.
#[test]
fn project_config_roundtrip() {
    let mut config = AivyxConfig::default();
    assert!(config.projects.is_empty());

    let project = ProjectConfig::new("my-app", "/home/user/my-app");
    config.add_project(project.clone()).unwrap();

    // Find by name
    assert!(config.find_project("my-app").is_some());
    assert!(config.find_project("other").is_none());

    // Find by path — exact match
    let found = config
        .find_project_by_path(std::path::Path::new("/home/user/my-app"))
        .unwrap();
    assert_eq!(found.name, "my-app");

    // Find by path — subdirectory (prefix match)
    let found = config
        .find_project_by_path(std::path::Path::new("/home/user/my-app/src"))
        .unwrap();
    assert_eq!(found.name, "my-app");

    // No match for different path
    assert!(
        config
            .find_project_by_path(std::path::Path::new("/home/other"))
            .is_none()
    );

    // Name collision
    let dup = ProjectConfig::new("my-app", "/other/path");
    assert!(config.add_project(dup).is_err());

    // Remove
    let removed = config.remove_project("my-app").unwrap();
    assert_eq!(removed.name, "my-app");
    assert!(config.projects.is_empty());

    // Remove non-existent
    assert!(config.remove_project("my-app").is_err());
}

/// Test 2: Project-scoped memory recall filters by tag.
#[tokio::test]
async fn project_scoped_memory() {
    let db_path = std::env::temp_dir().join(format!("aivyx-proj-mem-{}", uuid::Uuid::new_v4()));
    let store = aivyx_memory::MemoryStore::open(&db_path).unwrap();

    let embedding_provider = Arc::new(MockEmbeddingProvider::new(64));
    let memory_key =
        aivyx_crypto::derive_memory_key(&aivyx_crypto::MasterKey::from_bytes([42u8; 32]));

    let mut mgr =
        aivyx_memory::MemoryManager::new(store, embedding_provider, memory_key, 100).unwrap();

    let agent_id = AgentId::new();

    // Store a global memory (no project tag)
    mgr.remember(
        "Rust is a systems language".to_string(),
        aivyx_memory::MemoryKind::Fact,
        Some(agent_id),
        vec![],
    )
    .await
    .unwrap();

    // Store a project-scoped memory
    mgr.remember(
        "The aivyx project uses workspace layout".to_string(),
        aivyx_memory::MemoryKind::Fact,
        Some(agent_id),
        vec!["project:aivyx".to_string()],
    )
    .await
    .unwrap();

    // Recall without tag filter — should find at least one of the two
    let all = mgr
        .recall("aivyx project Rust", 10, Some(agent_id), &[])
        .await
        .unwrap();
    assert!(
        !all.is_empty(),
        "expected at least 1 memory in unfiltered recall"
    );

    // Recall with project tag filter — only the tagged one
    let scoped = mgr
        .recall(
            "workspace layout",
            10,
            Some(agent_id),
            &["project:aivyx".to_string()],
        )
        .await
        .unwrap();
    assert!(!scoped.is_empty(), "expected at least 1 scoped memory");
    for entry in &scoped {
        assert!(
            entry.tags.contains(&"project:aivyx".to_string()),
            "all results should have the project tag"
        );
    }

    // Recall with different project tag — should NOT find the aivyx memory
    let other = mgr
        .recall(
            "workspace layout",
            10,
            Some(agent_id),
            &["project:other".to_string()],
        )
        .await
        .unwrap();
    assert!(
        other.is_empty(),
        "no memories should match project:other tag"
    );

    // Cleanup
    let _ = std::fs::remove_dir_all(&db_path);
}

/// Test 3: CWD-based project auto-detection on Agent.
#[test]
fn project_auto_detect_cwd() {
    let mut config = AivyxConfig::default();
    let project = ProjectConfig::new("aivyx", "/home/user/projects/aivyx");
    config.add_project(project).unwrap();

    // Simulate what create_agent_with_context does
    let cwd = std::path::Path::new("/home/user/projects/aivyx/crates/aivyx-agent");
    let found = config.find_project_by_path(cwd);
    assert!(found.is_some());
    assert_eq!(found.unwrap().name, "aivyx");

    // CWD outside any project
    let cwd_outside = std::path::Path::new("/home/user/documents");
    assert!(config.find_project_by_path(cwd_outside).is_none());
}

/// Test 4: ProjectTreeTool execution on a temp directory.
#[tokio::test]
async fn project_tree_tool_execution() {
    let root = std::env::temp_dir().join(format!("aivyx-tree-integ-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("src/models")).unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
    std::fs::write(root.join("src/models/user.rs"), "struct User {}").unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"test\"").unwrap();
    // These should be excluded
    std::fs::create_dir_all(root.join("target/debug")).unwrap();
    std::fs::write(root.join("target/debug/test"), "binary").unwrap();
    std::fs::create_dir_all(root.join("node_modules/foo")).unwrap();
    std::fs::write(root.join("node_modules/foo/index.js"), "").unwrap();

    let tool = ProjectTreeTool::new();
    let result = tool
        .execute(serde_json::json!({
            "path": root.to_str().unwrap(),
            "max_depth": 4,
        }))
        .await
        .unwrap();

    let tree = result["tree"].as_str().unwrap();

    // Should include source files
    assert!(tree.contains("src/"), "tree should contain src/");
    assert!(tree.contains("main.rs"), "tree should contain main.rs");
    assert!(tree.contains("models/"), "tree should contain models/");
    assert!(
        tree.contains("Cargo.toml"),
        "tree should contain Cargo.toml"
    );

    // Should exclude build artifacts
    assert!(!tree.contains("target/"), "tree should NOT contain target/");
    assert!(
        !tree.contains("node_modules/"),
        "tree should NOT contain node_modules/"
    );

    // Entry count should be reasonable
    let count = result["entry_count"].as_u64().unwrap();
    assert!(count >= 4 && count <= 10, "unexpected entry count: {count}");

    let _ = std::fs::remove_dir_all(&root);
}

/// Test 5: ProjectOutlineTool execution on a temp Rust file.
#[tokio::test]
async fn project_outline_tool_execution() {
    let dir = std::env::temp_dir().join(format!("aivyx-outline-integ-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();

    let file = dir.join("lib.rs");
    std::fs::write(
        &file,
        r#"
pub struct Agent {
    name: String,
    id: u64,
}

impl Agent {
    pub fn new(name: &str) -> Self {
        Agent { name: name.into(), id: 0 }
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

pub enum Status {
    Active,
    Inactive,
}

pub trait Runnable {
    fn run(&self);
}

pub fn create_agent(name: &str) -> Agent {
    Agent::new(name)
}
"#,
    )
    .unwrap();

    let tool = ProjectOutlineTool::new();
    let result = tool
        .execute(serde_json::json!({ "path": file.to_str().unwrap() }))
        .await
        .unwrap();

    assert_eq!(result["language"], "rust");

    let item_count = result["item_count"].as_u64().unwrap();
    // pub struct Agent, impl Agent, pub fn new, pub fn name, pub enum Status, pub trait Runnable, fn run, pub fn create_agent
    assert!(
        item_count >= 6,
        "expected at least 6 outline items, got {item_count}"
    );

    let outline = result["outline"].as_str().unwrap();
    assert!(outline.contains("struct"), "outline should contain struct");
    assert!(outline.contains("impl"), "outline should contain impl");
    assert!(outline.contains("enum"), "outline should contain enum");
    assert!(outline.contains("trait"), "outline should contain trait");
    assert!(
        outline.contains("function"),
        "outline should contain function"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
