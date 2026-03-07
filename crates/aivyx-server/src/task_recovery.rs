//! Task recovery on server startup.
//!
//! Scans the `TaskStore` for missions in non-terminal states (Planning,
//! Planned, Executing, Verifying) and marks interrupted ones as Failed.
//! This prevents stale "Running" missions from lingering after a crash.

use aivyx_core::Result;
use aivyx_crypto::MasterKey;
use aivyx_task::{TaskStatus, TaskStore};

/// Recover tasks that were interrupted by a server crash/restart.
///
/// Scans for non-terminal missions and marks them as Failed with a reason
/// indicating they were interrupted. Returns the count of recovered tasks.
pub fn recover_interrupted_tasks(store: &TaskStore, key: &MasterKey) -> Result<usize> {
    let all = store.list(key)?;
    let mut recovered = 0;

    for meta in &all {
        if !meta.status.is_terminal() {
            // Load full mission to update it
            if let Some(mut mission) = store.load(&meta.id, key)? {
                let prev_status = format!("{:?}", mission.status);
                mission.status = TaskStatus::Failed {
                    reason: format!("interrupted by server restart (was {prev_status})"),
                };
                mission.updated_at = chrono::Utc::now();
                store.save(&mission, key)?;
                recovered += 1;

                tracing::info!(
                    task_id = %meta.id,
                    goal = %meta.goal,
                    prev_status = %prev_status,
                    "recovered interrupted task"
                );
            }
        }
    }

    if recovered > 0 {
        tracing::info!(count = recovered, "recovered interrupted tasks on startup");
    }

    Ok(recovered)
}
