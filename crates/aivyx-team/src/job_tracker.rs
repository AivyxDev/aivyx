//! Async job tracking for parallel specialist dispatch.
//!
//! [`JobTracker`] manages the lifecycle of specialist jobs spawned by
//! [`DelegateTaskTool`](crate::delegation::DelegateTaskTool) in async mode.
//! Each job transitions through `Pending → Running → Completed | Failed`.
//!
//! Jobs track intermediate [`JobProgress`] events so the coordinator can
//! monitor long-running specialist work via [`CheckJobStatusTool`](crate::delegation::CheckJobStatusTool).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

/// Status of an asynchronous specialist job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobStatus {
    /// Job has been created but not yet started.
    Pending,
    /// Job is currently executing.
    Running,
    /// Job completed successfully.
    Completed,
    /// Job failed with an error.
    Failed,
}

/// An intermediate progress update for a running job.
///
/// Progress events are recorded via [`JobTracker::update_progress()`] and
/// read by [`CheckJobStatusTool`](crate::delegation::CheckJobStatusTool)
/// to give the coordinator visibility into long-running specialist work.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobProgress {
    /// Human-readable status message describing what the specialist is doing.
    pub message: String,
    /// When this progress event was recorded.
    pub timestamp: DateTime<Utc>,
}

/// A tracked specialist job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    /// Unique identifier for this job (UUID v4 string).
    pub id: String,
    /// Name of the specialist agent running this job.
    pub agent_name: String,
    /// Description of the delegated task.
    pub task: String,
    /// Current status of the job.
    pub status: JobStatus,
    /// Result text on success.
    pub result: Option<String>,
    /// Error message on failure.
    pub error: Option<String>,
    /// Intermediate progress events reported during execution.
    pub progress: Vec<JobProgress>,
    /// When the job was created.
    pub created_at: DateTime<Utc>,
    /// When the job finished (completed or failed). `None` while running.
    pub finished_at: Option<DateTime<Utc>>,
}

/// Thread-safe tracker for async specialist jobs.
///
/// Shared between the [`DelegateTaskTool`](crate::delegation::DelegateTaskTool),
/// [`CheckJobStatusTool`](crate::delegation::CheckJobStatusTool), and
/// [`CollectResultsTool`](crate::delegation::CollectResultsTool) via `Arc` cloning.
#[derive(Clone)]
pub struct JobTracker {
    jobs: Arc<Mutex<HashMap<String, Job>>>,
}

impl JobTracker {
    /// Create a new empty job tracker.
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register a new job as running and return its ID.
    pub async fn spawn_job(&self, agent_name: &str, task: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let job = Job {
            id: id.clone(),
            agent_name: agent_name.to_string(),
            task: task.to_string(),
            status: JobStatus::Running,
            result: None,
            error: None,
            progress: Vec::new(),
            created_at: Utc::now(),
            finished_at: None,
        };
        self.jobs.lock().await.insert(id.clone(), job);
        id
    }

    /// Get a snapshot of a specific job.
    pub async fn get_job(&self, id: &str) -> Option<Job> {
        self.jobs.lock().await.get(id).cloned()
    }

    /// Get snapshots of all tracked jobs.
    pub async fn list_jobs(&self) -> Vec<Job> {
        self.jobs.lock().await.values().cloned().collect()
    }

    /// Mark a job as completed with its result.
    pub async fn complete_job(&self, id: &str, result: String) {
        if let Some(job) = self.jobs.lock().await.get_mut(id) {
            job.status = JobStatus::Completed;
            job.result = Some(result);
            job.finished_at = Some(Utc::now());
        }
    }

    /// Mark a job as failed with an error message.
    pub async fn fail_job(&self, id: &str, error: String) {
        if let Some(job) = self.jobs.lock().await.get_mut(id) {
            job.status = JobStatus::Failed;
            job.error = Some(error);
            job.finished_at = Some(Utc::now());
        }
    }

    /// Record an intermediate progress event for a running job.
    ///
    /// Progress events are timestamped and appended to the job's progress
    /// log. The coordinator reads these via `check_job_status` with the
    /// `since` parameter to avoid re-fetching old events.
    pub async fn update_progress(&self, id: &str, message: String) {
        if let Some(job) = self.jobs.lock().await.get_mut(id) {
            job.progress.push(JobProgress {
                message,
                timestamp: Utc::now(),
            });
        }
    }

    /// Get progress events for a job since a given index.
    ///
    /// Returns `(events_since_index, total_event_count)`, or `None` if the
    /// job does not exist. The `since` index is clamped to the total count.
    pub async fn get_progress(&self, id: &str, since: usize) -> Option<(Vec<JobProgress>, usize)> {
        let jobs = self.jobs.lock().await;
        jobs.get(id).map(|j| {
            let total = j.progress.len();
            let start = since.min(total);
            let events = j.progress[start..].to_vec();
            (events, total)
        })
    }

    /// Check whether all tracked jobs have finished (completed or failed).
    pub async fn all_completed(&self) -> bool {
        let jobs = self.jobs.lock().await;
        if jobs.is_empty() {
            return true;
        }
        jobs.values()
            .all(|j| j.status == JobStatus::Completed || j.status == JobStatus::Failed)
    }

