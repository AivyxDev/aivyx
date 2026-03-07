//! Encrypted persistence for task missions.
//!
//! [`TaskStore`] wraps [`EncryptedStore`] with a `"task:{id}"` key namespace.
//! Follows the same pattern as [`SessionStore`](aivyx_agent::SessionStore).

use std::path::Path;

use aivyx_core::{AivyxError, Result, TaskId};
use aivyx_crypto::{EncryptedStore, MasterKey};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::{Mission, TaskStatus};

/// Summary metadata for listing missions without loading full state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskMetadata {
    /// Unique task identifier.
    pub id: TaskId,
    /// The original goal.
    pub goal: String,
    /// Agent profile name.
    pub agent_name: String,
    /// Current lifecycle status.
    pub status: TaskStatus,
    /// Number of completed steps.
    pub steps_completed: usize,
    /// Total number of steps.
    pub steps_total: usize,
    /// When the mission was created.
    pub created_at: DateTime<Utc>,
    /// When the mission was last updated.
    pub updated_at: DateTime<Utc>,
}

impl From<&Mission> for TaskMetadata {
    fn from(m: &Mission) -> Self {
        Self {
            id: m.id,
            goal: m.goal.clone(),
            agent_name: m.agent_name.clone(),
            status: m.status.clone(),
            steps_completed: m.steps_completed(),
            steps_total: m.steps.len(),
            created_at: m.created_at,
            updated_at: m.updated_at,
        }
    }
}

/// Encrypted persistence for task missions.
pub struct TaskStore {
    store: EncryptedStore,
}

