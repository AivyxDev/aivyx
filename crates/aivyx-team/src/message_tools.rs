use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::Mutex;

use aivyx_audit::{AuditEvent, AuditLog};
use aivyx_core::{AivyxError, CapabilityScope, Result, Tool, ToolId};

use crate::message_bus::{MessageBus, TeamMessage};

/// Tool that allows an agent to send a message to another team member.
///
/// Supports rate limiting (per-turn) and optional audit logging of
/// all messages sent through the bus.
pub struct SendMessageTool {
    id: ToolId,
    bus: Arc<MessageBus>,
    sender_name: String,
    /// Per-turn message counter (resets when a new tool instance is created).
    message_count: Arc<AtomicU32>,
    /// Maximum messages allowed per delegation turn (0 = unlimited).
    max_per_turn: u32,
    /// Optional audit log for recording peer messages.
    audit_log: Option<AuditLog>,
}

impl SendMessageTool {
    /// Create a new send-message tool.
    pub fn new(bus: Arc<MessageBus>, sender_name: String) -> Self {
        Self {
            id: ToolId::new(),
            bus,
            sender_name,
            message_count: Arc::new(AtomicU32::new(0)),
            max_per_turn: 0,
            audit_log: None,
        }
    }

    /// Set the maximum messages per turn. 0 means unlimited.
    pub fn with_max_per_turn(mut self, max: u32) -> Self {
        self.max_per_turn = max;
        self
    }

    /// Attach an audit log for recording sent messages.
    pub fn with_audit_log(mut self, log: AuditLog) -> Self {
        self.audit_log = Some(log);
        self
    }
}

#[async_trait]
impl Tool for SendMessageTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "send_message"
    }

    fn description(&self) -> &str {
        "Send a message to another team member by name."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "to": {
                    "type": "string",
                    "description": "Name of the team member to send to"
                },
                "content": {
                    "type": "string",
                    "description": "Message content"
                },
                "message_type": {
                    "type": "string",
                    "description": "Type of message (e.g., 'text', 'status', 'review_request', 'review_response')",
                    "default": "text"
                }
            },
            "required": ["to", "content"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("coordination".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let to = input["to"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("send_message: missing 'to'".into()))?;
        let content = input["content"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("send_message: missing 'content'".into()))?;
        let message_type = input["message_type"].as_str().unwrap_or("text");

        // Rate limiting: check before incrementing
        if self.max_per_turn > 0 {
            let count = self.message_count.fetch_add(1, Ordering::Relaxed);
            if count >= self.max_per_turn {
                return Ok(serde_json::json!({
                    "status": "rate_limited",
                    "error": format!(
                        "Message limit reached ({} per turn). Wait for next delegation.",
                        self.max_per_turn
                    )
                }));
            }
        }

        let msg = TeamMessage {
            from: self.sender_name.clone(),
            to: to.to_string(),
            content: content.to_string(),
            message_type: message_type.to_string(),
            timestamp: Utc::now(),
        };

        self.bus.send(msg)?;

        // Audit log if configured
        if let Some(log) = &self.audit_log {
            let _ = log.append(AuditEvent::TeamMessage {
                from: self.sender_name.clone(),
                to: to.to_string(),
            });
        }

        Ok(serde_json::json!({
            "status": "sent",
            "to": to
        }))
    }
}

/// Tool that allows an agent to read pending messages from its inbox.
///
/// Uses a local buffer to preserve unmatched messages across reads.
/// Messages are drained from the broadcast receiver into the buffer,
/// then filtered — only matched messages are returned, unmatched ones
/// stay in the buffer for future reads.
pub struct ReadMessagesTool {
    id: ToolId,
    receiver: Arc<Mutex<tokio::sync::broadcast::Receiver<TeamMessage>>>,
    /// Local buffer for messages drained from broadcast but not yet consumed.
    buffer: Arc<Mutex<Vec<TeamMessage>>>,
}

