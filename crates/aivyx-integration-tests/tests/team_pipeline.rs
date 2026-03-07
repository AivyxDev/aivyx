//! Integration tests for the team pipeline: delegation tools, MessageBus,
//! message tools, and team audit events.

use std::sync::Arc;

use aivyx_audit::AuditEvent;
use aivyx_core::Tool as _;
use aivyx_team::message_bus::MessageBus;
use aivyx_team::message_bus::TeamMessage;
use aivyx_team::message_tools::{ReadMessagesTool, SendMessageTool};
use tokio::sync::Mutex;

#[tokio::test]
async fn message_bus_delivers_between_members() {
    let names = vec!["coordinator".into(), "researcher".into(), "coder".into()];
    let bus = MessageBus::new(&names);

    let mut rx_researcher = bus.subscribe("researcher").unwrap();
    let mut rx_coder = bus.subscribe("coder").unwrap();

    let bus = Arc::new(bus);

    // Coordinator sends to researcher
    bus.send(TeamMessage {
        from: "coordinator".into(),
        to: "researcher".into(),
        content: "Find info about X".into(),
        message_type: "task".into(),
        timestamp: chrono::Utc::now(),
    })
    .unwrap();

    // Coordinator sends to coder
    bus.send(TeamMessage {
        from: "coordinator".into(),
        to: "coder".into(),
        content: "Implement Y".into(),
        message_type: "task".into(),
        timestamp: chrono::Utc::now(),
    })
    .unwrap();

    let msg1 = rx_researcher.try_recv().unwrap();
    assert_eq!(msg1.from, "coordinator");
    assert_eq!(msg1.content, "Find info about X");

    let msg2 = rx_coder.try_recv().unwrap();
    assert_eq!(msg2.from, "coordinator");
    assert_eq!(msg2.content, "Implement Y");
}

#[tokio::test]
async fn send_message_tool_delivers_through_bus() {
    let names = vec!["lead".into(), "worker".into()];
    let bus = MessageBus::new(&names);
    let mut rx = bus.subscribe("worker").unwrap();
    let bus = Arc::new(bus);

    let tool = SendMessageTool::new(Arc::clone(&bus), "lead".into());

    let result = tool
        .execute(serde_json::json!({
            "to": "worker",
            "content": "Please do this task",
            "message_type": "delegation"
        }))
        .await
        .unwrap();

    assert_eq!(result["status"], "sent");

    let msg = rx.try_recv().unwrap();
    assert_eq!(msg.from, "lead");
    assert_eq!(msg.to, "worker");
    assert_eq!(msg.content, "Please do this task");
    assert_eq!(msg.message_type, "delegation");
}

#[tokio::test]
async fn read_messages_tool_drains_all_pending() {
    let names = vec!["alice".into(), "bob".into()];
    let bus = MessageBus::new(&names);
    let rx = bus.subscribe("bob").unwrap();
    let bus = Arc::new(bus);
    let rx = Arc::new(Mutex::new(rx));

    // Send 3 messages
    for i in 0..3 {
        bus.send(TeamMessage {
            from: "alice".into(),
            to: "bob".into(),
            content: format!("msg {i}"),
            message_type: "text".into(),
            timestamp: chrono::Utc::now(),
        })
        .unwrap();
    }

    let tool = ReadMessagesTool::new(rx);
    let result = tool.execute(serde_json::json!({})).await.unwrap();

    assert_eq!(result["count"], 3);
    let messages = result["messages"].as_array().unwrap();
    assert_eq!(messages[0]["content"], "msg 0");
    assert_eq!(messages[2]["content"], "msg 2");
}

#[test]
fn team_delegation_audit_event_serializes() {
    let event = AuditEvent::TeamDelegation {
        from: "coordinator".into(),
        to: "researcher".into(),
        task: "Find info about X".into(),
    };

    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("TeamDelegation"));
    assert!(json.contains("coordinator"));
    assert!(json.contains("researcher"));

    // Round-trip
    let deserialized: AuditEvent = serde_json::from_str(&json).unwrap();
    match deserialized {
        AuditEvent::TeamDelegation { from, to, task } => {
            assert_eq!(from, "coordinator");
            assert_eq!(to, "researcher");
            assert_eq!(task, "Find info about X");
        }
        _ => panic!("Expected TeamDelegation"),
    }
}

#[test]
fn team_message_audit_event_serializes() {
    let event = AuditEvent::TeamMessage {
        from: "guardian".into(),
        to: "coordinator".into(),
    };

    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("TeamMessage"));

    let deserialized: AuditEvent = serde_json::from_str(&json).unwrap();
    match deserialized {
        AuditEvent::TeamMessage { from, to } => {
            assert_eq!(from, "guardian");
            assert_eq!(to, "coordinator");
        }
        _ => panic!("Expected TeamMessage"),
    }
}
