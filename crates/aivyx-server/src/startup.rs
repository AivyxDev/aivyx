//! Server startup: router construction and `AppState` building.
//!
//! `build_router()` constructs the Axum router with all routes and middleware.
//! `build_app_state()` creates the shared state from configuration and secrets.

use std::sync::Arc;
use tokio::sync::Mutex;

use aivyx_agent::{AgentSession, SessionStore};
use aivyx_audit::AuditLog;
use aivyx_config::{AivyxConfig, AivyxDirs};
use aivyx_core::Result;
use aivyx_crypto::{EncryptedStore, MasterKey, derive_audit_key};
use sha2::{Digest, Sha256};

use crate::app_state::{AppState, EndpointRateLimiters};
use crate::middleware;
use crate::routes;

/// Constructs the Axum router with all routes and middleware.
///
/// The `/health` endpoint is outside the auth layer. All other routes
/// require Bearer token authentication via the auth middleware.
/// Security headers are applied to all responses.
pub fn build_router(state: Arc<AppState>) -> axum::Router {
    let sidecar_mode = state.sidecar_mode;

    // Public routes (no auth required — WebSocket authenticates in-band)
    let public = axum::Router::new()
        .route("/health", axum::routing::get(routes::health::health))
        .route("/ws", axum::routing::get(routes::ws::ws_handler))
        .route(
            "/metrics",
            axum::routing::get(routes::metrics::prometheus_metrics),
        );

    // --- Rate-limited tiers (inside auth layer) ---

    // LLM tier: endpoints that invoke LLM inference
    let llm_routes = axum::Router::new()
        .route("/chat", axum::routing::post(routes::chat::chat))
        .route(
            "/chat/stream",
            axum::routing::post(routes::chat::stream_chat),
        )
        .route(
            "/chat/audio",
            axum::routing::post(routes::chat::audio_chat)
                .layer(axum::extract::DefaultBodyLimit::max(10_485_760)), // 10 MiB for audio
        )
        .route(
            "/teams/{name}/run",
            axum::routing::post(routes::teams::run_team),
        )
        .route(
            "/teams/{name}/run/stream",
            axum::routing::post(routes::teams::stream_run_team),
        )
        .route(
            "/digest",
            axum::routing::post(routes::digest::generate_digest),
        )
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::rate_limit::rate_limit_llm,
        ));

    // Search tier: memory search and profile extraction
    let search_routes = axum::Router::new()
        .route(
            "/memory/search",
            axum::routing::post(routes::memory::search_memories),
        )
        .route(
            "/memory/profile/extract",
            axum::routing::post(routes::memory::extract_profile),
        )
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::rate_limit::rate_limit_search,
        ));

    // Task tier: task creation and resumption
    let task_routes = axum::Router::new()
        .route(
            "/tasks",
            axum::routing::get(routes::tasks::list_tasks).post(routes::tasks::create_task),
        )
        .route(
            "/tasks/{id}/resume",
            axum::routing::post(routes::tasks::resume_task),
        )
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::rate_limit::rate_limit_task,
        ));

    // Unmetered routes: all other protected endpoints
    let unmetered_routes = axum::Router::new()
        .route(
            "/agents",
            axum::routing::get(routes::agents::list_agents).post(routes::agents::create_agent),
        )
        .route(
            "/agents/{name}",
            axum::routing::get(routes::agents::get_agent)
                .put(routes::agents::update_agent)
                .delete(routes::agents::delete_agent),
        )
        .route(
            "/agents/{name}/duplicate",
            axum::routing::post(routes::agents::duplicate_agent),
        )
        .route(
            "/agents/{name}/capabilities",
            axum::routing::put(routes::agents::update_capabilities),
        )
        .route(
            "/agents/{name}/persona",
            axum::routing::get(routes::agents::get_persona).patch(routes::agents::patch_persona),
        )
        .route("/teams", axum::routing::get(routes::teams::list_teams))
        .route("/teams/{name}", axum::routing::get(routes::teams::get_team))
        .route(
            "/teams/{name}/sessions",
            axum::routing::get(routes::teams::list_team_sessions),
        )
        .route("/memory", axum::routing::get(routes::memory::list_memories))
        .route(
            "/memory/{id}",
            axum::routing::delete(routes::memory::delete_memory),
        )
        .route(
            "/memory/stats",
            axum::routing::get(routes::memory::memory_stats),
        )
        .route(
            "/memory/triples",
            axum::routing::get(routes::memory::list_triples),
        )
        .route(
            "/memory/profile",
            axum::routing::get(routes::memory::get_profile).put(routes::memory::update_profile),
        )
        .route("/audit", axum::routing::get(routes::audit::recent_audit))
        .route(
            "/audit/verify",
            axum::routing::post(routes::audit::verify_audit),
        )
        .route(
            "/audit/search",
            axum::routing::get(routes::audit::search_audit),
        )
        .route(
            "/sessions",
            axum::routing::get(routes::sessions::list_sessions),
        )
        .route(
            "/sessions/{id}",
            axum::routing::delete(routes::sessions::delete_session),
        )
        .route(
            "/projects",
            axum::routing::get(routes::projects::list_projects)
                .post(routes::projects::create_project),
        )
        .route(
            "/projects/{name}",
            axum::routing::delete(routes::projects::delete_project),
        )
        .route(
            "/tasks/{id}",
            axum::routing::get(routes::tasks::get_task).delete(routes::tasks::delete_task),
        )
        .route(
            "/tasks/{id}/cancel",
            axum::routing::post(routes::tasks::cancel_task),
        )
        .route(
            "/channels",
            axum::routing::get(routes::channels::list_channels)
                .post(routes::channels::create_channel),
        )
        .route(
            "/channels/{name}",
            axum::routing::get(routes::channels::get_channel)
                .put(routes::channels::update_channel)
                .delete(routes::channels::delete_channel),
        )
        .route(
            "/schedules",
            axum::routing::get(routes::schedules::list_schedules)
                .post(routes::schedules::create_schedule),
        )
        .route(
            "/schedules/{name}",
            axum::routing::delete(routes::schedules::delete_schedule),
        )
        .route(
            "/notifications",
            axum::routing::get(routes::schedules::list_notifications)
                .delete(routes::schedules::drain_notifications),
        )
        .route(
            "/notifications/history",
            axum::routing::get(routes::schedules::notification_history),
        )
        .route(
            "/notifications/{id}/rating",
            axum::routing::put(routes::schedules::rate_notification),
        )
        .route(
            "/plugins",
            axum::routing::get(routes::plugins::list_plugins).post(routes::plugins::install_plugin),
        )
        .route(
            "/plugins/{name}",
            axum::routing::delete(routes::plugins::remove_plugin),
        )
        .route(
            "/plugins/templates",
            axum::routing::get(routes::templates::list_templates),
        )
        .route("/skills", axum::routing::get(routes::skills::list_skills))
        .route(
            "/skills/{name}",
            axum::routing::get(routes::skills::get_skill).delete(routes::skills::delete_skill),
        )
        .route("/tools", axum::routing::get(routes::tools::list_tools))
        .route(
            "/config",
            axum::routing::get(routes::config::get_config).patch(routes::config::patch_config),
        )
        .route(
            "/secrets",
            axum::routing::get(routes::secrets::list_secrets).post(routes::secrets::set_secret),
        )
        .route(
            "/secrets/{name}",
            axum::routing::delete(routes::secrets::delete_secret),
        )
        .route("/status", axum::routing::get(routes::status::system_status))
        .route(
            "/metrics/summary",
            axum::routing::get(routes::metrics::metrics_summary),
        )
        .route(
            "/metrics/timeline",
            axum::routing::get(routes::metrics::metrics_timeline),
        )
        .route(
            "/admin/rotate-token",
            axum::routing::post(routes::admin::rotate_token),
        )
        // --- Federation endpoints ---
        .route(
            "/federation/ping",
            axum::routing::get(routes::federation::ping),
        )
        .route(
            "/federation/peers",
            axum::routing::get(routes::federation::list_peers),
        )
        .route(
            "/federation/peers/{id}/agents",
            axum::routing::get(routes::federation::peer_agents),
        )
        .route(
            "/federation/relay/chat",
            axum::routing::post(routes::federation::relay_chat),
        )
        .route(
            "/federation/relay/task",
            axum::routing::post(routes::federation::relay_task),
        )
        .route(
            "/federation/search",
            axum::routing::post(routes::federation::federated_search),
        );

    // Merge all protected tiers under the auth middleware
    let protected = llm_routes
        .merge(search_routes)
        .merge(task_routes)
        .merge(unmetered_routes)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::auth::auth_middleware,
        ));

    // Build CORS layer from config (sidecar mode allows localhost origins)
    let cors = build_cors_layer(&state.config, sidecar_mode);

    public
        .merge(protected)
        .layer(axum::extract::DefaultBodyLimit::max(1_048_576)) // 1 MiB default
        .layer(axum::middleware::from_fn(
            middleware::security::security_headers,
        ))
        .layer(
            tower_http::trace::TraceLayer::new_for_http()
                .make_span_with(|request: &axum::http::Request<_>| {
                    let request_id = uuid::Uuid::new_v4();
                    tracing::info_span!(
                        "http_request",
                        request_id = %request_id,
                        method = %request.method(),
                        path = %request.uri().path(),
                    )
                })
                .on_response(|response: &axum::http::Response<_>, latency: std::time::Duration, _span: &tracing::Span| {
                    let status = response.status().as_u16().to_string();
                    metrics::counter!("http_requests_total", "status" => status).increment(1);
                    metrics::histogram!("http_request_duration_seconds").record(latency.as_secs_f64());
                    tracing::info!(
                        status = %response.status().as_u16(),
                        latency_ms = %latency.as_millis(),
                        "response"
                    );
                }),
        )
        .layer(cors)
        .with_state(state)
}

