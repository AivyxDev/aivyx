//! Deterministic session ID derivation for channel conversations.
//!
//! Maps `(platform, user_id)` to a stable [`SessionId`] so that the same user
//! on the same platform always resumes the same conversation.

use aivyx_core::SessionId;
use sha2::{Digest, Sha256};

/// Derive a deterministic [`SessionId`] from a platform and user identifier.
///
/// Uses `SHA-256("channel-session:" || platform || ":" || user_id)` to produce
/// a stable UUID-format ID. The existing `SessionStore` works unchanged
/// because it keys by `SessionId` regardless of how the ID was generated.
///
/// # Examples
///
/// ```
/// # use aivyx_server::channels::session::derive_channel_session_id;
/// let id1 = derive_channel_session_id("telegram", "123456");
/// let id2 = derive_channel_session_id("telegram", "123456");
/// assert_eq!(id1, id2); // deterministic
///
/// let id3 = derive_channel_session_id("email", "user@example.com");
/// assert_ne!(id1, id3); // different platform/user → different ID
/// ```
pub fn derive_channel_session_id(platform: &str, user_id: &str) -> SessionId {
    let mut hasher = Sha256::new();
    hasher.update(b"channel-session:");
    hasher.update(platform.as_bytes());
    hasher.update(b":");
    hasher.update(user_id.as_bytes());
    let hash: [u8; 32] = hasher.finalize().into();

    // Use first 16 bytes to build a UUID (version 4 / variant RFC4122 format)
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&hash[..16]);

    let uuid = uuid::Builder::from_bytes(bytes)
        .with_variant(uuid::Variant::RFC4122)
        .with_version(uuid::Version::Random)
        .into_uuid();

    // SessionId implements FromStr via newtype_id! macro
    uuid.to_string()
        .parse::<SessionId>()
        .expect("valid UUID string should parse to SessionId")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_same_input() {
        let a = derive_channel_session_id("telegram", "123456");
        let b = derive_channel_session_id("telegram", "123456");
        assert_eq!(a, b);
    }

    #[test]
    fn different_platform_different_id() {
        let tg = derive_channel_session_id("telegram", "123456");
        let em = derive_channel_session_id("email", "123456");
        assert_ne!(tg, em);
    }

    #[test]
    fn different_user_different_id() {
        let a = derive_channel_session_id("telegram", "111");
        let b = derive_channel_session_id("telegram", "222");
        assert_ne!(a, b);
    }

    #[test]
    fn id_has_valid_uuid_format() {
        let id = derive_channel_session_id("telegram", "test");
        let s = id.to_string();
        // UUID format: 8-4-4-4-12
        assert_eq!(s.len(), 36);
        assert!(uuid::Uuid::parse_str(&s).is_ok());
    }
}
