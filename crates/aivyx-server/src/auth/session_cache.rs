//! In-memory session cache for SSO sessions.
//!
//! Stores authenticated sessions keyed by session ID, with expiry tracking
//! and periodic cleanup of expired entries.

use std::collections::HashMap;
use std::sync::Mutex;

use aivyx_core::TenantId;
use aivyx_tenant::AivyxRole;
use chrono::{DateTime, Utc};

/// In-memory cache of authenticated SSO sessions.
///
/// Thread-safe via `Mutex<HashMap>`. Sessions are created after successful
/// OIDC token validation and looked up on subsequent requests to avoid
/// re-validating the JWT on every call.
pub struct SessionCache {
    sessions: Mutex<HashMap<String, SessionRecord>>,
}

/// A single authenticated session record.
#[derive(Debug, Clone)]
pub struct SessionRecord {
    /// Unique session identifier.
    pub session_id: String,
    /// The authenticated user's identity (OIDC `sub` claim).
    pub user_id: String,
    /// The RBAC role assigned to this session.
    pub role: AivyxRole,
    /// Optional tenant scope for multi-tenant sessions.
    pub tenant_id: Option<TenantId>,
    /// When this session expires.
    pub expires_at: DateTime<Utc>,
}

impl SessionCache {
    /// Create a new empty session cache.
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// Store a session record and return its session ID.
    pub fn create_session(&self, record: SessionRecord) -> String {
        let session_id = record.session_id.clone();
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.insert(session_id.clone(), record);
        session_id
    }

    /// Look up a session by ID. Returns `None` if not found or expired.
    pub fn get_session(&self, session_id: &str) -> Option<SessionRecord> {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.get(session_id).and_then(|record| {
            if record.expires_at > Utc::now() {
                Some(record.clone())
            } else {
                None
            }
        })
    }

    /// Remove a session by ID.
    pub fn destroy_session(&self, session_id: &str) {
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.remove(session_id);
    }

    /// Remove all expired sessions from the cache.
    pub fn cleanup_expired(&self) {
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let now = Utc::now();
        sessions.retain(|_, record| record.expires_at > now);
    }
}

impl Default for SessionCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn make_record(id: &str, expires_in_secs: i64) -> SessionRecord {
        SessionRecord {
            session_id: id.to_string(),
            user_id: format!("user-{id}"),
            role: AivyxRole::Operator,
            tenant_id: None,
            expires_at: Utc::now() + Duration::seconds(expires_in_secs),
        }
    }

    #[test]
    fn create_and_get_session() {
        let cache = SessionCache::new();
        let record = make_record("sess-1", 3600);
        let id = cache.create_session(record.clone());
        assert_eq!(id, "sess-1");

        let found = cache.get_session("sess-1").unwrap();
        assert_eq!(found.user_id, "user-sess-1");
        assert_eq!(found.role, AivyxRole::Operator);
    }

    #[test]
    fn get_nonexistent_session_returns_none() {
        let cache = SessionCache::new();
        assert!(cache.get_session("does-not-exist").is_none());
    }

    #[test]
    fn expired_session_returns_none() {
        let cache = SessionCache::new();
        let record = make_record("expired", -1); // already expired
        cache.create_session(record);
        assert!(cache.get_session("expired").is_none());
    }

    #[test]
    fn destroy_session() {
        let cache = SessionCache::new();
        cache.create_session(make_record("sess-2", 3600));
        assert!(cache.get_session("sess-2").is_some());

        cache.destroy_session("sess-2");
        assert!(cache.get_session("sess-2").is_none());
    }

    #[test]
    fn destroy_nonexistent_session_is_noop() {
        let cache = SessionCache::new();
        cache.destroy_session("nope"); // should not panic
    }

    #[test]
    fn cleanup_expired_removes_old_sessions() {
        let cache = SessionCache::new();
        cache.create_session(make_record("active", 3600));
        cache.create_session(make_record("expired-1", -1));
        cache.create_session(make_record("expired-2", -100));

        cache.cleanup_expired();

        assert!(cache.get_session("active").is_some());
        // Expired sessions should be gone from the map entirely
        let sessions = cache.sessions.lock().unwrap();
        assert_eq!(sessions.len(), 1);
        assert!(sessions.contains_key("active"));
    }

    #[test]
    fn session_with_tenant_id() {
        let cache = SessionCache::new();
        let tid = TenantId::new();
        let record = SessionRecord {
            session_id: "tenant-sess".into(),
            user_id: "alice".into(),
            role: AivyxRole::Admin,
            tenant_id: Some(tid),
            expires_at: Utc::now() + Duration::seconds(3600),
        };
        cache.create_session(record);

        let found = cache.get_session("tenant-sess").unwrap();
        assert_eq!(found.tenant_id, Some(tid));
        assert_eq!(found.role, AivyxRole::Admin);
    }
}
