//! Multi-agent team coordination for the aivyx framework.
//!
//! Provides team configuration, inter-agent messaging, a team runtime,
//! delegation tools, and capability attenuation for team members.

pub mod capability_delegation;
pub mod config;
pub mod decompose;
pub mod delegation;
pub mod job_tracker;
pub mod message_bus;
pub mod message_tools;
pub mod nonagon;
pub mod runtime;
pub mod session_store;
pub mod spawn;
pub mod suggest;
pub mod synthesize;
pub mod verify;

pub use config::{DialogueConfig, OrchestrationMode, TeamConfig, TeamMemberConfig};
pub use delegation::SpecialistPool;
pub use message_bus::{MessageBus, TeamMessage};
pub use runtime::TeamRuntime;
pub use session_store::{PersistedTeamSession, TeamSessionMetadata, TeamSessionStore};
