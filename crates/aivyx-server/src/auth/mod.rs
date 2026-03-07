//! Authentication and session management.
//!
//! Provides [`SessionCache`] for in-memory SSO session tracking.

pub mod session_cache;

pub use session_cache::{SessionCache, SessionRecord};
