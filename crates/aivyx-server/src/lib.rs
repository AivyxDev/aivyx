//! HTTP API gateway for the aivyx framework.
//!
//! Exposes the aivyx agent engine over an Axum HTTP server with REST endpoints
//! for chat, agents, teams, memory, audit, and sessions. Authentication uses
//! Bearer tokens stored in `EncryptedStore`. Streaming responses use SSE in an
//! OpenAI-compatible format.

pub mod app_state;
pub mod channels;
pub mod error;
pub mod middleware;
pub mod routes;
pub mod scheduler;
pub mod startup;
pub mod transcription;
pub mod validation;

pub use app_state::AppState;
pub use error::ServerError;
pub use startup::{build_app_state_with_keys, build_router};
