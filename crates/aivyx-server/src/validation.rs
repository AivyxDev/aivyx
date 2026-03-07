//! Shared input validation helpers for HTTP endpoints.
//!
//! Centralises name validation to prevent path traversal and injection attacks.
//! All route modules should use these functions instead of defining their own.

use aivyx_core::AivyxError;

use crate::error::ServerError;

/// Validates a resource name (agent, team, project, schedule, plugin).
///
/// Rejects names that are empty, too long, or contain characters that could
/// enable path traversal or injection attacks.
pub fn validate_name(name: &str) -> Result<(), ServerError> {
    if name.is_empty()
        || name.len() > 64
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name.contains('\0')
        || name.starts_with('.')
        || name != name.trim()
        || name.chars().any(|c| c.is_control())
    {
        return Err(ServerError(AivyxError::Config(format!(
            "invalid resource name: {name}"
        ))));
    }
    Ok(())
}

/// Validates a secret key name (more permissive on length).
///
/// Same checks as `validate_name` but allows up to 128 characters.
pub fn validate_secret_name(name: &str) -> Result<(), ServerError> {
    if name.is_empty()
        || name.len() > 128
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name.contains('\0')
        || name.starts_with('.')
        || name != name.trim()
        || name.chars().any(|c| c.is_control())
    {
        return Err(ServerError(AivyxError::Config(format!(
            "invalid secret name: {name}"
        ))));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_names() {
        assert!(validate_name("my-agent").is_ok());
        assert!(validate_name("agent_v2").is_ok());
        assert!(validate_name("test123").is_ok());
    }

    #[test]
    fn rejects_empty() {
        assert!(validate_name("").is_err());
    }

    #[test]
    fn rejects_path_traversal() {
        assert!(validate_name("../etc/passwd").is_err());
        assert!(validate_name("foo/bar").is_err());
        assert!(validate_name("foo\\bar").is_err());
    }

    #[test]
    fn rejects_leading_dot() {
        assert!(validate_name(".hidden").is_err());
    }

    #[test]
    fn rejects_control_chars() {
        assert!(validate_name("foo\nbar").is_err());
        assert!(validate_name("foo\x00bar").is_err());
    }

    #[test]
    fn rejects_leading_trailing_whitespace() {
        assert!(validate_name(" leading").is_err());
        assert!(validate_name("trailing ").is_err());
    }

    #[test]
    fn rejects_too_long() {
        let long = "a".repeat(65);
        assert!(validate_name(&long).is_err());
    }

    #[test]
    fn secret_name_allows_longer() {
        let name = "a".repeat(128);
        assert!(validate_secret_name(&name).is_ok());
        let too_long = "a".repeat(129);
        assert!(validate_secret_name(&too_long).is_err());
    }
}
