//! CLI handlers for schedule management.

use aivyx_agent::AgentSession;
use aivyx_config::{AivyxConfig, AivyxDirs, ScheduleEntry, validate_cron};
use aivyx_core::{AivyxError, Result};
use aivyx_crypto::{MasterKey, derive_audit_key, derive_schedule_key};
use aivyx_memory::{Notification, NotificationStore};

use crate::channel::CliChannel;
use crate::output;

/// List all configured schedules.
pub fn list() -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let config = AivyxConfig::load(dirs.config_path())?;

    if config.schedules.is_empty() {
        println!("  No schedules configured.");
        println!("  Use `aivyx schedule add` to create one.");
        return Ok(());
    }

    output::header("Schedules");
    println!();
    println!(
        "  {:<20} {:<16} {:<12} {:<8} LAST RUN",
        "NAME", "CRON", "AGENT", "ENABLED"
    );
    for entry in &config.schedules {
        let enabled = if entry.enabled { "yes" } else { "no" };
        let last_run = entry
            .last_run_at
            .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "never".into());
        println!(
            "  {:<20} {:<16} {:<12} {:<8} {}",
            entry.name, entry.cron, entry.agent, enabled, last_run
        );
    }
    println!();

    Ok(())
}

/// Add a new schedule entry.
pub fn add(name: &str, cron: &str, agent: &str, prompt: &str, no_notify: bool) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    // Validate cron expression before saving
    validate_cron(cron)?;

    let mut config = AivyxConfig::load(dirs.config_path())?;

    let mut entry = ScheduleEntry::new(name, cron, agent, prompt);
    if no_notify {
        entry.notify = false;
    }

    config.add_schedule(entry)?;
    config.save(dirs.config_path())?;

    // Audit log
    if let Ok(master_key) = unlock_master_key(&dirs) {
        let audit_key = derive_audit_key(&master_key);
        let audit_log = aivyx_audit::AuditLog::new(dirs.audit_path(), &audit_key);
        let _ = audit_log.append(aivyx_audit::AuditEvent::ScheduleFired {
            schedule_name: name.to_string(),
            agent_name: agent.to_string(),
            timestamp: chrono::Utc::now(),
        });
    }

    output::success(&format!("added schedule: {name}"));
    output::kv("Cron", cron);
    output::kv("Agent", agent);
    output::kv("Notify", &(!no_notify).to_string());
    println!();

    Ok(())
}

/// Remove a schedule entry by name.
pub fn remove(name: &str) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let mut config = AivyxConfig::load(dirs.config_path())?;
    config.remove_schedule(name)?;
    config.save(dirs.config_path())?;

    output::success(&format!("removed schedule: {name}"));
    println!();

    Ok(())
}

/// Enable a schedule entry.
pub fn enable(name: &str) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let mut config = AivyxConfig::load(dirs.config_path())?;

    let entry = config
        .find_schedule_mut(name)
        .ok_or_else(|| AivyxError::Scheduler(format!("schedule not found: {name}")))?;
    entry.enabled = true;
    config.save(dirs.config_path())?;

    output::success(&format!("enabled schedule: {name}"));
    println!();

    Ok(())
}

/// Disable a schedule entry.
pub fn disable(name: &str) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let mut config = AivyxConfig::load(dirs.config_path())?;

    let entry = config
        .find_schedule_mut(name)
        .ok_or_else(|| AivyxError::Scheduler(format!("schedule not found: {name}")))?;
    entry.enabled = false;
    config.save(dirs.config_path())?;

    output::success(&format!("disabled schedule: {name}"));
    println!();

    Ok(())
}

/// Run a schedule entry immediately (bypass cron timing).
pub async fn run_now(name: &str) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let master_key = unlock_master_key(&dirs)?;
    let config = AivyxConfig::load(dirs.config_path())?;

    let entry = config
        .find_schedule(name)
        .ok_or_else(|| AivyxError::Scheduler(format!("schedule not found: {name}")))?;

    output::header(&format!("Running schedule: {name}"));
    output::kv("Agent", &entry.agent);
    output::kv("Prompt", &entry.prompt);
    println!();

    let agent_name = entry.agent.clone();
    let prompt = entry.prompt.clone();
    let notify = entry.notify;

    // Save paths before moving dirs into session
    let schedules_dir = dirs.schedules_dir();
    let agent_dirs = AivyxDirs::new(dirs.root());
    let session = AgentSession::new(agent_dirs, config, master_key);
    let mut agent = session.create_agent_with_context(&agent_name, None).await?;

    let (token_tx, mut token_rx) = tokio::sync::mpsc::channel::<String>(64);
    let print_handle = tokio::spawn(async move {
        use std::io::Write;
        while let Some(token) = token_rx.recv().await {
            print!("{token}");
            std::io::stdout().flush().ok();
        }
        println!();
    });

    let cli_channel = CliChannel;
    let result = agent
        .turn_stream(&prompt, Some(&cli_channel), token_tx, None)
        .await?;
    let _ = print_handle.await;

    // Store as notification if configured
    if notify {
        let key_bytes: [u8; 32] = session
            .master_key()
            .expose_secret()
            .try_into()
            .map_err(|_| AivyxError::Crypto("master key byte length mismatch".into()))?;
        let schedule_key = derive_schedule_key(&MasterKey::from_bytes(key_bytes));
        let store = NotificationStore::open(schedules_dir.join("notifications.db"))?;
        let notification = Notification::new(name, &result);
        store.push(&notification, &schedule_key)?;
        println!();
        output::success("result stored as notification");
    }

    println!();
    output::kv(
        "Estimated cost",
        &format!("${:.4}", agent.current_cost_usd()),
    );
    println!();

    Ok(())
}

/// List pending notifications from background schedule activity.
pub fn list_notifications() -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let notif_db = dirs.schedules_dir().join("notifications.db");
    if !notif_db.exists() {
        println!("  No pending notifications.");
        return Ok(());
    }

    let master_key = unlock_master_key(&dirs)?;
    let schedule_key = derive_schedule_key(&master_key);
    let store = NotificationStore::open(notif_db)?;
    let notifications = store.list(&schedule_key)?;

    if notifications.is_empty() {
        println!("  No pending notifications.");
        return Ok(());
    }

    output::header(&format!("Pending notifications ({})", notifications.len()));
    println!();

    for (i, n) in notifications.iter().enumerate() {
        let ts = n.created_at.format("%Y-%m-%d %H:%M");
        let rating_badge = match n.rating {
            Some(ref r) => format!(" [{}]", r),
            None => String::new(),
        };
        println!("  {}. [{}] {}{}", i + 1, ts, n.source, rating_badge);
        println!("     {}", n.content);
        println!();
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

fn unlock_master_key(dirs: &AivyxDirs) -> Result<MasterKey> {
    crate::unlock::unlock_master_key(dirs)
}
