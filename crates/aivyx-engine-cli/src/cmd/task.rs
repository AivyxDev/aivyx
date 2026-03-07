//! CLI handlers for multi-step task (mission) management.

use std::sync::Arc;

use aivyx_agent::AgentSession;
use aivyx_audit::AuditLog;
use aivyx_config::{AivyxConfig, AivyxDirs};
use aivyx_core::{AivyxError, Result, TaskId};
use aivyx_crypto::{MasterKey, derive_audit_key, derive_task_key};
use aivyx_task::{Mission, ProgressEvent, TaskEngine, TaskStore};
use async_trait::async_trait;

use crate::channel::CliChannel;
use crate::output;

/// Run a new multi-step mission end-to-end.
pub async fn run(agent: &str, goal: &str) -> Result<()> {
    let (engine, _dirs) = build_engine()?;

    output::header("Planning mission");
    output::kv("Agent", agent);
    output::kv("Goal", goal);
    println!();

    let cli_progress = CliProgressSink;
    let cli_channel = CliChannel;

    let (token_tx, mut token_rx) = tokio::sync::mpsc::channel::<String>(64);

    let print_handle = tokio::spawn(async move {
        use std::io::Write;
        while let Some(token) = token_rx.recv().await {
            print!("{token}");
            std::io::stdout().flush().ok();
        }
    });

    let task_id = engine
        .create_mission(goal, agent, Some(&cli_progress))
        .await?;

    let mission = engine
        .execute_mission_stream(&task_id, Some(&cli_channel), Some(&cli_progress), token_tx)
        .await?;

    let _ = print_handle.await;
    println!();

    print_mission_summary(&mission);
    Ok(())
}

/// List all missions.
pub fn list() -> Result<()> {
    let (engine, _dirs) = build_engine()?;
    let missions = engine.list_missions()?;

    if missions.is_empty() {
        println!("  No tasks found.");
        return Ok(());
    }

    output::header("Tasks");
    println!();

    for meta in &missions {
        output::kv("  Task", &meta.id.to_string());
        output::kv("  Goal", &meta.goal);
        output::kv("  Agent", &meta.agent_name);
        output::kv("  Status", &format!("{:?}", meta.status));
        output::kv(
            "  Progress",
            &format!("{}/{} steps", meta.steps_completed, meta.steps_total),
        );
        output::kv(
            "  Updated",
            &meta.updated_at.format("%Y-%m-%d %H:%M:%S").to_string(),
        );
        println!();
    }

    Ok(())
}

/// Show details of a single mission.
pub fn show(id: &str) -> Result<()> {
    let task_id: TaskId = id
        .parse()
        .map_err(|_| AivyxError::Config(format!("invalid task ID: {id}")))?;

    let (engine, _dirs) = build_engine()?;
    let mission = engine
        .get_mission(&task_id)?
        .ok_or_else(|| AivyxError::Task(format!("task not found: {id}")))?;

    output::header("Task Details");
    println!();
    output::kv("ID", &mission.id.to_string());
    output::kv("Goal", &mission.goal);
    output::kv("Agent", &mission.agent_name);
    output::kv("Status", &format!("{:?}", mission.status));
    output::kv(
        "Created",
        &mission.created_at.format("%Y-%m-%d %H:%M:%S").to_string(),
    );
    output::kv(
        "Updated",
        &mission.updated_at.format("%Y-%m-%d %H:%M:%S").to_string(),
    );
    println!();

    output::header("Steps");
    println!();
    for step in &mission.steps {
        output::kv(&format!("  Step {}", step.index + 1), &step.description);
        output::kv("    Status", &format!("{:?}", step.status));
        if let Some(ref result) = step.result {
            let preview = if result.len() > 100 {
                format!("{}...", &result[..result.floor_char_boundary(100)])
            } else {
                result.clone()
            };
            output::kv("    Result", &preview);
        }
    }

    Ok(())
}

/// Resume an interrupted mission.
pub async fn resume(id: &str) -> Result<()> {
    let task_id: TaskId = id
        .parse()
        .map_err(|_| AivyxError::Config(format!("invalid task ID: {id}")))?;

    let (engine, _dirs) = build_engine()?;
    let cli_progress = CliProgressSink;
    let cli_channel = CliChannel;

    output::header(&format!("Resuming task {id}"));
    println!();

    let mission = engine
        .resume(&task_id, Some(&cli_channel), Some(&cli_progress))
        .await?;

    print_mission_summary(&mission);
    Ok(())
}

/// Cancel a running mission.
pub fn cancel(id: &str) -> Result<()> {
    let task_id: TaskId = id
        .parse()
        .map_err(|_| AivyxError::Config(format!("invalid task ID: {id}")))?;

    let (engine, _dirs) = build_engine()?;
    engine.cancel(&task_id)?;

    output::success(&format!("Cancelled task {id}"));
    Ok(())
}

/// Delete a completed/failed/cancelled mission.
pub fn delete(id: &str) -> Result<()> {
    let task_id: TaskId = id
        .parse()
        .map_err(|_| AivyxError::Config(format!("invalid task ID: {id}")))?;

    let (engine, _dirs) = build_engine()?;
    engine.delete_mission(&task_id)?;

    output::success(&format!("Deleted task {id}"));
    Ok(())
}

/// CLI progress sink that prints events to stdout.
struct CliProgressSink;

#[async_trait]
impl aivyx_core::ProgressSink<ProgressEvent> for CliProgressSink {
    async fn emit(&self, event: ProgressEvent) -> Result<()> {
        match event {
            ProgressEvent::Planned { task_id, steps, .. } => {
                output::success(&format!("Mission {task_id} planned with {steps} steps"));
                println!();
            }
            ProgressEvent::StepStarted {
                step_index,
                step_description,
                ..
            } => {
                output::header(&format!("Step {} — {}", step_index + 1, step_description));
                println!();
            }
            ProgressEvent::StepCompleted {
                step_index,
                success,
                ..
            } => {
                if success {
                    output::success(&format!("Step {} completed", step_index + 1));
                } else {
                    output::error(&format!("Step {} failed", step_index + 1));
                }
                println!();
            }
            ProgressEvent::MissionCompleted { success, .. } => {
                println!();
                if success {
                    output::success("Mission completed successfully");
                } else {
                    output::error("Mission failed");
                }
            }
            ProgressEvent::Resumed { from_step, .. } => {
                output::kv("Resuming from step", &format!("{}", from_step + 1));
                println!();
            }
        }
        Ok(())
    }
}

/// Print a summary of a completed mission.
fn print_mission_summary(mission: &Mission) {
    println!();
    output::kv("Status", &format!("{:?}", mission.status));
    output::kv(
        "Steps",
        &format!(
            "{}/{} completed",
            mission.steps_completed(),
            mission.steps.len()
        ),
    );
    println!();
}

/// Construct a TaskEngine from the user's local configuration.
fn build_engine() -> Result<(TaskEngine, AivyxDirs)> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let master_key = unlock_master_key(&dirs)?;
    let config = AivyxConfig::load(dirs.config_path())?;

    let task_key = derive_task_key(&master_key);
    let audit_key = derive_audit_key(&master_key);
    let audit_log = AuditLog::new(dirs.audit_path(), &audit_key);

    let store = TaskStore::open(dirs.tasks_dir().join("tasks.db"))?;

    let agent_dirs = AivyxDirs::new(dirs.root());
    let session = Arc::new(AgentSession::new(agent_dirs, config, master_key));

    let engine = TaskEngine::new(session, store, task_key, Some(audit_log));
    Ok((engine, dirs))
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
