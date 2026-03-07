//! CLI handlers for channel management.

use aivyx_config::{AivyxConfig, AivyxDirs, ChannelConfig, ChannelPlatform};
use aivyx_core::{AivyxError, Result};
use aivyx_crypto::{EncryptedStore, MasterKey};
use dialoguer::{Input, Select};

use crate::output;

/// List all configured channels.
pub fn list() -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let config = AivyxConfig::load(dirs.config_path())?;

    if config.channels.is_empty() {
        println!("  No channels configured.");
        println!("  Use `aivyx channel add` to create one.");
        return Ok(());
    }

    output::header("Channels");
    println!();
    println!(
        "  {:<20} {:<12} {:<16} {:<8} ALLOWED USERS",
        "NAME", "PLATFORM", "AGENT", "ENABLED"
    );
    for ch in &config.channels {
        let enabled = if ch.enabled { "yes" } else { "no" };
        let users = if ch.allowed_users.is_empty() {
            "(none)".to_string()
        } else {
            ch.allowed_users.join(", ")
        };
        println!(
            "  {:<20} {:<12} {:<16} {:<8} {}",
            ch.name, ch.platform, ch.agent, enabled, users
        );
    }
    println!();

    Ok(())
}

/// Interactive wizard to add a new channel.
pub fn add() -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    // 1. Channel name
    let name: String = Input::new()
        .with_prompt("  Channel name (slug-style, e.g., telegram-personal)")
        .interact_text()
        .map_err(|e| AivyxError::Other(format!("input error: {e}")))?;

    if name.is_empty() {
        return Err(AivyxError::Config("channel name cannot be empty".into()));
    }

    // 2. Platform selection
    let platforms = &["Telegram", "Email"];
    let platform_idx = Select::new()
        .with_prompt("  Platform")
        .items(platforms)
        .default(0)
        .interact()
        .map_err(|e| AivyxError::Other(format!("selection error: {e}")))?;

    let platform = match platform_idx {
        0 => ChannelPlatform::Telegram,
        1 => ChannelPlatform::Email,
        _ => return Err(AivyxError::Config("invalid platform selection".into())),
    };

    // 3. Agent profile name
    let agent: String = Input::new()
        .with_prompt("  Agent profile name")
        .default("assistant".into())
        .interact_text()
        .map_err(|e| AivyxError::Other(format!("input error: {e}")))?;

    // 4. Platform-specific settings
    let mut channel = ChannelConfig::new(&name, platform, &agent);

    match platform {
        ChannelPlatform::Telegram => {
            let bot_token_ref: String = Input::new()
                .with_prompt("  Bot token secret name (key in encrypted store)")
                .default("tg-bot-token".into())
                .interact_text()
                .map_err(|e| AivyxError::Other(format!("input error: {e}")))?;
            channel
                .settings
                .insert("bot_token_ref".into(), bot_token_ref);
        }
        ChannelPlatform::Email => {
            let imap_host: String = Input::new()
                .with_prompt("  IMAP host")
                .interact_text()
                .map_err(|e| AivyxError::Other(format!("input error: {e}")))?;
            let smtp_host: String = Input::new()
                .with_prompt("  SMTP host")
                .interact_text()
                .map_err(|e| AivyxError::Other(format!("input error: {e}")))?;
            let username: String = Input::new()
                .with_prompt("  Email username")
                .interact_text()
                .map_err(|e| AivyxError::Other(format!("input error: {e}")))?;
            let password_ref: String = Input::new()
                .with_prompt("  Password secret name (key in encrypted store)")
                .default("email-password".into())
                .interact_text()
                .map_err(|e| AivyxError::Other(format!("input error: {e}")))?;

            channel.settings.insert("imap_host".into(), imap_host);
            channel.settings.insert("smtp_host".into(), smtp_host);
            channel.settings.insert("username".into(), username);
            channel.settings.insert("password_ref".into(), password_ref);
        }
        _ => {}
    }

    // 5. Allowed users
    let allowed_users_str: String = Input::new()
        .with_prompt("  Allowed users (comma-separated, empty to deny all)")
        .default(String::new())
        .interact_text()
        .map_err(|e| AivyxError::Other(format!("input error: {e}")))?;

    if !allowed_users_str.is_empty() {
        channel.allowed_users = allowed_users_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }

    // Save to config
    let mut config = AivyxConfig::load(dirs.config_path())?;
    config.add_channel(channel)?;
    config.save(dirs.config_path())?;

    output::success(&format!("added channel: {name}"));
    output::kv("Platform", &platform.to_string());
    output::kv("Agent", &agent);
    println!();

    Ok(())
}

/// Remove a channel by name.
pub fn remove(name: &str) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let mut config = AivyxConfig::load(dirs.config_path())?;
    config.remove_channel(name)?;
    config.save(dirs.config_path())?;

    output::success(&format!("removed channel: {name}"));
    println!();

    Ok(())
}

/// Test a channel connection by name.
pub async fn test(name: &str) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let config = AivyxConfig::load(dirs.config_path())?;

    let channel = config
        .find_channel(name)
        .ok_or_else(|| AivyxError::Config(format!("channel not found: {name}")))?;

    output::header(&format!("Testing channel: {name}"));
    output::kv("Platform", &channel.platform.to_string());
    output::kv("Agent", &channel.agent);
    println!();

    match channel.platform {
        ChannelPlatform::Telegram => {
            test_telegram(channel, &dirs).await?;
        }
        _ => {
            println!(
                "  Connection testing for {} is not yet supported.",
                channel.platform
            );
        }
    }

    println!();
    Ok(())
}

/// Test a Telegram bot connection by calling the `getMe` API endpoint.
async fn test_telegram(channel: &ChannelConfig, dirs: &AivyxDirs) -> Result<()> {
    let bot_token_ref = channel
        .settings
        .get("bot_token_ref")
        .ok_or_else(|| AivyxError::Config("missing bot_token_ref setting".into()))?;

    let master_key = unlock_master_key(dirs)?;
    let store = EncryptedStore::open(dirs.store_path())?;

    let token_bytes = store.get(bot_token_ref, &master_key)?.ok_or_else(|| {
        AivyxError::Config(format!(
            "secret '{bot_token_ref}' not found in encrypted store"
        ))
    })?;

    let token = String::from_utf8(token_bytes)
        .map_err(|e| AivyxError::Config(format!("invalid UTF-8 in bot token: {e}")))?;

    println!("  Calling Telegram getMe API...");

    let url = format!("https://api.telegram.org/bot{}/getMe", token.trim());
    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| AivyxError::Http(format!("Telegram API request failed: {e}")))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| AivyxError::Http(format!("failed to read response body: {e}")))?;

    if status.is_success() {
        // Parse the bot username from the response
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body)
            && let Some(result) = json.get("result")
        {
            let bot_name = result
                .get("first_name")
                .and_then(|v| v.as_str())
                .unwrap_or("(unknown)");
            let bot_username = result
                .get("username")
                .and_then(|v| v.as_str())
                .unwrap_or("(unknown)");
            output::success("Telegram bot connection successful");
            output::kv("Bot name", bot_name);
            output::kv("Bot username", &format!("@{bot_username}"));
            return Ok(());
        }
        output::success("Telegram API responded OK");
    } else {
        output::error(&format!("Telegram API returned HTTP {status}"));
        output::error(&format!("Response: {body}"));
    }

    Ok(())
}

/// Verify that `~/.aivyx/` is initialized.
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
