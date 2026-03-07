//! Progress events emitted during mission execution.
//!
//! The [`aivyx_core::ProgressSink`] trait allows consumers
//! (CLI, TUI, server) to observe mission lifecycle events.
//! [`ChannelProgressSink`] sends events through a `tokio::sync::mpsc` channel.

use aivyx_core::TaskId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Channel-backed progress sink specialized to [`ProgressEvent`].
pub type ChannelProgressSink = aivyx_core::ChannelProgressSink<ProgressEvent>;

/// Events emitted during mission lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ProgressEvent {
    /// Mission plan created with N steps.
    Planned {
        task_id: TaskId,
        steps: usize,
        timestamp: DateTime<Utc>,
    },
    /// A step started executing.
    StepStarted {
        task_id: TaskId,
        step_index: usize,
        step_description: String,
        timestamp: DateTime<Utc>,
    },
    /// A step completed (success or failure).
    StepCompleted {
        task_id: TaskId,
        step_index: usize,
        success: bool,
        result_summary: String,
        timestamp: DateTime<Utc>,
    },
    /// Mission completed or failed.
    MissionCompleted {
        task_id: TaskId,
        success: bool,
        timestamp: DateTime<Utc>,
    },
    /// Mission was resumed from checkpoint.
    Resumed {
        task_id: TaskId,
        from_step: usize,
        timestamp: DateTime<Utc>,
    },
    /// An approval gate was reached and is waiting for user input.
    ApprovalRequested {
        task_id: TaskId,
        step_index: usize,
        context: String,
        timeout_secs: Option<u64>,
        timestamp: DateTime<Utc>,
    },
    /// An approval gate received a response.
    ApprovalReceived {
        task_id: TaskId,
        step_index: usize,
        approved: bool,
        timestamp: DateTime<Utc>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_event_serde_roundtrip() {
        let events = vec![
            ProgressEvent::Planned {
                task_id: TaskId::new(),
                steps: 5,
                timestamp: Utc::now(),
            },
            ProgressEvent::StepStarted {
                task_id: TaskId::new(),
                step_index: 0,
                step_description: "Search for info".into(),
                timestamp: Utc::now(),
            },
            ProgressEvent::StepCompleted {
                task_id: TaskId::new(),
                step_index: 0,
                success: true,
                result_summary: "Found 5 results".into(),
                timestamp: Utc::now(),
            },
            ProgressEvent::MissionCompleted {
                task_id: TaskId::new(),
                success: true,
                timestamp: Utc::now(),
            },
            ProgressEvent::Resumed {
                task_id: TaskId::new(),
                from_step: 2,
                timestamp: Utc::now(),
            },
        ];

        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            let restored: ProgressEvent = serde_json::from_str(&json).unwrap();
            // Verify type tag is present
            assert!(json.contains("\"type\":"));
            // Verify it deserializes without error
            let _ = restored;
        }
    }

    #[tokio::test]
    async fn channel_progress_sink_sends_events() {
        use aivyx_core::ProgressSink;

        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let sink = aivyx_core::ChannelProgressSink::new(tx);

        sink.emit(ProgressEvent::Planned {
            task_id: TaskId::new(),
            steps: 3,
            timestamp: Utc::now(),
        })
        .await
        .unwrap();

        let received = rx.recv().await.unwrap();
        assert!(matches!(received, ProgressEvent::Planned { steps: 3, .. }));
    }
}
