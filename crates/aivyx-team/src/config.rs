use std::path::Path;

use aivyx_core::{AivyxError, Result};
use serde::{Deserialize, Serialize};

/// Orchestration strategy for the team.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OrchestrationMode {
    /// One agent (the lead) coordinates all others.
    LeadAgent { lead: String },
}

/// Configuration for a team member.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMemberConfig {
    /// Agent profile name (must exist in ~/.aivyx/agents/).
    pub name: String,
    /// Role within the team.
    pub role: String,
}

/// Configuration for inter-specialist dialogue behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DialogueConfig {
    /// Whether specialists can send messages directly to each other.
    /// When false, only the coordinator can send messages.
    #[serde(default = "default_enable_peer_dialogue")]
    pub enable_peer_dialogue: bool,
    /// Maximum messages a single agent can send per delegation turn.
    #[serde(default = "default_max_messages_per_turn")]
    pub max_messages_per_turn: u32,
}

fn default_enable_peer_dialogue() -> bool {
    true
}

fn default_max_messages_per_turn() -> u32 {
    10
}

impl Default for DialogueConfig {
    fn default() -> Self {
        Self {
            enable_peer_dialogue: default_enable_peer_dialogue(),
            max_messages_per_turn: default_max_messages_per_turn(),
        }
    }
}

/// Team configuration, persisted as TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamConfig {
    /// Team name.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// How the team is coordinated.
    pub orchestration: OrchestrationMode,
    /// Team members.
    pub members: Vec<TeamMemberConfig>,
    /// Inter-specialist dialogue settings.
    #[serde(default)]
    pub dialogue: DialogueConfig,
}

impl TeamConfig {
    /// Load team config from a TOML file.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let content = std::fs::read_to_string(path.as_ref())?;
        toml::from_str(&content).map_err(|e| AivyxError::TomlDe(e.to_string()))
    }

    /// Save team config to a TOML file.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let content =
            toml::to_string_pretty(self).map_err(|e| AivyxError::TomlSer(e.to_string()))?;
        std::fs::write(path.as_ref(), content)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_roundtrip() {
        let config = TeamConfig {
            name: "test-team".into(),
            description: "A test team".into(),
            orchestration: OrchestrationMode::LeadAgent {
                lead: "coordinator".into(),
            },
            members: vec![
                TeamMemberConfig {
                    name: "coordinator".into(),
                    role: "Lead".into(),
                },
                TeamMemberConfig {
                    name: "coder".into(),
                    role: "Coder".into(),
                },
            ],
            dialogue: DialogueConfig::default(),
        };

        let toml_str = toml::to_string_pretty(&config).unwrap();
        let loaded: TeamConfig = toml::from_str(&toml_str).unwrap();

        assert_eq!(loaded.name, "test-team");
        assert_eq!(loaded.members.len(), 2);
    }

    #[test]
    fn save_load_file() {
        let config = TeamConfig {
            name: "file-test".into(),
            description: "Test".into(),
            orchestration: OrchestrationMode::LeadAgent {
                lead: "lead".into(),
            },
            members: vec![],
            dialogue: DialogueConfig::default(),
        };

        let path =
            std::env::temp_dir().join(format!("aivyx-team-test-{}.toml", uuid::Uuid::new_v4()));
        config.save(&path).unwrap();
        let loaded = TeamConfig::load(&path).unwrap();
        assert_eq!(loaded.name, "file-test");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn dialogue_config_defaults() {
        let config = DialogueConfig::default();
        assert!(config.enable_peer_dialogue);
        assert_eq!(config.max_messages_per_turn, 10);
    }

    #[test]
    fn dialogue_config_roundtrip() {
        let config = TeamConfig {
            name: "dialogue-test".into(),
            description: "Test".into(),
            orchestration: OrchestrationMode::LeadAgent {
                lead: "lead".into(),
            },
            members: vec![],
            dialogue: DialogueConfig {
                enable_peer_dialogue: false,
                max_messages_per_turn: 5,
            },
        };

        let toml_str = toml::to_string_pretty(&config).unwrap();
        let loaded: TeamConfig = toml::from_str(&toml_str).unwrap();
        assert!(!loaded.dialogue.enable_peer_dialogue);
        assert_eq!(loaded.dialogue.max_messages_per_turn, 5);
    }

    #[test]
    fn dialogue_config_missing_uses_defaults() {
        // Existing TOML without [dialogue] section should deserialize fine
        let toml_str = r#"
            name = "old-team"
            description = "Legacy config"
            members = []

            [orchestration]
            LeadAgent = { lead = "coordinator" }
        "#;
        let loaded: TeamConfig = toml::from_str(toml_str).unwrap();
        assert!(loaded.dialogue.enable_peer_dialogue);
        assert_eq!(loaded.dialogue.max_messages_per_turn, 10);
    }
}
