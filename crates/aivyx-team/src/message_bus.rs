use std::collections::HashMap;

use aivyx_core::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// A message passed between agents on a team.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMessage {
    /// Sender agent name.
    pub from: String,
    /// Recipient agent name (empty = broadcast).
    pub to: String,
    /// Message content.
    pub content: String,
    /// Message type tag.
    pub message_type: String,
    /// When the message was created.
    pub timestamp: DateTime<Utc>,
}

/// In-process message bus for team member communication.
///
/// Uses `broadcast` channels so multiple consumers can subscribe to each
/// agent's inbox. This allows both the lead agent and specialists to
/// receive messages — unlike `mpsc` where only one consumer exists.
pub struct MessageBus {
    senders: HashMap<String, broadcast::Sender<TeamMessage>>,
}

impl MessageBus {
    /// Create a new message bus with channels for the named agents.
    pub fn new(agent_names: &[String]) -> Self {
        let mut senders = HashMap::new();

        for name in agent_names {
            let (tx, _rx) = broadcast::channel(64);
            senders.insert(name.clone(), tx);
        }

        Self { senders }
    }

    /// Send a message to a specific agent.
    pub fn send(&self, msg: TeamMessage) -> Result<()> {
        let tx = self.senders.get(&msg.to).ok_or_else(|| {
            aivyx_core::AivyxError::Agent(format!("unknown recipient: {}", msg.to))
        })?;
        tx.send(msg)
            .map_err(|e| aivyx_core::AivyxError::Agent(format!("send failed: {e}")))?;
        Ok(())
    }

    /// Broadcast a message to all agents except the sender.
    pub fn broadcast(&self, msg: TeamMessage) -> Result<()> {
        for (name, tx) in &self.senders {
            if name != &msg.from {
                let mut m = msg.clone();
                m.to = name.clone();
                if let Err(e) = tx.send(m) {
                    tracing::warn!("MessageBus: broadcast to {name} failed: {e}");
                }
            }
        }
        Ok(())
    }

    /// Subscribe to messages for a specific agent.
    ///
    /// Unlike the previous `take_receiver()` which could only be called once,
    /// `subscribe()` can be called multiple times — each call creates an
    /// independent receiver. This enables specialists to receive messages
    /// even when the lead agent also has a subscription.
    pub fn subscribe(&self, name: &str) -> Option<broadcast::Receiver<TeamMessage>> {
        self.senders.get(name).map(|tx| tx.subscribe())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[tokio::test]
    async fn team_message_has_timestamp() {
        let before = Utc::now();
        let msg = TeamMessage {
            from: "alice".into(),
            to: "bob".into(),
            content: "hello".into(),
            message_type: "text".into(),
            timestamp: Utc::now(),
        };
        let after = Utc::now();
        assert!(msg.timestamp >= before && msg.timestamp <= after);
    }

    #[tokio::test]
    async fn send_and_receive() {
        let names = vec!["alice".into(), "bob".into()];
        let bus = MessageBus::new(&names);
        let mut rx = bus.subscribe("bob").unwrap();

        let msg = TeamMessage {
            from: "alice".into(),
            to: "bob".into(),
            content: "hello".into(),
            message_type: "text".into(),
            timestamp: Utc::now(),
        };

        bus.send(msg).unwrap();
        let received = rx.recv().await.unwrap();
        assert_eq!(received.from, "alice");
        assert_eq!(received.content, "hello");
    }

    #[tokio::test]
    async fn broadcast_skips_sender() {
        let names = vec!["a".into(), "b".into(), "c".into()];
        let bus = MessageBus::new(&names);

        let mut rx_a = bus.subscribe("a").unwrap();
        let mut rx_b = bus.subscribe("b").unwrap();
        let mut rx_c = bus.subscribe("c").unwrap();

        let msg = TeamMessage {
            from: "a".into(),
            to: String::new(),
            content: "hi all".into(),
            message_type: "broadcast".into(),
            timestamp: Utc::now(),
        };

        bus.broadcast(msg).unwrap();

        // b and c should receive, a should not
        assert!(rx_b.try_recv().is_ok());
        assert!(rx_c.try_recv().is_ok());
        assert!(rx_a.try_recv().is_err());
    }

    #[tokio::test]
    async fn send_to_unknown_recipient_returns_error() {
        let names = vec!["alice".into()];
        let bus = MessageBus::new(&names);

        let msg = TeamMessage {
            from: "alice".into(),
            to: "nonexistent".into(),
            content: "hello".into(),
            message_type: "text".into(),
            timestamp: Utc::now(),
        };

        let result = bus.send(msg);
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("unknown recipient"));
    }

    #[test]
    fn subscribe_multiple_times() {
        let names = vec!["alice".into(), "bob".into()];
        let bus = MessageBus::new(&names);

        // Can subscribe multiple times — broadcast channels support this
        let first = bus.subscribe("alice");
        assert!(first.is_some());

        let second = bus.subscribe("alice");
        assert!(second.is_some()); // Unlike take_receiver, this succeeds
    }

    #[tokio::test]
    async fn multiple_subscribers_receive_same_message() {
        let names = vec!["alice".into(), "bob".into()];
        let bus = MessageBus::new(&names);

        // Two subscribers on the same channel
        let mut rx1 = bus.subscribe("bob").unwrap();
        let mut rx2 = bus.subscribe("bob").unwrap();

        let msg = TeamMessage {
            from: "alice".into(),
            to: "bob".into(),
            content: "hello".into(),
            message_type: "text".into(),
            timestamp: Utc::now(),
        };

        bus.send(msg).unwrap();

        // Both should receive the message
        let m1 = rx1.recv().await.unwrap();
        let m2 = rx2.recv().await.unwrap();
        assert_eq!(m1.content, "hello");
        assert_eq!(m2.content, "hello");
    }
}
