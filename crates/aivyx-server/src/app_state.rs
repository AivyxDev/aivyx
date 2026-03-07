//! Shared application state for the HTTP server.
//!
//! `AppState` holds all infrastructure needed to handle requests: the agent
//! factory (`AgentSession`), session persistence, memory manager, audit log,
//! and the hashed bearer token for authentication. Wrapped in `Arc` for
//! Axum's `State` extractor.

use std::net::IpAddr;
use std::num::NonZeroU32;
use std::sync::Arc;
use tokio::sync::Mutex;

use aivyx_agent::{AgentSession, SessionStore};
use aivyx_audit::AuditLog;
use aivyx_config::server::RateLimitConfig;
use aivyx_config::{AivyxConfig, AivyxDirs};
use aivyx_crypto::MasterKey;
use aivyx_memory::MemoryManager;
use governor::clock::DefaultClock;
use governor::state::keyed::DashMapStateStore;
use governor::{Quota, RateLimiter};

/// Type alias for a per-IP keyed rate limiter.
pub type KeyedRateLimiter = RateLimiter<IpAddr, DashMapStateStore<IpAddr>, DefaultClock>;

/// Per-tier endpoint rate limiters.
///
/// Each tier controls a group of endpoints with different cost profiles.
/// Created from `RateLimitConfig` at server startup.
pub struct EndpointRateLimiters {
    /// LLM-calling endpoints: `/chat*`, `/teams/*/run*`, `/digest`.
    pub llm: Arc<KeyedRateLimiter>,
    /// Search endpoints: `/memory/search`, `/memory/profile/extract`.
    pub search: Arc<KeyedRateLimiter>,
    /// Task endpoints: `POST /tasks`, `/tasks/*/resume`.
    pub task: Arc<KeyedRateLimiter>,
}

impl EndpointRateLimiters {
    /// Build rate limiters from configuration.
    pub fn from_config(config: &RateLimitConfig) -> Self {
        Self {
            llm: Arc::new(build_keyed_limiter(
                config.llm.max_requests,
                config.llm.window_secs,
            )),
            search: Arc::new(build_keyed_limiter(
                config.search.max_requests,
                config.search.window_secs,
            )),
            task: Arc::new(build_keyed_limiter(
                config.task.max_requests,
                config.task.window_secs,
            )),
        }
    }
}

/// Construct a keyed GCRA rate limiter for the given quota.
fn build_keyed_limiter(max_requests: u32, window_secs: u64) -> KeyedRateLimiter {
    let max = NonZeroU32::new(max_requests).unwrap_or(NonZeroU32::new(1).unwrap());
    let quota = Quota::with_period(std::time::Duration::from_secs(window_secs))
        .expect("non-zero window")
        .allow_burst(max);
    RateLimiter::dashmap(quota)
}

/// Shared state for all HTTP handlers.
///
/// Constructed once at startup and shared via `Arc<AppState>` through Axum's
/// `State` extractor. Each `/chat` request creates a fresh `Agent` via
/// `AgentSession::create_agent()` — no per-session state lives here.
pub struct AppState {
    /// Factory for creating agents from profile names.
    pub agent_session: Arc<AgentSession>,
    /// Encrypted session persistence (redb-backed).
    pub session_store: SessionStore,
    /// Shared memory manager (embedding search + knowledge triples).
    pub memory_manager: Option<Arc<Mutex<MemoryManager>>>,
    /// HMAC-chained audit log.
    pub audit_log: AuditLog,
    /// Master encryption key (not `Clone` — shared via `Arc<AppState>`).
    pub master_key: MasterKey,
    /// File system paths (`~/.aivyx/`).
    pub dirs: AivyxDirs,
    /// System configuration.
    pub config: AivyxConfig,
    /// SHA-256 hash of the expected bearer token for constant-time comparison.
    /// Wrapped in `RwLock` to support runtime token rotation.
    pub bearer_token_hash: tokio::sync::RwLock<[u8; 32]>,
    /// Rate limiter for failed authentication attempts (per-IP).
    pub auth_rate_limiter:
        std::sync::Mutex<std::collections::HashMap<std::net::IpAddr, Vec<std::time::Instant>>>,
    /// Whether the server is running as a Tauri sidecar (enables localhost CORS).
    pub sidecar_mode: bool,
    /// Per-endpoint rate limiters (disabled when `None`).
    pub endpoint_rate_limiters: Option<EndpointRateLimiters>,
    /// Federation client for cross-instance agent communication.
    /// `None` when federation is not configured.
    pub federation: Option<Arc<aivyx_federation::client::FederationClient>>,
    /// Prometheus metrics handle for rendering /metrics endpoint.
    pub prometheus_handle: Option<metrics_exporter_prometheus::PrometheusHandle>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use aivyx_crypto::{MasterKey, derive_audit_key};

    #[test]
    fn app_state_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<AppState>();
    }

    #[test]
    fn app_state_construction() {
        let dir = std::env::temp_dir().join(format!("aivyx-state-test-{}", rand::random::<u64>()));
        std::fs::create_dir_all(dir.join("sessions")).unwrap();
        std::fs::create_dir_all(dir.join("agents")).unwrap();

        let dirs = AivyxDirs::new(&dir);
        let config = AivyxConfig::default();
        let master_key = MasterKey::from_bytes([42u8; 32]);
        let audit_key = derive_audit_key(&master_key);
        let audit_log = AuditLog::new(dir.join("audit.log"), &audit_key);
        let session_store = SessionStore::open(dir.join("sessions").join("sessions.db")).unwrap();

        let mk2 = MasterKey::from_bytes([42u8; 32]);
        let state = AppState {
            agent_session: Arc::new(AgentSession::new(dirs, config.clone(), mk2)),
            session_store,
            memory_manager: None,
            audit_log,
            master_key,
            dirs: AivyxDirs::new(&dir),
            config,
            bearer_token_hash: tokio::sync::RwLock::new([0u8; 32]),
            auth_rate_limiter: std::sync::Mutex::new(std::collections::HashMap::new()),
            sidecar_mode: false,
            endpoint_rate_limiters: None,
            federation: None,
            prometheus_handle: None,
        };

        assert_eq!(*state.bearer_token_hash.blocking_read(), [0u8; 32]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
