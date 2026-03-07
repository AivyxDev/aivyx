//! Minimal JWT/OIDC token validation.
//!
//! Decodes JWT tokens, deserializes claims, and checks expiry.
//! Signature validation is intentionally skipped — JWKS fetching will be
//! added in a future phase.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};

use crate::SsoError;

/// Claims extracted from an OIDC ID token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcClaims {
    /// Subject identifier (unique user ID from the IdP).
    pub sub: String,
    /// User email address, if provided.
    pub email: Option<String>,
    /// Group memberships from the IdP.
    #[serde(default)]
    pub groups: Vec<String>,
    /// Optional tenant hint for multi-tenant routing.
    pub tenant_hint: Option<String>,
    /// Token expiration time (Unix timestamp).
    pub exp: u64,
}

/// Validates OIDC JWT tokens.
///
/// Currently performs payload decoding and expiry checks only.
/// Signature validation (JWKS) will be added in a future phase.
pub struct OidcValidator {
    /// The expected issuer URL.
    pub issuer: String,
    /// The expected client/audience ID.
    pub client_id: String,
}

impl OidcValidator {
    /// Create a new validator for the given issuer and client ID.
    pub fn new(issuer: String, client_id: String) -> Self {
        Self { issuer, client_id }
    }

    /// Decode and validate a JWT token.
    ///
    /// Splits the token into header, payload, and signature parts,
    /// base64-decodes the payload, deserializes it to [`OidcClaims`],
    /// and checks that the token has not expired.
    ///
    /// **Note**: Signature validation is not yet implemented.
    pub fn validate_token(&self, token: &str) -> Result<OidcClaims, SsoError> {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 3 {
            return Err(SsoError::InvalidToken(format!(
                "expected 3 dot-separated parts, got {}",
                parts.len()
            )));
        }

        let payload_bytes = URL_SAFE_NO_PAD
            .decode(parts[1])
            .map_err(|e| SsoError::InvalidToken(format!("base64 decode failed: {e}")))?;

        let claims: OidcClaims = serde_json::from_slice(&payload_bytes)
            .map_err(|e| SsoError::PayloadDeserialize(e.to_string()))?;

        // Check expiry
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        if claims.exp <= now {
            return Err(SsoError::TokenExpired);
        }

        Ok(claims)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal JWT token (no real signature).
    fn build_test_jwt(claims: &OidcClaims) -> String {
        let header = URL_SAFE_NO_PAD.encode(b"{\"alg\":\"RS256\",\"typ\":\"JWT\"}");
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).unwrap());
        let signature = URL_SAFE_NO_PAD.encode(b"fake-signature");
        format!("{header}.{payload}.{signature}")
    }

    #[test]
    fn valid_token_decodes_correctly() {
        let claims = OidcClaims {
            sub: "user-123".into(),
            email: Some("alice@example.com".into()),
            groups: vec!["admins".into(), "devs".into()],
            tenant_hint: Some("acme".into()),
            exp: u64::MAX, // far future
        };

        let token = build_test_jwt(&claims);
        let validator = OidcValidator::new("https://idp.example.com".into(), "my-client".into());
        let result = validator.validate_token(&token).unwrap();

        assert_eq!(result.sub, "user-123");
        assert_eq!(result.email, Some("alice@example.com".into()));
        assert_eq!(result.groups, vec!["admins", "devs"]);
        assert_eq!(result.tenant_hint, Some("acme".into()));
    }

    #[test]
    fn expired_token_fails() {
        let claims = OidcClaims {
            sub: "user-456".into(),
            email: None,
            groups: vec![],
            tenant_hint: None,
            exp: 0, // already expired
        };

        let token = build_test_jwt(&claims);
        let validator = OidcValidator::new("https://idp.example.com".into(), "my-client".into());
        let result = validator.validate_token(&token);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, SsoError::TokenExpired));
    }

    #[test]
    fn malformed_token_fails() {
        let validator = OidcValidator::new("https://idp.example.com".into(), "my-client".into());

        // Too few parts
        let result = validator.validate_token("only.two");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), SsoError::InvalidToken(_)));

        // Bad base64
        let result = validator.validate_token("a.!!!invalid!!!.c");
        assert!(result.is_err());
    }

    #[test]
    fn token_with_no_optional_fields() {
        let claims = OidcClaims {
            sub: "minimal-user".into(),
            email: None,
            groups: vec![],
            tenant_hint: None,
            exp: u64::MAX,
        };

        let token = build_test_jwt(&claims);
        let validator = OidcValidator::new("https://idp.example.com".into(), "my-client".into());
        let result = validator.validate_token(&token).unwrap();

        assert_eq!(result.sub, "minimal-user");
        assert!(result.email.is_none());
        assert!(result.groups.is_empty());
        assert!(result.tenant_hint.is_none());
    }
}