/// Check if an origin is a valid sidecar origin (localhost or Tauri).
///
/// Uses exact host matching to prevent bypass via DNS rebinding attacks
/// like `http://localhost.evil.com`.
fn is_sidecar_origin(origin: &[u8]) -> bool {
    let s = match std::str::from_utf8(origin) {
        Ok(s) => s,
        Err(_) => return false,
    };

    // Tauri custom scheme
    if s.starts_with("tauri://") {
        return true;
    }

    // Parse scheme://host[:port]
    let rest = if let Some(r) = s.strip_prefix("http://") {
        r
    } else if let Some(r) = s.strip_prefix("https://") {
        r
    } else {
        return false;
    };

    // Extract host (before optional :port)
    let host = rest.split(':').next().unwrap_or(rest);
    host == "localhost" || host == "127.0.0.1"
}

/// Builds a CORS layer from the server config's `cors_origins`.
///
/// When `sidecar_mode` is true and no origins are explicitly configured,
/// allows requests from `localhost`, `127.0.0.1`, and `tauri://` origins.
/// This is safe because the sidecar server only listens on localhost and
/// the bearer token provides authentication.
fn build_cors_layer(config: &AivyxConfig, sidecar_mode: bool) -> tower_http::cors::CorsLayer {
    use tower_http::cors::{AllowOrigin, CorsLayer};

    // If origins are explicitly configured, use those regardless of mode
    if let Some(ref server) = config.server
        && !server.cors_origins.is_empty()
    {
        let origins: Vec<axum::http::HeaderValue> = server
            .cors_origins
            .iter()
            .filter_map(|o| o.parse().ok())
            .collect();
        return CorsLayer::new()
            .allow_origin(AllowOrigin::list(origins))
            .allow_methods([
                axum::http::Method::GET,
                axum::http::Method::POST,
                axum::http::Method::PUT,
                axum::http::Method::PATCH,
                axum::http::Method::DELETE,
            ])
            .allow_headers([
                axum::http::header::AUTHORIZATION,
                axum::http::header::CONTENT_TYPE,
            ]);
    }

    // In sidecar mode, allow localhost origins for Tauri webview.
    // Uses exact host matching to prevent bypass via domains like localhost.evil.com.
    if sidecar_mode {
        return CorsLayer::new()
            .allow_origin(AllowOrigin::predicate(|origin, _| {
                is_sidecar_origin(origin.as_bytes())
            }))
            .allow_methods([
                axum::http::Method::GET,
                axum::http::Method::POST,
                axum::http::Method::PUT,
                axum::http::Method::PATCH,
                axum::http::Method::DELETE,
            ])
            .allow_headers([
                axum::http::header::AUTHORIZATION,
                axum::http::header::CONTENT_TYPE,
            ]);
    }

    CorsLayer::new() // deny all cross-origin by default
}

