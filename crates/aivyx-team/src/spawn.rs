//! Dynamic specialist spawning tool.
//!
//! [`SpawnSpecialistTool`] allows the lead agent to create new specialist
//! agents mid-session with custom roles. This enables adaptive team
//! composition where the lead discovers it needs expertise that wasn't
//! pre-configured in the team definition.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tracing::info;

use aivyx_agent::AgentSession;
use aivyx_core::{AivyxError, CapabilityScope, Result, Tool, ToolId};

use crate::delegation::SpecialistPool;
use crate::message_bus::MessageBus;

/// Tool that lets the lead agent create specialist agents dynamically.
///
/// Spawned specialists are registered in both the [`SpecialistPool`] (for
/// delegation) and the [`MessageBus`] (for messaging). A safety limit
/// prevents runaway spawning.
pub struct SpawnSpecialistTool {
    id: ToolId,
    session: Arc<AgentSession>,
    pool: SpecialistPool,
    bus: Arc<MessageBus>,
    max_spawned: usize,
    spawned_count: Arc<AtomicUsize>,
}

impl SpawnSpecialistTool {
    /// Create a new spawn tool with the given safety limit.
    pub fn new(
        session: Arc<AgentSession>,
        pool: SpecialistPool,
        bus: Arc<MessageBus>,
        max_spawned: usize,
    ) -> Self {
        Self {
            id: ToolId::new(),
            session,
            pool,
            bus,
            max_spawned,
            spawned_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Clean up an ephemeral specialist agent.
    ///
    /// Removes the agent from both the [`SpecialistPool`] and the
    /// [`MessageBus`], and decrements the spawned count so the slot can
    /// be reused.
    pub async fn cleanup_agent(&self, name: &str) -> Result<()> {
        self.pool.deregister_spawned(name).await;
        self.bus.deregister_agent(name)?;
        // Decrement, but never below zero
        self.spawned_count
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |c| {
                if c > 0 { Some(c - 1) } else { Some(0) }
            })
            .ok();
        info!("Cleaned up ephemeral agent '{}'", name);
        Ok(())
    }
}

#[async_trait]
impl Tool for SpawnSpecialistTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "spawn_specialist"
    }

    fn description(&self) -> &str {
        "Create a new specialist agent with a custom role. The agent will be available for delegation and messaging."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "agent_name": {
                    "type": "string",
                    "description": "Unique name for the new specialist (e.g., 'security-auditor')"
                },
                "role": {
                    "type": "string",
                    "description": "Role description for the specialist (used in team context)"
                },
                "profile": {
                    "type": "string",
                    "description": "Optional agent profile name to use as base configuration. Defaults to 'aivyx'."
                }
            },
            "required": ["agent_name", "role"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("coordination".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let agent_name = input["agent_name"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("spawn_specialist: missing 'agent_name'".into()))?;
        let role = input["role"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("spawn_specialist: missing 'role'".into()))?;
        let profile = input["profile"].as_str().unwrap_or("aivyx");

        // Check safety limit
        let current = self.spawned_count.load(Ordering::Relaxed);
        if current >= self.max_spawned {
            return Err(AivyxError::Agent(format!(
                "spawn_specialist: limit reached ({current}/{max}). Cannot spawn more specialists.",
                max = self.max_spawned
            )));
        }

        // Create the agent
        let _agent = self.session.create_agent(profile).await?;

        // Register in the message bus so the new specialist can receive messages
        let _rx = self.bus.register_agent(agent_name)?;

        // Register in the specialist pool for delegation
        self.pool.register_spawned(agent_name, role).await;

        self.spawned_count.fetch_add(1, Ordering::Relaxed);

        info!(
            "Spawned specialist '{}' (role: {}, profile: {}, count: {}/{})",
            agent_name,
            role,
            profile,
            self.spawned_count.load(Ordering::Relaxed),
            self.max_spawned
        );

        Ok(serde_json::json!({
            "status": "spawned",
            "agent_name": agent_name,
            "role": role,
            "profile": profile,
            "spawned_count": self.spawned_count.load(Ordering::Relaxed),
            "max_spawned": self.max_spawned,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_tool_name_and_schema() {
        let dir = std::env::temp_dir().join(format!("aivyx-spawn-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let dirs = aivyx_config::AivyxDirs::new(&dir);
        let config = aivyx_config::AivyxConfig::default();
        let key = aivyx_crypto::MasterKey::generate();
        let session = Arc::new(AgentSession::new(dirs, config, key));

        let names = vec!["lead".to_string()];
        let bus = Arc::new(MessageBus::new(&names));
        let pool = SpecialistPool::new(
            Arc::clone(&session),
            Some(Arc::clone(&bus)),
            aivyx_capability::CapabilitySet::new(),
            None,
            crate::config::DialogueConfig::default(),
        );

        let tool = SpawnSpecialistTool::new(session, pool, bus, 5);
        assert_eq!(tool.name(), "spawn_specialist");

        let schema = tool.input_schema();
        assert!(schema["properties"]["agent_name"].is_object());
        assert!(schema["properties"]["role"].is_object());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn cleanup_agent_decrements_count_and_deregisters() {
        let dir = std::env::temp_dir().join(format!("aivyx-cleanup-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let dirs = aivyx_config::AivyxDirs::new(&dir);
        let config = aivyx_config::AivyxConfig::default();
        let key = aivyx_crypto::MasterKey::generate();
        let session = Arc::new(AgentSession::new(dirs, config, key));

        let names = vec!["lead".to_string()];
        let bus = Arc::new(MessageBus::new(&names));
        let pool = SpecialistPool::new(
            Arc::clone(&session),
            Some(Arc::clone(&bus)),
            aivyx_capability::CapabilitySet::new(),
            None,
            crate::config::DialogueConfig::default(),
        );

        let tool = SpawnSpecialistTool::new(session, pool, Arc::clone(&bus), 5);

        // Simulate a spawned agent: register in bus + bump count
        bus.register_agent("ephemeral").unwrap();
        tool.spawned_count.fetch_add(1, Ordering::Relaxed);
        assert_eq!(tool.spawned_count.load(Ordering::Relaxed), 1);

        // Cleanup
        tool.cleanup_agent("ephemeral").await.unwrap();

        // Count decremented
        assert_eq!(tool.spawned_count.load(Ordering::Relaxed), 0);
        // Agent removed from bus
        assert!(bus.subscribe("ephemeral").is_none());

        std::fs::remove_dir_all(&dir).ok();
    }
}
