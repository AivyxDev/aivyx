//! Background scheduler runtime.
//!
//! `spawn_scheduler()` starts a background `tokio::spawn` loop that ticks
//! every 60 seconds, checking schedule entries from `AivyxConfig`. When a
//! cron expression matches, it runs the configured agent and optionally
//! stores the result as a notification.

use std::sync::Arc;

use aivyx_config::AivyxConfig;
use aivyx_crypto::{MasterKey, derive_schedule_key};
use aivyx_memory::{Notification, NotificationStore};
use chrono::Utc;
use tokio::task::JoinHandle;

use crate::app_state::AppState;

/// Spawn the background scheduler loop.
///
/// Returns a `JoinHandle` to the spawned task. The loop runs indefinitely,
/// ticking every 60 seconds to check for due schedules. Errors are logged
/// but do not crash the server.
pub fn spawn_scheduler(state: Arc<AppState>) -> JoinHandle<()> {
    tokio::spawn(async move {
        tracing::info!("scheduler started (60s tick interval)");

        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;

            if let Err(e) = tick(&state).await {
                tracing::warn!("scheduler tick error: {e}");
            }
        }
    })
}

/// Single scheduler tick: check all schedule entries and fire any that are due.
async fn tick(state: &AppState) -> aivyx_core::Result<()> {
    // Reload config each tick to pick up changes without restart
    let mut config = AivyxConfig::load(state.dirs.config_path())?;
    let now = Utc::now();

    for entry in &mut config.schedules {
        if !entry.enabled {
            continue;
        }

        // Check if the entry is due
        if !is_due(&entry.cron, entry.last_run_at, now) {
            continue;
        }

        let is_team = entry.team.is_some();
        tracing::info!(
            "scheduler firing: {} (agent: {}, team: {})",
            entry.name,
            entry.agent,
            is_team
        );

        // Audit: schedule fired
        let _ = state
            .audit_log
            .append(aivyx_audit::AuditEvent::ScheduleFired {
                schedule_name: entry.name.clone(),
                agent_name: entry.agent.clone(),
                timestamp: now,
            });

        // Run as team session or single agent
        let result = if let Some(ref team_name) = entry.team {
            run_schedule_team(state, team_name, &entry.agent, &entry.prompt).await
        } else {
            run_schedule_agent(state, &entry.agent, &entry.prompt).await
        };

        match &result {
            Ok(response) => {
                tracing::info!("schedule '{}' completed successfully", entry.name);

                // Store notification if configured
                if entry.notify
                    && let Err(e) = store_notification(state, &entry.name, response)
                {
                    tracing::warn!("failed to store notification for '{}': {e}", entry.name);
                }

                let _ = state
                    .audit_log
                    .append(aivyx_audit::AuditEvent::ScheduleCompleted {
                        schedule_name: entry.name.clone(),
                        success: true,
                        result_summary: truncate(response, 200),
                    });
            }
            Err(e) => {
                tracing::error!("schedule '{}' failed: {e}", entry.name);

                let _ = state
                    .audit_log
                    .append(aivyx_audit::AuditEvent::ScheduleCompleted {
                        schedule_name: entry.name.clone(),
                        success: false,
                        result_summary: e.to_string(),
                    });
            }
        }

        // Update last_run_at
        entry.last_run_at = Some(now);
    }

    // Save config to persist last_run_at updates
    config.save(state.dirs.config_path())?;
    Ok(())
}

/// Check if a cron expression is due based on last run time and current time.
///
/// A schedule is due if at least one cron match occurred between `last_run_at`
/// (or epoch if never run) and `now`.
fn is_due(
    cron_expr: &str,
    last_run_at: Option<chrono::DateTime<Utc>>,
    now: chrono::DateTime<Utc>,
) -> bool {
    let cron = match croner::Cron::new(cron_expr).parse() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("invalid cron expression '{cron_expr}': {e}");
            return false;
        }
    };

    // Check the window between last run (or 61s ago if first run) and now
    let from = last_run_at.unwrap_or_else(|| now - chrono::Duration::seconds(61));

    // Use croner's find_next_occurrence from `from` time
    // If next occurrence is before or at `now`, it's due
    cron.find_next_occurrence(&from, false)
        .is_ok_and(|next| next <= now)
}

/// Run a single agent turn and return the response text.
async fn run_schedule_agent(
    state: &AppState,
    agent_name: &str,
    prompt: &str,
) -> aivyx_core::Result<String> {
    let mut agent = state
        .agent_session
        .create_agent_with_context(agent_name, None)
        .await?;

    // Run a single turn with no interactive channel (background execution)
    agent.turn(prompt, None).await
}

/// Run a scheduled team session and return the lead agent's response.
async fn run_schedule_team(
    state: &AppState,
    team_name: &str,
    _agent_name: &str,
    prompt: &str,
) -> aivyx_core::Result<String> {
    let dirs = aivyx_config::AivyxDirs::new(state.dirs.root());
    let key_bytes: [u8; 32] = state.master_key.expose_secret()[..32]
        .try_into()
        .map_err(|_| aivyx_core::AivyxError::Crypto("invalid master key length".into()))?;
    let mk = MasterKey::from_bytes(key_bytes);
    let config = state.config.read().await.clone();
    let session = aivyx_agent::AgentSession::new(dirs, config, mk);
    let dirs2 = aivyx_config::AivyxDirs::new(state.dirs.root());
    let runtime = aivyx_team::TeamRuntime::load(team_name, &dirs2, session)?;
    runtime.run(prompt, None).await
}

/// Store an agent response as a notification.
fn store_notification(
    state: &AppState,
    schedule_name: &str,
    content: &str,
) -> aivyx_core::Result<()> {
    let key_bytes: [u8; 32] =
        state.master_key.expose_secret().try_into().map_err(|_| {
            aivyx_core::AivyxError::Crypto("master key byte length mismatch".into())
        })?;
    let schedule_key = derive_schedule_key(&MasterKey::from_bytes(key_bytes));

    let store = NotificationStore::open(state.dirs.schedules_dir().join("notifications.db"))?;
    let notification = Notification::new(schedule_name, content);

    // Audit
    let _ = state
        .audit_log
        .append(aivyx_audit::AuditEvent::NotificationStored {
            notification_id: notification.id.to_string(),
            source: schedule_name.to_string(),
        });

    store.push(&notification, &schedule_key)
}

/// Truncate a string to a maximum length, breaking at a char boundary.
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let boundary = s.floor_char_boundary(max_len);
        format!("{}...", &s[..boundary])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_due_first_run() {
        // A schedule that has never run should be due (within 61s window)
        let now = Utc::now();
        // "every minute" should always be due
        assert!(is_due("* * * * *", None, now));
    }

    #[test]
    fn is_due_recently_run() {
        let now = Utc::now();
        // Just ran — should not be due within the same minute
        assert!(!is_due("* * * * *", Some(now), now));
    }

    #[test]
    fn is_due_invalid_cron() {
        let now = Utc::now();
        assert!(!is_due("invalid cron", None, now));
    }

    #[test]
    fn truncate_short_string() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string() {
        let long = "a".repeat(300);
        let result = truncate(&long, 200);
        assert!(result.len() <= 204); // 200 + "..."
        assert!(result.ends_with("..."));
    }
}