impl TaskStore {
    /// Open or create a task store at the given path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let store = EncryptedStore::open(path)?;
        Ok(Self { store })
    }

    /// Save a mission (create or update).
    pub fn save(&self, mission: &Mission, key: &MasterKey) -> Result<()> {
        let store_key = format!("task:{}", mission.id);
        let json = serde_json::to_vec(mission).map_err(AivyxError::Serialization)?;
        self.store.put(&store_key, &json, key)
    }

    /// Load a mission by ID.
    pub fn load(&self, task_id: &TaskId, key: &MasterKey) -> Result<Option<Mission>> {
        let store_key = format!("task:{task_id}");
        match self.store.get(&store_key, key)? {
            Some(bytes) => {
                let mission: Mission =
                    serde_json::from_slice(&bytes).map_err(AivyxError::Serialization)?;
                Ok(Some(mission))
            }
            None => Ok(None),
        }
    }

    /// List all missions as metadata summaries.
    pub fn list(&self, key: &MasterKey) -> Result<Vec<TaskMetadata>> {
        let keys = self.store.list_keys()?;
        let mut metadata = Vec::new();
        for k in keys {
            if let Some(id_str) = k.strip_prefix("task:") {
                let task_id: TaskId = id_str
                    .parse()
                    .map_err(|e| AivyxError::Storage(format!("invalid task ID: {e}")))?;
                if let Some(mission) = self.load(&task_id, key)? {
                    metadata.push(TaskMetadata::from(&mission));
                }
            }
        }
        Ok(metadata)
    }

    /// Delete a mission by ID.
    pub fn delete(&self, task_id: &TaskId) -> Result<()> {
        let store_key = format!("task:{task_id}");
        self.store.delete(&store_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Step, StepStatus};

    fn test_mission() -> Mission {
        let mut m = Mission::new("test goal", "agent1");
        m.status = TaskStatus::Planned;
        m.steps = vec![Step {
            index: 0,
            description: "step 0".into(),
            tool_hints: vec!["web_search".into()],
            status: StepStatus::Pending,
            prompt: None,
            result: None,
            retries: 0,
            started_at: None,
            completed_at: None,
        }];
        m
    }

    #[test]
    fn save_load_roundtrip() {
        let dir =
            std::env::temp_dir().join(format!("aivyx-task-store-{}", aivyx_core::TaskId::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = TaskStore::open(dir.join("tasks.db")).unwrap();
        let key = MasterKey::generate();

        let mission = test_mission();
        store.save(&mission, &key).unwrap();

        let loaded = store.load(&mission.id, &key).unwrap().unwrap();
        assert_eq!(loaded.goal, "test goal");
        assert_eq!(loaded.steps.len(), 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_missing_returns_none() {
        let dir =
            std::env::temp_dir().join(format!("aivyx-task-store-{}", aivyx_core::TaskId::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = TaskStore::open(dir.join("tasks.db")).unwrap();
        let key = MasterKey::generate();

        let result = store.load(&TaskId::new(), &key).unwrap();
        assert!(result.is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_and_delete() {
        let dir =
            std::env::temp_dir().join(format!("aivyx-task-store-{}", aivyx_core::TaskId::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = TaskStore::open(dir.join("tasks.db")).unwrap();
        let key = MasterKey::generate();

        let m1 = test_mission();
        let m2 = test_mission();
        store.save(&m1, &key).unwrap();
        store.save(&m2, &key).unwrap();

        let list = store.list(&key).unwrap();
        assert_eq!(list.len(), 2);

        store.delete(&m1.id).unwrap();
        let list = store.list(&key).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, m2.id);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn metadata_from_mission() {
        let mut m = test_mission();
        m.steps[0].status = StepStatus::Completed;
        let meta = TaskMetadata::from(&m);
        assert_eq!(meta.steps_completed, 1);
        assert_eq!(meta.steps_total, 1);
    }

    #[test]
    fn store_save_load_roundtrip() {
        let dir =
            std::env::temp_dir().join(format!("aivyx-task-store-rt-{}", aivyx_core::TaskId::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = TaskStore::open(dir.join("tasks.db")).unwrap();
        let key = MasterKey::generate();

        let mut mission = test_mission();
        mission.goal = "roundtrip goal".into();
        mission.agent_name = "roundtrip-agent".into();
        store.save(&mission, &key).unwrap();

        let loaded = store.load(&mission.id, &key).unwrap().unwrap();
        assert_eq!(loaded.id, mission.id);
        assert_eq!(loaded.goal, "roundtrip goal");
        assert_eq!(loaded.agent_name, "roundtrip-agent");
        assert_eq!(loaded.steps.len(), 1);
        assert_eq!(loaded.steps[0].description, "step 0");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn store_load_nonexistent() {
        let dir =
            std::env::temp_dir().join(format!("aivyx-task-store-ne-{}", aivyx_core::TaskId::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = TaskStore::open(dir.join("tasks.db")).unwrap();
        let key = MasterKey::generate();

        let random_id = TaskId::new();
        let result = store.load(&random_id, &key).unwrap();
        assert!(result.is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn store_delete_removes_mission() {
        let dir = std::env::temp_dir().join(format!(
            "aivyx-task-store-del-{}",
            aivyx_core::TaskId::new()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = TaskStore::open(dir.join("tasks.db")).unwrap();
        let key = MasterKey::generate();

        let mission = test_mission();
        store.save(&mission, &key).unwrap();
        assert!(store.load(&mission.id, &key).unwrap().is_some());

        store.delete(&mission.id).unwrap();
        assert!(store.load(&mission.id, &key).unwrap().is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn store_list_multiple() {
        let dir =
            std::env::temp_dir().join(format!("aivyx-task-store-lm-{}", aivyx_core::TaskId::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = TaskStore::open(dir.join("tasks.db")).unwrap();
        let key = MasterKey::generate();

        let m1 = test_mission();
        let m2 = test_mission();
        let m3 = test_mission();
        store.save(&m1, &key).unwrap();
        store.save(&m2, &key).unwrap();
        store.save(&m3, &key).unwrap();

        let list = store.list(&key).unwrap();
        assert_eq!(list.len(), 3);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn store_list_empty() {
        let dir =
            std::env::temp_dir().join(format!("aivyx-task-store-le-{}", aivyx_core::TaskId::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = TaskStore::open(dir.join("tasks.db")).unwrap();
        let key = MasterKey::generate();

        let list = store.list(&key).unwrap();
        assert!(list.is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn store_overwrite_existing() {
        let dir =
            std::env::temp_dir().join(format!("aivyx-task-store-ow-{}", aivyx_core::TaskId::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = TaskStore::open(dir.join("tasks.db")).unwrap();
        let key = MasterKey::generate();

        let mut mission = test_mission();
        let original_id = mission.id;
        mission.goal = "original goal".into();
        store.save(&mission, &key).unwrap();

        mission.goal = "updated goal".into();
        mission.status = TaskStatus::Executing;
        store.save(&mission, &key).unwrap();

        let loaded = store.load(&original_id, &key).unwrap().unwrap();
        assert_eq!(loaded.goal, "updated goal");
        assert_eq!(loaded.status, TaskStatus::Executing);

        // Verify there's still only one entry
        let list = store.list(&key).unwrap();
        assert_eq!(list.len(), 1);

        std::fs::remove_dir_all(&dir).ok();
    }
}
