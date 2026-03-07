//! Encrypted persistence for team sessions.
//!
//! Stores lead and specialist conversation histories so that team runs
//! can be resumed across process restarts. Uses `EncryptedStore` with
//! key format `"team-session:{team_name}:{session_id}"`.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use aivyx_core::{AivyxError, Result};
use aivyx_crypto::{EncryptedStore, MasterKey};
use aivyx_llm::ChatMessage;

/// Persisted state of a team session, including lead and specialist conversations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedTeamSession {
    /// Session identifier.
    pub session_id: String,
    /// Team name this session belongs to.
    pub team_name: String,
    /// Lead agent conversation history.
    pub lead_conversation: Vec<ChatMessage>,
    /// Specialist conversation histories, keyed by role name.
    pub specialist_conversations: HashMap<String, Vec<ChatMessage>>,
    /// Summary of completed work (injected into context on resume).
    pub completed_work: Vec<String>,
    /// When the session was created (RFC 3339).
    pub created_at: String,
    /// When the session was last saved (RFC 3339).
    pub updated_at: String,
}

/// Summary metadata for listing team sessions without loading full conversations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamSessionMetadata {
    /// Session identifier.
    pub session_id: String,
    /// Team name.
    pub team_name: String,
    /// Number of lead conversation messages.
    pub lead_message_count: usize,
    /// Number of specialists with saved conversations.
    pub specialist_count: usize,
    /// Number of completed work entries.
    pub completed_work_count: usize,
    /// When the session was created (RFC 3339).
    pub created_at: String,
    /// When the session was last saved (RFC 3339).
    pub updated_at: String,
}

impl PersistedTeamSession {
    /// Extract metadata from a full session.
    pub fn metadata(&self) -> TeamSessionMetadata {
        TeamSessionMetadata {
            session_id: self.session_id.clone(),
            team_name: self.team_name.clone(),
            lead_message_count: self.lead_conversation.len(),
            specialist_count: self.specialist_conversations.len(),
            completed_work_count: self.completed_work.len(),
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
        }
    }
}

/// Encrypted store for team sessions backed by `redb`.
pub struct TeamSessionStore {
    store: EncryptedStore,
}

