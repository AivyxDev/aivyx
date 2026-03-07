//! Inbound communication channels.
//!
//! Receives messages from external platforms (Telegram, Email, etc.) and
//! routes them through the agent turn loop. Each platform adapter implements
//! the [`InboundChannel`] trait and is managed by [`ChannelManager`].

pub mod manager;
pub mod session;
#[cfg(feature = "telegram")]
pub mod telegram;

pub use manager::{ChannelManager, InboundChannel, MessageHandler, spawn_channel_manager};
pub use session::derive_channel_session_id;
