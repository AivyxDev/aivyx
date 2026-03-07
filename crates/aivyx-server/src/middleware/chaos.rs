//! Chaos testing middleware for fault injection.
//!
//! When enabled, this middleware probabilistically injects faults into HTTP
//! responses: random 500 errors, artificial latency, and body corruption.
//! Controlled via `ChaosConfig` which is read from environment variables
//! at startup.

use std::sync::Arc;
use std::task::{Context, Poll};

use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::IntoResponse,
};
use rand::Rng;
use serde::{Deserialize, Serialize};
use tower::{Layer, Service};

/// Configuration for the chaos fault-injection middleware.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChaosConfig {
    /// Whether chaos injection is active.
    pub enabled: bool,
    /// Probability (0.0..1.0) of returning an HTTP 500 error.
    pub http_error_probability: f64,
    /// Probability (0.0..1.0) of injecting artificial latency.
    pub latency_probability: f64,
    /// Duration in milliseconds for injected latency.
    pub latency_ms: u64,
    /// Probability (0.0..1.0) of replacing the response body with empty bytes.
    pub corrupt_body_probability: f64,
}

/// Tower layer that wraps services with chaos fault injection.
#[derive(Clone)]
pub struct ChaosLayer {
    config: Arc<ChaosConfig>,
}

impl ChaosLayer {
    pub fn new(config: Arc<ChaosConfig>) -> Self {
        Self { config }
    }
}

impl<S> Layer<S> for ChaosLayer {
    type Service = ChaosService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ChaosService {
            inner,
            config: self.config.clone(),
        }
    }
}

/// Tower service that probabilistically injects faults.
///
/// Faults are applied in order: HTTP error, latency, body corruption.
/// If an HTTP error is injected the request is short-circuited and the
/// downstream handler is never called.
#[derive(Clone)]
pub struct ChaosService<S> {
    inner: S,
    config: Arc<ChaosConfig>,
}

impl<S> Service<Request<Body>> for ChaosService<S>
where
    S: Service<Request<Body>, Response = axum::response::Response> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = axum::response::Response;
    type Error = S::Error;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let config = self.config.clone();
        let mut inner = self.inner.clone();
        // Swap to ensure readiness is preserved
        std::mem::swap(&mut self.inner, &mut inner);

        Box::pin(async move {
            if !config.enabled {
                return inner.call(req).await;
            }

            // Sample all random decisions upfront so the non-Send ThreadRng
            // does not live across any await point.
            let (inject_error, inject_latency, inject_corrupt) = {
                let mut rng = rand::thread_rng();
                (
                    config.http_error_probability > 0.0
                        && rng.r#gen::<f64>() < config.http_error_probability,
                    config.latency_probability > 0.0
                        && rng.r#gen::<f64>() < config.latency_probability,
                    config.corrupt_body_probability > 0.0
                        && rng.r#gen::<f64>() < config.corrupt_body_probability,
                )
            };

            // Fault: HTTP 500
            if inject_error {
                metrics::counter!("chaos_faults_total", "type" => "http_error").increment(1);
                return Ok((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    axum::Json(serde_json::json!({"error": "chaos: injected server error"})),
                )
                    .into_response());
            }

            // Fault: latency
            if inject_latency {
                metrics::counter!("chaos_faults_total", "type" => "latency").increment(1);
                tokio::time::sleep(std::time::Duration::from_millis(config.latency_ms)).await;
            }

            let response = inner.call(req).await?;

            // Fault: corrupt body
            if inject_corrupt {
                metrics::counter!("chaos_faults_total", "type" => "corrupt_body").increment(1);
                return Ok((response.status(), Body::from("")).into_response());
            }

            Ok(response)
        })
    }
}