impl TeamSessionStore {
    /// Open or create a team session store at the given path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let store = EncryptedStore::open(path)?;
        Ok(Self { store })
    }

    /// Save a team session.
    pub fn save(&self, session: &PersistedTeamSession, key: &MasterKey) -> Result<()> {
        let store_key = Self::make_key(&session.team_name, &session.session_id);
        let data = serde_json::to_vec(session).map_err(AivyxError::Serialization)?;
        self.store.put(&store_key, &data, key)
    }

    /// Load a team session by team name and session ID.
    pub fn load(
        &self,
        team_name: &str,
        session_id: &str,
        key: &MasterKey,
    ) -> Result<Option<PersistedTeamSession>> {
        let store_key = Self::make_key(team_name, session_id);
        match self.store.get(&store_key, key)? {
            Some(data) => {
                let session: PersistedTeamSession = serde_json::from_slice(&data)?;
                Ok(Some(session))
            }
            None => Ok(None),
        }
    }

    /// List metadata for all sessions belonging to a team.
    pub fn list(&self, team_name: &str, key: &MasterKey) -> Result<Vec<TeamSessionMetadata>> {
        let prefix = format!("team-session:{team_name}:");
        let keys = self.store.list_keys()?;
        let mut sessions = Vec::new();

        for k in keys {
            if k.starts_with(&prefix)
                && let Ok(Some(data)) = self.store.get(&k, key)
                && let Ok(session) = serde_json::from_slice::<PersistedTeamSession>(&data)
            {
                sessions.push(session.metadata());
            }
        }

        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(sessions)
    }

    /// Delete a team session.
    pub fn delete(&self, team_name: &str, session_id: &str) -> Result<()> {
        let store_key = Self::make_key(team_name, session_id);
        self.store.delete(&store_key)
    }

    fn make_key(team_name: &str, session_id: &str) -> String {
        format!("team-session:{team_name}:{session_id}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (TeamSessionStore, MasterKey, std::path::PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("aivyx-team-session-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("team-sessions.db");
        let store = TeamSessionStore::open(&db_path).unwrap();
        let key = MasterKey::generate();
        (store, key, dir)
    }

    fn make_session(team_name: &str, session_id: &str, lead_msgs: usize) -> PersistedTeamSession {
        let mut lead_conversation = Vec::new();
        for i in 0..lead_msgs {
            if i % 2 == 0 {
                lead_conversation.push(ChatMessage::user(format!("msg {i}")));
            } else {
                lead_conversation.push(ChatMessage::assistant(format!("reply {i}")));
            }
        }

        let mut specialist_conversations = HashMap::new();
        specialist_conversations.insert(
            "researcher".to_string(),
            vec![
                ChatMessage::user("research this".to_string()),
                ChatMessage::assistant("findings: ...".to_string()),
            ],
        );

        PersistedTeamSession {
            session_id: session_id.to_string(),
            team_name: team_name.to_string(),
            lead_conversation,
            specialist_conversations,
            completed_work: vec!["Step 1: researched topic".to_string()],
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    #[test]
    fn save_load_roundtrip() {
        let (store, key, dir) = setup();
        let session = make_session("dev-team", "sess-1", 4);

        store.save(&session, &key).unwrap();
        let loaded = store.load("dev-team", "sess-1", &key).unwrap().unwrap();

        assert_eq!(loaded.session_id, "sess-1");
        assert_eq!(loaded.team_name, "dev-team");
        assert_eq!(loaded.lead_conversation.len(), 4);
        assert_eq!(loaded.specialist_conversations.len(), 1);
        assert!(loaded.specialist_conversations.contains_key("researcher"));
        assert_eq!(loaded.completed_work.len(), 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_nonexistent_returns_none() {
        let (store, key, dir) = setup();
        let result = store.load("dev-team", "no-such-session", &key).unwrap();
        assert!(result.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_returns_only_matching_team() {
        let (store, key, dir) = setup();
        let s1 = make_session("team-a", "sess-1", 2);
        let s2 = make_session("team-a", "sess-2", 4);
        let s3 = make_session("team-b", "sess-3", 6);

        store.save(&s1, &key).unwrap();
        store.save(&s2, &key).unwrap();
        store.save(&s3, &key).unwrap();

        let team_a = store.list("team-a", &key).unwrap();
        assert_eq!(team_a.len(), 2);
        for meta in &team_a {
            assert_eq!(meta.team_name, "team-a");
        }

        let team_b = store.list("team-b", &key).unwrap();
        assert_eq!(team_b.len(), 1);
        assert_eq!(team_b[0].session_id, "sess-3");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_removes_session() {
        let (store, key, dir) = setup();
        let session = make_session("dev-team", "sess-del", 2);

        store.save(&session, &key).unwrap();
        assert!(store.load("dev-team", "sess-del", &key).unwrap().is_some());

        store.delete("dev-team", "sess-del").unwrap();
        assert!(store.load("dev-team", "sess-del", &key).unwrap().is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn empty_specialists_is_valid() {
        let (store, key, dir) = setup();
        let session = PersistedTeamSession {
            session_id: "solo".to_string(),
            team_name: "solo-team".to_string(),
            lead_conversation: vec![ChatMessage::user("hello".to_string())],
            specialist_conversations: HashMap::new(),
            completed_work: Vec::new(),
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        };

        store.save(&session, &key).unwrap();
        let loaded = store.load("solo-team", "solo", &key).unwrap().unwrap();
        assert!(loaded.specialist_conversations.is_empty());
        assert!(loaded.completed_work.is_empty());
        assert_eq!(loaded.lead_conversation.len(), 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn metadata_extraction() {
        let session = make_session("dev-team", "sess-meta", 6);
        let meta = session.metadata();
        assert_eq!(meta.session_id, "sess-meta");
        assert_eq!(meta.team_name, "dev-team");
        assert_eq!(meta.lead_message_count, 6);
        assert_eq!(meta.specialist_count, 1);
        assert_eq!(meta.completed_work_count, 1);
    }

    #[test]
    fn serde_roundtrip() {
        let session = make_session("dev-team", "sess-serde", 4);
        let json = serde_json::to_string(&session).unwrap();
        let restored: PersistedTeamSession = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.session_id, session.session_id);
        assert_eq!(restored.team_name, session.team_name);
        assert_eq!(
            restored.lead_conversation.len(),
            session.lead_conversation.len()
        );
        assert_eq!(
            restored.specialist_conversations.len(),
            session.specialist_conversations.len()
        );
    }
}