impl ReadMessagesTool {
    /// Create a new read-messages tool with a broadcast receiver.
    pub fn new(receiver: Arc<Mutex<tokio::sync::broadcast::Receiver<TeamMessage>>>) -> Self {
        Self {
            id: ToolId::new(),
            receiver,
            buffer: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl Tool for ReadMessagesTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "read_messages"
    }

    fn description(&self) -> &str {
        "Read pending messages from other team members. \
         Supports filtering by sender, message type, and time."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "from": {
                    "type": "string",
                    "description": "Filter by sender name (optional)"
                },
                "message_type": {
                    "type": "string",
                    "description": "Filter by message type e.g. 'review_request', 'review_response', 'text' (optional)"
                },
                "since": {
                    "type": "string",
                    "description": "ISO 8601 timestamp — only return messages newer than this (optional)"
                }
            },
            "required": []
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("coordination".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        // Parse optional filters
        let filter_from = input["from"].as_str();
        let filter_type = input["message_type"].as_str();
        let filter_since: Option<DateTime<Utc>> = input["since"]
            .as_str()
            .and_then(|s| s.parse::<DateTime<Utc>>().ok());

        // Step 1: Drain all new messages from broadcast receiver into buffer
        {
            let mut rx = self.receiver.lock().await;
            loop {
                match rx.try_recv() {
                    Ok(msg) => {
                        self.buffer.lock().await.push(msg);
                    }
                    Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) => {
                        tracing::warn!("ReadMessagesTool: missed {n} messages due to buffer lag");
                    }
                    Err(_) => break,
                }
            }
        }

        // Step 2: Partition buffer — matched messages go to output, unmatched stay
        let mut buf = self.buffer.lock().await;
        let mut matched = Vec::new();
        let mut remaining = Vec::new();

        for msg in buf.drain(..) {
            let passes_from = filter_from.is_none_or(|f| msg.from == f);
            let passes_type = filter_type.is_none_or(|t| msg.message_type == t);
            let passes_since = filter_since.is_none_or(|ts| msg.timestamp > ts);

            if passes_from && passes_type && passes_since {
                matched.push(msg);
            } else {
                remaining.push(msg);
            }
        }

        *buf = remaining;

        // Step 3: Serialize matched messages
        let messages: Vec<serde_json::Value> = matched
            .iter()
            .map(|msg| {
                serde_json::json!({
                    "from": msg.from,
                    "content": msg.content,
                    "message_type": msg.message_type,
                    "timestamp": msg.timestamp.to_rfc3339(),
                })
            })
            .collect();

        Ok(serde_json::json!({
            "messages": messages,
            "count": messages.len()
        }))
    }
}

/// Tool for structured peer review requests.
///
/// Sends a structured review request message via the message bus and returns
/// immediately. The coordinator must then delegate the peer a turn to process
/// the review. This two-phase design works because specialists only execute
/// when the coordinator delegates to them — they aren't running concurrently.
pub struct RequestPeerReviewTool {
    id: ToolId,
    bus: Arc<MessageBus>,
    sender_name: String,
    /// Optional audit log for recording review requests.
    audit_log: Option<AuditLog>,
}

impl RequestPeerReviewTool {
    /// Create a new peer review request tool.
    pub fn new(bus: Arc<MessageBus>, sender_name: String) -> Self {
        Self {
            id: ToolId::new(),
            bus,
            sender_name,
            audit_log: None,
        }
    }

    /// Attach an audit log for recording review requests.
    pub fn with_audit_log(mut self, log: AuditLog) -> Self {
        self.audit_log = Some(log);
        self
    }
}