/// Constructs `AppState` from configuration with a single master key.
///
/// **Warning**: Uses a zero-byte placeholder for `AppState::master_key` because
/// `MasterKey` is not `Clone` and the real key is consumed by `AgentSession`.
/// Prefer `build_app_state_with_keys()` in production. This function exists only
/// for test convenience.
#[cfg(test)]
pub fn build_app_state(
    dirs: AivyxDirs,
    config: AivyxConfig,
    master_key: MasterKey,
    bearer_token: &str,
) -> Result<Arc<AppState>> {
    let audit_key = derive_audit_key(&master_key);
    let audit_log = AuditLog::new(dirs.audit_path(), &audit_key);

    let session_store = SessionStore::open(dirs.sessions_dir().join("sessions.db"))?;

    // Hash the bearer token for constant-time comparison
    let mut hasher = Sha256::new();
    hasher.update(bearer_token.as_bytes());
    let bearer_token_hash: [u8; 32] = hasher.finalize().into();

    // Memory manager is optional — requires embedding config
    let memory_manager = build_memory_manager(&dirs, &config, &master_key)?;

    let agent_dirs = AivyxDirs::new(dirs.root());
    let agent_config = config.clone();

    let state = AppState {
        agent_session: Arc::new(AgentSession::new(agent_dirs, agent_config, master_key)),
        session_store,
        memory_manager,
        audit_log,
        master_key: MasterKey::from_bytes([0u8; 32]), // placeholder
        dirs,
        config,
        bearer_token_hash: tokio::sync::RwLock::new(bearer_token_hash),
        auth_rate_limiter: std::sync::Mutex::new(std::collections::HashMap::new()),
        sidecar_mode: false,
        endpoint_rate_limiters: None,
        federation: None,
        prometheus_handle: None,
    };

    Ok(Arc::new(state))
}

