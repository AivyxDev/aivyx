//! Horizontal scaling configuration.
//!
//! When `stateless_mode` is enabled, the server expects all persistent state
//! to live in external PostgreSQL and Redis instances. Local `redb` storage
//! is used only for ephemeral caches.

use serde::{Deserialize, Serialize};

/// Configuration for horizontal scaling mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScalingConfig {
    /// When true, the server operates in stateless mode.
    /// All persistent state must be backed by PostgreSQL + Redis.
    pub stateless_mode: bool,

    /// Strategy for WebSocket/voice session affinity.
    pub session_affinity: SessionAffinityStrategy,

    /// Unique instance identifier for distributed coordination.
    /// Defaults to a random UUID if not set.
    pub instance_id: String,
}

impl Default for ScalingConfig {
    fn default() -> Self {
        Self {
            stateless_mode: false,
            session_affinity: SessionAffinityStrategy::default(),
            instance_id: uuid::Uuid::new_v4().to_string(),
        }
    }
}

/// Strategy for maintaining WebSocket and voice session affinity
/// across multiple server instances.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SessionAffinityStrategy {
    /// No session affinity — any instance can handle any request.
    /// WebSocket connections may break on instance restart.
    #[default]
    None,

    /// Consistent hashing based on session ID.
    /// Requires a load balancer that supports hash-based routing.
    ConsistentHashing,

    /// Route based on a custom header (e.g., `X-Aivyx-Instance-Id`).
    /// The load balancer should use this header for sticky sessions.
    HeaderBased,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_scaling_config_is_not_stateless() {
        let config = ScalingConfig::default();
        assert!(!config.stateless_mode);
        assert_eq!(config.session_affinity, SessionAffinityStrategy::None);
        assert!(!config.instance_id.is_empty());
    }

    #[test]
    fn scaling_config_serde_roundtrip() {
        let config = ScalingConfig {
            stateless_mode: true,
            session_affinity: SessionAffinityStrategy::ConsistentHashing,
            instance_id: "node-1".into(),
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: ScalingConfig = serde_json::from_str(&json).unwrap();
        assert!(parsed.stateless_mode);
        assert_eq!(parsed.session_affinity, SessionAffinityStrategy::ConsistentHashing);
        assert_eq!(parsed.instance_id, "node-1");
    }

    #[test]
    fn session_affinity_strategy_serde() {
        let strategies = vec![
            (SessionAffinityStrategy::None, "\"none\""),
            (SessionAffinityStrategy::ConsistentHashing, "\"consistent_hashing\""),
            (SessionAffinityStrategy::HeaderBased, "\"header_based\""),
        ];
        for (strategy, expected) in strategies {
            let json = serde_json::to_string(&strategy).unwrap();
            assert_eq!(json, expected);
        }
    }
}