#[async_trait]
impl Tool for RequestPeerReviewTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "request_peer_review"
    }

    fn description(&self) -> &str {
        "Request structured feedback from a peer specialist. \
         Sends a review request and returns immediately — the peer must be \
         delegated a turn to process the review."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "peer": {
                    "type": "string",
                    "description": "Name of the peer to request review from"
                },
                "content": {
                    "type": "string",
                    "description": "The content to be reviewed"
                },
                "context": {
                    "type": "string",
                    "description": "Additional context or criteria for the review (optional)"
                }
            },
            "required": ["peer", "content"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("coordination".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let peer = input["peer"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("request_peer_review: missing 'peer'".into()))?;
        let content = input["content"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("request_peer_review: missing 'content'".into()))?;
        let context = input["context"].as_str().unwrap_or("No additional context");

        let request_id = uuid::Uuid::new_v4().to_string();

        let formatted = format!(
            "[REVIEW REQUEST id={request_id}]\n\
             From: {sender}\n\
             Context: {context}\n\n\
             {content}\n\
             [END REVIEW REQUEST]",
            sender = self.sender_name
        );

        let msg = TeamMessage {
            from: self.sender_name.clone(),
            to: peer.to_string(),
            content: formatted,
            message_type: "review_request".to_string(),
            timestamp: Utc::now(),
        };

        self.bus.send(msg)?;

        // Audit log if configured
        if let Some(log) = &self.audit_log {
            let _ = log.append(AuditEvent::TeamMessage {
                from: self.sender_name.clone(),
                to: peer.to_string(),
            });
        }

        Ok(serde_json::json!({
            "status": "request_sent",
            "request_id": request_id,
            "peer": peer,
            "note": "Peer must be delegated a turn to process the review. \
                     Use read_messages(message_type='review_response') to collect feedback."
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message_bus::MessageBus;

    #[tokio::test]
    async fn send_message_delivers() {
        let names = vec!["alice".into(), "bob".into()];
        let bus = MessageBus::new(&names);
        let mut rx = bus.subscribe("bob").unwrap();
        let bus = Arc::new(bus);

        let tool = SendMessageTool::new(Arc::clone(&bus), "alice".into());
        let result = tool
            .execute(serde_json::json!({
                "to": "bob",
                "content": "hello"
            }))
            .await
            .unwrap();

        assert_eq!(result["status"], "sent");

        let msg = rx.try_recv().unwrap();
        assert_eq!(msg.from, "alice");
        assert_eq!(msg.content, "hello");
    }

    #[tokio::test]
    async fn send_message_rate_limited() {
        let names = vec!["alice".into(), "bob".into()];
        let bus = Arc::new(MessageBus::new(&names));
        let _rx = bus.subscribe("bob"); // keep channel alive

        let tool = SendMessageTool::new(Arc::clone(&bus), "alice".into()).with_max_per_turn(2);

        // First two should succeed
        let r1 = tool
            .execute(serde_json::json!({"to": "bob", "content": "msg1"}))
            .await
            .unwrap();
        assert_eq!(r1["status"], "sent");

        let r2 = tool
            .execute(serde_json::json!({"to": "bob", "content": "msg2"}))
            .await
            .unwrap();
        assert_eq!(r2["status"], "sent");

        // Third should be rate-limited
        let r3 = tool
            .execute(serde_json::json!({"to": "bob", "content": "msg3"}))
            .await
            .unwrap();
        assert_eq!(r3["status"], "rate_limited");
    }

    #[tokio::test]
    async fn send_message_counter_reset() {
        let names = vec!["alice".into(), "bob".into()];
        let bus = Arc::new(MessageBus::new(&names));
        let _rx = bus.subscribe("bob"); // keep channel alive

        // First tool instance — send max messages
        let tool1 = SendMessageTool::new(Arc::clone(&bus), "alice".into()).with_max_per_turn(1);
        let r1 = tool1
            .execute(serde_json::json!({"to": "bob", "content": "msg1"}))
            .await
            .unwrap();
        assert_eq!(r1["status"], "sent");

        let r2 = tool1
            .execute(serde_json::json!({"to": "bob", "content": "msg2"}))
            .await
            .unwrap();
        assert_eq!(r2["status"], "rate_limited");

        // New tool instance — counter resets
        let tool2 = SendMessageTool::new(Arc::clone(&bus), "alice".into()).with_max_per_turn(1);
        let r3 = tool2
            .execute(serde_json::json!({"to": "bob", "content": "msg3"}))
            .await
            .unwrap();
        assert_eq!(r3["status"], "sent");
    }

    #[tokio::test]
    async fn read_messages_returns_pending() {
        let names = vec!["alice".into(), "bob".into()];
        let bus = MessageBus::new(&names);
        let rx = bus.subscribe("bob").unwrap();
        let bus = Arc::new(bus);
        let rx = Arc::new(Mutex::new(rx));

        bus.send(TeamMessage {
            from: "alice".into(),
            to: "bob".into(),
            content: "hello".into(),
            message_type: "text".into(),
            timestamp: Utc::now(),
        })
        .unwrap();

        let tool = ReadMessagesTool::new(rx);
        let result = tool.execute(serde_json::json!({})).await.unwrap();

        assert_eq!(result["count"], 1);
        assert_eq!(result["messages"][0]["from"], "alice");
    }

    #[tokio::test]
    async fn read_messages_returns_empty() {
        let names = vec!["alice".into()];
        let bus = MessageBus::new(&names);
        let rx = bus.subscribe("alice").unwrap();
        let rx = Arc::new(Mutex::new(rx));

        let tool = ReadMessagesTool::new(rx);
        let result = tool.execute(serde_json::json!({})).await.unwrap();

        assert_eq!(result["count"], 0);
        assert!(result["messages"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn read_messages_filter_by_from() {
        let names = vec!["alice".into(), "bob".into(), "carol".into()];
        let bus = MessageBus::new(&names);
        let rx = bus.subscribe("carol").unwrap();

        // Two messages from different senders
        bus.send(TeamMessage {
            from: "alice".into(),
            to: "carol".into(),
            content: "from alice".into(),
            message_type: "text".into(),
            timestamp: Utc::now(),
        })
        .unwrap();
        bus.send(TeamMessage {
            from: "bob".into(),
            to: "carol".into(),
            content: "from bob".into(),
            message_type: "text".into(),
            timestamp: Utc::now(),
        })
        .unwrap();

        let tool = ReadMessagesTool::new(Arc::new(Mutex::new(rx)));
        let result = tool
            .execute(serde_json::json!({"from": "alice"}))
            .await
            .unwrap();

        assert_eq!(result["count"], 1);
        assert_eq!(result["messages"][0]["from"], "alice");
    }

    #[tokio::test]
    async fn read_messages_filter_by_type() {
        let names = vec!["alice".into(), "bob".into()];
        let bus = MessageBus::new(&names);
        let rx = bus.subscribe("bob").unwrap();

        bus.send(TeamMessage {
            from: "alice".into(),
            to: "bob".into(),
            content: "text msg".into(),
            message_type: "text".into(),
            timestamp: Utc::now(),
        })
        .unwrap();
        bus.send(TeamMessage {
            from: "alice".into(),
            to: "bob".into(),
            content: "review content".into(),
            message_type: "review_request".into(),
            timestamp: Utc::now(),
        })
        .unwrap();

        let tool = ReadMessagesTool::new(Arc::new(Mutex::new(rx)));
        let result = tool
            .execute(serde_json::json!({"message_type": "review_request"}))
            .await
            .unwrap();

        assert_eq!(result["count"], 1);
        assert_eq!(result["messages"][0]["message_type"], "review_request");
    }

    #[tokio::test]
    async fn read_messages_filter_by_since() {
        let names = vec!["alice".into(), "bob".into()];
        let bus = MessageBus::new(&names);
        let rx = bus.subscribe("bob").unwrap();

        let old_time = Utc::now() - chrono::Duration::seconds(60);
        let cutoff = Utc::now() - chrono::Duration::seconds(30);

        bus.send(TeamMessage {
            from: "alice".into(),
            to: "bob".into(),
            content: "old msg".into(),
            message_type: "text".into(),
            timestamp: old_time,
        })
        .unwrap();
        bus.send(TeamMessage {
            from: "alice".into(),
            to: "bob".into(),
            content: "new msg".into(),
            message_type: "text".into(),
            timestamp: Utc::now(),
        })
        .unwrap();

        let tool = ReadMessagesTool::new(Arc::new(Mutex::new(rx)));
        let result = tool
            .execute(serde_json::json!({"since": cutoff.to_rfc3339()}))
            .await
            .unwrap();

        assert_eq!(result["count"], 1);
        assert_eq!(result["messages"][0]["content"], "new msg");
    }

    #[tokio::test]
    async fn read_messages_preserves_unmatched() {
        let names = vec!["alice".into(), "bob".into()];
        let bus = MessageBus::new(&names);
        let rx = bus.subscribe("bob").unwrap();

        bus.send(TeamMessage {
            from: "alice".into(),
            to: "bob".into(),
            content: "text msg".into(),
            message_type: "text".into(),
            timestamp: Utc::now(),
        })
        .unwrap();
        bus.send(TeamMessage {
            from: "alice".into(),
            to: "bob".into(),
            content: "review content".into(),
            message_type: "review_request".into(),
            timestamp: Utc::now(),
        })
        .unwrap();

        let tool = ReadMessagesTool::new(Arc::new(Mutex::new(rx)));

        // First read: only review_request
        let r1 = tool
            .execute(serde_json::json!({"message_type": "review_request"}))
            .await
            .unwrap();
        assert_eq!(r1["count"], 1);
        assert_eq!(r1["messages"][0]["message_type"], "review_request");

        // Second read: the text message should still be available
        let r2 = tool.execute(serde_json::json!({})).await.unwrap();
        assert_eq!(r2["count"], 1);
        assert_eq!(r2["messages"][0]["message_type"], "text");
    }

    #[tokio::test]
    async fn read_messages_no_filter_drains_all() {
        let names = vec!["alice".into(), "bob".into()];
        let bus = MessageBus::new(&names);
        let rx = bus.subscribe("bob").unwrap();

        for i in 0..3 {
            bus.send(TeamMessage {
                from: "alice".into(),
                to: "bob".into(),
                content: format!("msg {i}"),
                message_type: "text".into(),
                timestamp: Utc::now(),
            })
            .unwrap();
        }

        let tool = ReadMessagesTool::new(Arc::new(Mutex::new(rx)));
        let result = tool.execute(serde_json::json!({})).await.unwrap();

        assert_eq!(result["count"], 3);

        // Second read should be empty
        let result2 = tool.execute(serde_json::json!({})).await.unwrap();
        assert_eq!(result2["count"], 0);
    }

    #[test]
    fn send_message_schema() {
        let names = vec!["a".into()];
        let bus = Arc::new(MessageBus::new(&names));
        let tool = SendMessageTool::new(bus, "a".into());
        assert_eq!(tool.name(), "send_message");
        let schema = tool.input_schema();
        assert!(schema["properties"]["to"].is_object());
        assert!(schema["properties"]["content"].is_object());
    }

    #[test]
    fn read_messages_schema() {
        let names = vec!["a".into()];
        let bus = MessageBus::new(&names);
        let rx = bus.subscribe("a").unwrap();
        let tool = ReadMessagesTool::new(Arc::new(Mutex::new(rx)));
        assert_eq!(tool.name(), "read_messages");
        let schema = tool.input_schema();
        assert!(schema["properties"]["from"].is_object());
        assert!(schema["properties"]["message_type"].is_object());
        assert!(schema["properties"]["since"].is_object());
    }

    #[tokio::test]
    async fn request_peer_review_sends_structured() {
        let names = vec!["coder".into(), "reviewer".into()];
        let bus = MessageBus::new(&names);
        let mut rx = bus.subscribe("reviewer").unwrap();
        let bus = Arc::new(bus);

        let tool = RequestPeerReviewTool::new(Arc::clone(&bus), "coder".into());
        let result = tool
            .execute(serde_json::json!({
                "peer": "reviewer",
                "content": "function foo() { return 42; }",
                "context": "Check for correctness"
            }))
            .await
            .unwrap();

        assert_eq!(result["status"], "request_sent");
        assert_eq!(result["peer"], "reviewer");
        assert!(result["request_id"].as_str().is_some());

        let msg = rx.try_recv().unwrap();
        assert_eq!(msg.from, "coder");
        assert_eq!(msg.to, "reviewer");
        assert_eq!(msg.message_type, "review_request");
        assert!(msg.content.contains("[REVIEW REQUEST"));
        assert!(msg.content.contains("function foo()"));
        assert!(msg.content.contains("Check for correctness"));
    }

    #[test]
    fn request_peer_review_schema() {
        let names = vec!["a".into(), "b".into()];
        let bus = Arc::new(MessageBus::new(&names));
        let tool = RequestPeerReviewTool::new(bus, "a".into());
        assert_eq!(tool.name(), "request_peer_review");
        let schema = tool.input_schema();
        assert!(schema["properties"]["peer"].is_object());
        assert!(schema["properties"]["content"].is_object());
        assert!(schema["properties"]["context"].is_object());
        assert_eq!(schema["required"], serde_json::json!(["peer", "content"]));
    }
}