/// Constructs `AppState` with two separate master keys.
///
/// `agent_key` is consumed by `AgentSession`, `store_key` is kept for
/// `SessionStore` and `MemoryManager` operations in request handlers.
/// `sidecar_mode` enables localhost CORS for Tauri desktop integration.
pub fn build_app_state_with_keys(
    dirs: AivyxDirs,
    config: AivyxConfig,
    agent_key: MasterKey,
    store_key: MasterKey,
    bearer_token: &str,
    sidecar_mode: bool,
) -> Result<Arc<AppState>> {
    let audit_key = derive_audit_key(&store_key);
    let audit_log = AuditLog::new(dirs.audit_path(), &audit_key);

    let session_store = SessionStore::open(dirs.sessions_dir().join("sessions.db"))?;

    let mut hasher = Sha256::new();
    hasher.update(bearer_token.as_bytes());
    let bearer_token_hash: [u8; 32] = hasher.finalize().into();

    let memory_manager = build_memory_manager(&dirs, &config, &store_key)?;

    let agent_dirs = AivyxDirs::new(dirs.root());
    let agent_config = config.clone();

    // Build endpoint rate limiters if configured
    let endpoint_rate_limiters = config
        .server
        .as_ref()
        .and_then(|s| s.rate_limit.as_ref())
        .map(EndpointRateLimiters::from_config);

    // Install Prometheus metrics recorder
    let prometheus_handle = metrics_exporter_prometheus::PrometheusBuilder::new()
        .install_recorder()
        .ok();

    let state = AppState {
        agent_session: Arc::new(AgentSession::new(agent_dirs, agent_config, agent_key)),
        session_store,
        memory_manager,
        audit_log,
        master_key: store_key,
        dirs,
        config,
        bearer_token_hash: tokio::sync::RwLock::new(bearer_token_hash),
        auth_rate_limiter: std::sync::Mutex::new(std::collections::HashMap::new()),
        sidecar_mode,
        endpoint_rate_limiters,
        federation: None,
        prometheus_handle,
    };

    Ok(Arc::new(state))
}

/// Attempt to build a `MemoryManager` if embedding config is present.
fn build_memory_manager(
    dirs: &AivyxDirs,
    config: &AivyxConfig,
    master_key: &MasterKey,
) -> Result<Option<Arc<Mutex<aivyx_memory::MemoryManager>>>> {
    if let Some(ref embedding_config) = config.embedding {
        let memory_key = aivyx_crypto::derive_memory_key(master_key);
        let memory_store = aivyx_memory::MemoryStore::open(dirs.memory_dir().join("memory.db"))?;

        // Need an EncryptedStore for the embedding provider factory
        let enc_store = EncryptedStore::open(dirs.store_path())?;
        let provider: Arc<dyn aivyx_llm::EmbeddingProvider> = Arc::from(
            aivyx_llm::create_embedding_provider(embedding_config, &enc_store, master_key)?,
        );

        let mgr = aivyx_memory::MemoryManager::new(
            memory_store,
            provider,
            memory_key,
            config.memory.max_memories,
        )?;
        Ok(Some(Arc::new(Mutex::new(mgr))))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_token_hashing() {
        let mut hasher = Sha256::new();
        hasher.update(b"test-token");
        let expected: [u8; 32] = hasher.finalize().into();

        let mut hasher2 = Sha256::new();
        hasher2.update(b"test-token");
        let actual: [u8; 32] = hasher2.finalize().into();

        assert_eq!(expected, actual);
    }
}
