//! OIDC SSO support for the Aivyx platform.
//!
//! Provides [`OidcValidator`] for JWT token validation, [`RoleMapper`] for
//! mapping OIDC group claims to [`AivyxRole`], and [`SsoError`] for SSO-specific
//! error handling.

pub mod oidc;
pub mod role_mapper;

pub use oidc::{OidcClaims, OidcValidator};
pub use role_mapper::{GroupRoleMapping, RoleMapper};

use thiserror::Error;

/// Errors specific to SSO/OIDC operations.
#[derive(Debug, Error)]
pub enum SsoError {
    /// The JWT token format is invalid (missing parts, bad base64, etc.).
    #[error("invalid token format: {0}")]
    InvalidToken(String),

    /// The JWT token has expired.
    #[error("token expired")]
    TokenExpired,

    /// Failed to deserialize the token payload.
    #[error("payload deserialization failed: {0}")]
    PayloadDeserialize(String),
}