    /// Wait until all tracked jobs have finished, polling at the given interval.
    ///
    /// Returns when all jobs are `Completed` or `Failed`, or when the timeout
    /// elapses (returns `false` on timeout).
    pub async fn wait_all(&self, poll_interval: Duration, timeout: Duration) -> bool {
        let start = tokio::time::Instant::now();
        let mut interval = tokio::time::interval(poll_interval);

        loop {
            interval.tick().await;
            if self.all_completed().await {
                return true;
            }
            if start.elapsed() >= timeout {
                return false;
            }
        }
    }
}

impl Default for JobTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn job_spawn_and_get() {
        let tracker = JobTracker::new();
        let id = tracker.spawn_job("researcher", "find docs").await;

        let job = tracker.get_job(&id).await.unwrap();
        assert_eq!(job.agent_name, "researcher");
        assert_eq!(job.task, "find docs");
        assert_eq!(job.status, JobStatus::Running);
        assert!(job.result.is_none());
    }

    #[tokio::test]
    async fn job_complete() {
        let tracker = JobTracker::new();
        let id = tracker.spawn_job("coder", "implement feature").await;

        tracker
            .complete_job(&id, "done, 42 lines".to_string())
            .await;

        let job = tracker.get_job(&id).await.unwrap();
        assert_eq!(job.status, JobStatus::Completed);
        assert_eq!(job.result.as_deref(), Some("done, 42 lines"));
        assert!(job.error.is_none());
    }

    #[tokio::test]
    async fn job_fail() {
        let tracker = JobTracker::new();
        let id = tracker.spawn_job("reviewer", "review code").await;

        tracker.fail_job(&id, "profile not found".to_string()).await;

        let job = tracker.get_job(&id).await.unwrap();
        assert_eq!(job.status, JobStatus::Failed);
        assert_eq!(job.error.as_deref(), Some("profile not found"));
    }

    #[tokio::test]
    async fn job_list_all() {
        let tracker = JobTracker::new();
        tracker.spawn_job("researcher", "task 1").await;
        tracker.spawn_job("coder", "task 2").await;
        tracker.spawn_job("writer", "task 3").await;

        let jobs = tracker.list_jobs().await;
        assert_eq!(jobs.len(), 3);
    }

    #[tokio::test]
    async fn job_all_completed() {
        let tracker = JobTracker::new();
        assert!(tracker.all_completed().await); // empty = true

        let id1 = tracker.spawn_job("a", "t1").await;
        let id2 = tracker.spawn_job("b", "t2").await;
        assert!(!tracker.all_completed().await);

        tracker.complete_job(&id1, "ok".to_string()).await;
        assert!(!tracker.all_completed().await);

        tracker.fail_job(&id2, "err".to_string()).await;
        assert!(tracker.all_completed().await);
    }

    #[tokio::test]
    async fn check_nonexistent_job_returns_none() {
        let tracker = JobTracker::new();
        let random_id = uuid::Uuid::new_v4().to_string();
        let result = tracker.get_job(&random_id).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn job_progress_recorded() {
        let tracker = JobTracker::new();
        let id = tracker.spawn_job("coder", "build feature").await;

        tracker
            .update_progress(&id, "Parsing requirements".to_string())
            .await;
        tracker
            .update_progress(&id, "Writing code".to_string())
            .await;

        let (events, total) = tracker.get_progress(&id, 0).await.unwrap();
        assert_eq!(total, 2);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].message, "Parsing requirements");
        assert_eq!(events[1].message, "Writing code");
    }

    #[tokio::test]
    async fn job_progress_since() {
        let tracker = JobTracker::new();
        let id = tracker.spawn_job("researcher", "search").await;

        tracker.update_progress(&id, "step 1".to_string()).await;
        tracker.update_progress(&id, "step 2".to_string()).await;
        tracker.update_progress(&id, "step 3".to_string()).await;

        // Get events since index 2 (should return only "step 3")
        let (events, total) = tracker.get_progress(&id, 2).await.unwrap();
        assert_eq!(total, 3);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].message, "step 3");

        // Since beyond total returns empty
        let (events, total) = tracker.get_progress(&id, 10).await.unwrap();
        assert_eq!(total, 3);
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn job_timestamps_set() {
        let tracker = JobTracker::new();
        let before = Utc::now();
        let id = tracker.spawn_job("analyst", "analyze").await;

        let job = tracker.get_job(&id).await.unwrap();
        assert!(job.created_at >= before);
        assert!(job.finished_at.is_none());

        tracker.complete_job(&id, "done".to_string()).await;
        let job = tracker.get_job(&id).await.unwrap();
        assert!(job.finished_at.is_some());
        assert!(job.finished_at.unwrap() >= job.created_at);
    }

    #[tokio::test]
    async fn job_fail_sets_finished_at() {
        let tracker = JobTracker::new();
        let id = tracker.spawn_job("coder", "task").await;

        tracker.fail_job(&id, "oops".to_string()).await;
        let job = tracker.get_job(&id).await.unwrap();
        assert!(job.finished_at.is_some());
    }

    #[tokio::test]
    async fn job_progress_nonexistent_returns_none() {
        let tracker = JobTracker::new();
        assert!(tracker.get_progress("fake-id", 0).await.is_none());
    }

    #[tokio::test]
    async fn update_progress_on_nonexistent_is_noop() {
        let tracker = JobTracker::new();
        // Should not panic
        tracker
            .update_progress("fake-id", "hello".to_string())
            .await;
    }
}
