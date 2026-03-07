//! Security headers middleware.
//!
//! Adds standard security headers to all responses: `X-Content-Type-Options`,
//! `X-Frame-Options`, `Strict-Transport-Security`, and `Content-Security-Policy`.
//! CORS is configured separately via `tower-http::CorsLayer`.

use axum::http::HeaderValue;
use axum::http::header;
use axum::middleware::Next;
use axum::response::Response;

/// Middleware function that sets security headers on all responses.
pub async fn security_headers(
    request: axum::http::Request<axum::body::Body>,
    next: Next,
) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();

    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::STRICT_TRANSPORT_SECURITY,
        HeaderValue::from_static("max-age=63072000; includeSubDomains; preload"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));

    response
}
