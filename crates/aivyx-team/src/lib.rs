//! Multi-agent team coordination for the aivyx framework.
//!
//! Provides team configuration, inter-agent messaging, a team runtime,
//! delegation tools, and capability attenuation for team members.

pub mod capability_delegation;
pub mod config;
pub mod decompose;
pub mod delegation;
pub mod job_tracker;
#[cfg(feature = "memory")]
pub mod memory_sharing;
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
#[cfg(feature = "memory")]
pub use memory_sharing::TeamMemoryQueryTool;
pub use message_bus::{MessageBus, TeamMessage};
pub use runtime::TeamRuntime;
pub use session_store::{PersistedTeamSession, TeamSessionMetadata, TeamSessionStore};
