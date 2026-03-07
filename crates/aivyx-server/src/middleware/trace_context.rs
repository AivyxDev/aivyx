//! W3C Trace Context propagation middleware.
//!
//! Extracts the `traceparent` header from incoming requests and injects it
//! into the tracing span. If no `traceparent` is present, generates a new
//! trace ID. The `traceparent` header is always set on the response.
//!
//! Format: `00-{trace_id}-{span_id}-{flags}`
//! See: https://www.w3.org/TR/trace-context/

use axum::body::Body;
use axum::http::Request;
use axum::middleware::Next;
use axum::response::Response;

/// Header name for W3C Trace Context.
const TRACEPARENT: &str = "traceparent";

/// Parse a `traceparent` header value.
///
/// Format: `{version}-{trace_id}-{parent_id}-{trace_flags}`
/// Example: `00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01`
///
/// Returns `(trace_id, parent_id, flags)` or `None` if invalid.
pub fn parse_traceparent(value: &str) -> Option<(String, String, String)> {
    let parts: Vec<&str> = value.split('-').collect();
    if parts.len() != 4 || parts[0] != "00" {
        return None;
    }

    let trace_id = parts[1];
    let parent_id = parts[2];
    let flags = parts[3];

    // Validate lengths: trace_id=32 hex, parent_id=16 hex, flags=2 hex
    if trace_id.len() != 32 || parent_id.len() != 16 || flags.len() != 2 {
        return None;
    }

    // Validate hex
    if !trace_id.chars().all(|c| c.is_ascii_hexdigit())
        || !parent_id.chars().all(|c| c.is_ascii_hexdigit())
        || !flags.chars().all(|c| c.is_ascii_hexdigit())
    {
        return None;
    }

    Some((
        trace_id.to_string(),
        parent_id.to_string(),
        flags.to_string(),
    ))
}

/// Generate a new `traceparent` header value.
pub fn generate_traceparent() -> String {
    let trace_id = uuid::Uuid::new_v4().to_string().replace('-', "");
    let span_id = &uuid::Uuid::new_v4().to_string().replace('-', "")[..16];
    format!("00-{trace_id}-{span_id}-01")
}

/// Middleware that propagates W3C Trace Context headers.
///
/// - Extracts `traceparent` from the request (or generates a new one)
/// - Adds the trace ID to the current tracing span
/// - Sets `traceparent` on the response
pub async fn trace_context_middleware(req: Request<Body>, next: Next) -> Response {
    let traceparent = req
        .headers()
        .get(TRACEPARENT)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            parse_traceparent(v).map(|(tid, pid, flags)| format!("00-{tid}-{pid}-{flags}"))
        })
        .unwrap_or_else(generate_traceparent);

    // Extract trace_id for the span
    let trace_id = traceparent
        .split('-')
        .nth(1)
        .unwrap_or("unknown")
        .to_string();

    tracing::Span::current().record("trace_id", &tracing::field::display(&trace_id));

    let mut response = next.run(req).await;

    // Inject traceparent into response
    if let Ok(value) = traceparent.parse() {
        response.headers_mut().insert(TRACEPARENT, value);
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_traceparent() {
        let tp = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        let (trace_id, parent_id, flags) = parse_traceparent(tp).unwrap();
        assert_eq!(trace_id, "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(parent_id, "00f067aa0ba902b7");
        assert_eq!(flags, "01");
    }

    #[test]
    fn parse_invalid_traceparent_wrong_version() {
        assert!(parse_traceparent("01-abc-def-00").is_none());
    }

    #[test]
    fn parse_invalid_traceparent_wrong_length() {
        assert!(parse_traceparent("00-short-short-00").is_none());
    }

    #[test]
    fn parse_invalid_traceparent_non_hex() {
        let tp = "00-zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz-00f067aa0ba902b7-01";
        assert!(parse_traceparent(tp).is_none());
    }

    #[test]
    fn generate_traceparent_is_valid() {
        let tp = generate_traceparent();
        assert!(parse_traceparent(&tp).is_some());
        assert!(tp.starts_with("00-"));
    }

    #[test]
    fn generate_traceparent_uniqueness() {
        let tp1 = generate_traceparent();
        let tp2 = generate_traceparent();
        assert_ne!(tp1, tp2);
    }
}
