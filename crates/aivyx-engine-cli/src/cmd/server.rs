//! CLI commands for the HTTP server.
//!
//! `aivyx server start` — start the HTTP server.
//! `aivyx server token generate` — generate a new bearer token.
//! `aivyx server token show` — show the existing bearer token.

use std::io::BufRead;

use aivyx_config::{AivyxConfig, AivyxDirs};
use aivyx_core::{AivyxError, Result};
use aivyx_crypto::{EncryptedStore, MasterKey};
use aivyx_server::{build_app_state_with_keys, build_router};

use crate::output;

/// The key name for storing the bearer token in the encrypted store.
const BEARER_TOKEN_KEY: &str = "server-bearer-token";

/// Start the HTTP server.
///
/// When `json_startup` is true, emits a JSON line to stdout with the bound
/// port, PID, and bearer token. This is used by the Tauri desktop app to
/// discover the sidecar server.
///
/// When `stdin_passphrase` is true, reads the passphrase from stdin instead
/// of an interactive prompt. This is used for headless/sidecar startup.
pub async fn start(
    bind: Option<&str>,
    port: Option<u16>,
    json_startup: bool,
    stdin_passphrase: bool,
) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let config = AivyxConfig::load(dirs.config_path())?;
    let server_config = config.server.clone().unwrap_or_default();

    let bind_addr = bind.unwrap_or(&server_config.bind_address);
    let bind_port = port.unwrap_or(server_config.port);

    // If --stdin-passphrase, read from stdin and set env var so the
    // centralized unlock module picks it up without an interactive prompt.
    if stdin_passphrase && std::env::var("AIVYX_PASSPHRASE").is_err() {
        let stdin = std::io::stdin();
        let mut line = String::new();
        stdin
            .lock()
            .read_line(&mut line)
            .map_err(|e| AivyxError::Other(format!("failed to read passphrase from stdin: {e}")))?;
        let passphrase = line.trim_end_matches('\n').trim_end_matches('\r');
        // SAFETY: single-threaded at this point (before any spawned tasks).
        unsafe { std::env::set_var("AIVYX_PASSPHRASE", passphrase) };
    }

    // Unlock master key once, then derive a second key for AgentSession
    let master_key = crate::unlock::unlock_master_key(&dirs)?;
    let key_bytes: [u8; 32] = master_key
        .expose_secret()
        .try_into()
        .map_err(|_| AivyxError::Crypto("master key is not 32 bytes".into()))?;
    let agent_key = MasterKey::from_bytes(key_bytes);

    // Load or auto-generate bearer token
    let enc_store = EncryptedStore::open(dirs.store_path())?;
    let token = match enc_store.get(BEARER_TOKEN_KEY, &master_key)? {
        Some(token_bytes) => String::from_utf8(token_bytes)
            .map_err(|e| AivyxError::Crypto(format!("invalid bearer token encoding: {e}")))?,
        None => {
            // Auto-generate token on first server start (avoids needing
            // a separate `token generate` step which causes redb lock
            // contention in Docker environments).
            let mut bytes = [0u8; 32];
            use rand::RngCore;
            rand::thread_rng().fill_bytes(&mut bytes);
            let token = hex::encode(bytes);
            enc_store.put(BEARER_TOKEN_KEY, token.as_bytes(), &master_key)?;
            eprintln!("  [info] auto-generated bearer token");
            token
        }
    };
    // Drop the store before building app state (release redb lock)
    drop(enc_store);

    // Clean up legacy plaintext bearer-token file if it exists
    let token_path = dirs.root().join("bearer-token");
    if token_path.exists() {
        let _ = std::fs::remove_file(&token_path);
        eprintln!("  [info] removed legacy plaintext bearer-token file");
    }

    let sidecar_mode = json_startup;
    let state =
        build_app_state_with_keys(dirs, config, agent_key, master_key, &token, sidecar_mode)?;

    // Spawn background services
    let _channel_handle = aivyx_server::channels::spawn_channel_manager(state.clone());
    let _scheduler_handle = aivyx_server::scheduler::spawn_scheduler(state.clone());

    let router = build_router(state);

    // Bind first, then read the actual address (supports port 0)
    let addr = format!("{bind_addr}:{bind_port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| AivyxError::Other(format!("failed to bind to {addr}: {e}")))?;
    let actual_addr = listener
        .local_addr()
        .map_err(|e| AivyxError::Other(format!("failed to get local addr: {e}")))?;

    if json_startup {
        let startup = serde_json::json!({
            "event": "ready",
            "port": actual_addr.port(),
            "pid": std::process::id(),
            "bearer_token": token,
        });
        println!("{startup}");
    } else {
        output::header("Starting aivyx server");
        output::kv("Address", &actual_addr.to_string());
    }

    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await
    .map_err(|e| AivyxError::Other(format!("server error: {e}")))?;

    Ok(())
}

/// Generate a new bearer token and store it.
pub fn token_generate() -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let master_key = unlock_master_key(&dirs)?;
    let enc_store = EncryptedStore::open(dirs.store_path())?;

    // Generate 32 random bytes and hex-encode
    let mut bytes = [0u8; 32];
    use rand::RngCore;
    rand::thread_rng().fill_bytes(&mut bytes);
    let token = hex::encode(bytes);

    enc_store.put(BEARER_TOKEN_KEY, token.as_bytes(), &master_key)?;

    output::header("Bearer token generated");
    output::kv("Token", &token);
    println!("\n  Save this token — it will not be shown again.");

    Ok(())
}

/// Show the existing bearer token.
pub fn token_show() -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let master_key = unlock_master_key(&dirs)?;
    let enc_store = EncryptedStore::open(dirs.store_path())?;

    match enc_store.get(BEARER_TOKEN_KEY, &master_key)? {
        Some(token_bytes) => {
            let token = String::from_utf8(token_bytes)
                .map_err(|e| AivyxError::Crypto(format!("invalid token encoding: {e}")))?;
            output::kv("Token", &token);
        }
        None => {
            output::error("no bearer token configured. Run `aivyx server token generate` first.");
        }
    }

    Ok(())
}

fn check_initialized(dirs: &AivyxDirs) -> Result<()> {
    if !dirs.is_initialized() {
        output::error("aivyx is not initialized. Run `aivyx genesis` to get started.");
        return Err(AivyxError::NotInitialized(
            "run `aivyx genesis` first".into(),
        ));
    }
    Ok(())
}

/// Unlock the master key (delegates to centralized unlock module).
fn unlock_master_key(dirs: &AivyxDirs) -> Result<MasterKey> {
    crate::unlock::unlock_master_key(dirs)
}
