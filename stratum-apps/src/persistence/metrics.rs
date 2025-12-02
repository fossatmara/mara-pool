use crate::persistence::{PersistenceBackend, PersistenceEvent};
use crate::task_manager::TaskManager;
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(feature = "metrics")]
use axum::{
    http::{header, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
#[cfg(feature = "metrics")]
use prometheus::{Encoder, IntCounterVec, Opts, TextEncoder};
#[cfg(feature = "metrics")]
use std::net::SocketAddr;

// ============================================================================
// Prometheus Metric Definitions
// ============================================================================

#[cfg(feature = "metrics")]
lazy_static::lazy_static! {
    /// Counter for valid shares submitted by miners
    /// Labels: user_identity, template_id
    static ref SHARES_VALID_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new("pool_shares_valid_total", "Total number of valid shares submitted")
            .namespace("stratum"),
        &["user_identity", "template_id"]
    ).expect("Failed to create SHARES_VALID_TOTAL metric");

    /// Counter for invalid shares submitted by miners
    /// Labels: user_identity, error_code
    static ref SHARES_INVALID_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new("pool_shares_invalid_total", "Total number of invalid shares submitted")
            .namespace("stratum"),
        &["user_identity", "error_code"]
    ).expect("Failed to create SHARES_INVALID_TOTAL metric");

    /// Counter for blocks found
    /// Labels: user_identity
    static ref BLOCKS_FOUND_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new("pool_blocks_found_total", "Total number of blocks found")
            .namespace("stratum"),
        &["user_identity"]
    ).expect("Failed to create BLOCKS_FOUND_TOTAL metric");
}

/// Register all metrics with the default Prometheus registry
#[cfg(feature = "metrics")]
fn register_metrics() {
    let registry = prometheus::default_registry();

    // Register each metric, ignoring errors if already registered
    let _ = registry.register(Box::new(SHARES_VALID_TOTAL.clone()));
    let _ = registry.register(Box::new(SHARES_INVALID_TOTAL.clone()));
    let _ = registry.register(Box::new(BLOCKS_FOUND_TOTAL.clone()));

    tracing::debug!("Prometheus metrics registered");
}

#[derive(Debug, Clone)]
pub struct MetricsBackend {
    // We might want to store these for introspection or if we move server logic here later
    _resource_path: PathBuf,
    _port: u16,
}

impl MetricsBackend {
    pub fn new(
        resource_path: PathBuf,
        port: u16,
        task_manager: Arc<TaskManager>,
    ) -> Result<Self, crate::persistence::Error> {
        #[cfg(feature = "metrics")]
        {
            // Register all Prometheus metrics
            register_metrics();

            // Start the HTTP server for metrics
            let addr: SocketAddr = format!("0.0.0.0:{}", port)
                .parse()
                .map_err(|e| crate::persistence::Error::Custom(format!("Invalid port: {}", e)))?;

            task_manager.spawn(async move {
                if let Err(e) = start_metrics_server(addr).await {
                    tracing::error!("Metrics server failed: {}", e);
                }
            });

            tracing::info!("MetricsBackend initialized with metrics on port {}", port);
        }

        Ok(Self {
            _resource_path: resource_path,
            _port: port,
        })
    }
}

impl PersistenceBackend for MetricsBackend {
    fn persist_event(&self, event: PersistenceEvent) {
        #[cfg(feature = "metrics")]
        match event {
            PersistenceEvent::Share(share) => {
                let user = share.user_identity.as_str();
                let template_id = share
                    .template_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "unknown".to_string());

                if share.is_valid {
                    // Increment valid shares counter
                    SHARES_VALID_TOTAL
                        .with_label_values(&[user, &template_id])
                        .inc();

                    // Check if this share found a block
                    if share.is_block_found {
                        BLOCKS_FOUND_TOTAL.with_label_values(&[user]).inc();

                        tracing::info!(user = user, template_id = template_id, "Block found!");
                    }

                    tracing::trace!(
                        user = user,
                        template_id = template_id,
                        share_work = share.share_work,
                        "Valid share recorded"
                    );
                } else {
                    // Increment invalid shares counter
                    let error_code = share.error_code.as_deref().unwrap_or("unknown");

                    SHARES_INVALID_TOTAL
                        .with_label_values(&[user, error_code])
                        .inc();

                    tracing::trace!(
                        user = user,
                        error_code = error_code,
                        "Invalid share recorded"
                    );
                }
            }
        }
        #[cfg(not(feature = "metrics"))]
        {
            let _ = event;
        }
    }

    fn flush(&self) {}
    fn shutdown(&self) {}
}

// ============================================================================
// HTTP Server for Prometheus Metrics
// ============================================================================

/// HTTP handler for the `/metrics` endpoint.
///
/// Returns Prometheus metrics in text exposition format.
///
/// # Returns
/// - `200 OK` with metrics in Prometheus format on success
/// - `500 Internal Server Error` if metric encoding fails
#[cfg(feature = "metrics")]
pub async fn metrics_handler() -> impl IntoResponse {
    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();
    let mut buffer = Vec::new();

    match encoder.encode(&metric_families, &mut buffer) {
        Ok(_) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, prometheus::TEXT_FORMAT)],
            buffer,
        ),
        Err(e) => {
            tracing::error!("Failed to encode metrics: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                [(header::CONTENT_TYPE, "text/plain")],
                format!("Error encoding metrics: {}", e).into_bytes(),
            )
        }
    }
}

/// Start the metrics HTTP server.
///
/// This function spawns an HTTP server that serves Prometheus metrics at the `/metrics` endpoint.
/// The server runs indefinitely until the process is terminated.
///
/// # Arguments
/// * `addr` - The socket address to bind to (e.g., `0.0.0.0:9090`)
///
/// # Returns
/// An error if the server fails to start or encounters a fatal error.
///
/// # Example
/// ```ignore
/// use std::net::SocketAddr;
/// use stratum_apps::persistence::start_metrics_server;
///
/// #[tokio::main]
/// async fn main() {
///     let addr: SocketAddr = "0.0.0.0:9090".parse().unwrap();
///     if let Err(e) = start_metrics_server(addr).await {
///         eprintln!("Metrics server error: {}", e);
///     }
/// }
/// ```
#[cfg(feature = "metrics")]
pub async fn start_metrics_server(addr: SocketAddr) -> Result<(), String> {
    tracing::info!("Starting metrics server on {}", addr);

    let app = Router::new().route("/metrics", get(metrics_handler));

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("Failed to bind metrics server to {}: {}", addr, e))?;

    tracing::info!("Metrics server listening on {}", addr);

    axum::serve(listener, app)
        .await
        .map_err(|e| format!("Metrics server error: {}", e))?;

    Ok(())
}

#[cfg(all(test, feature = "metrics"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_metrics_handler() {
        let response = metrics_handler().await.into_response();

        // Should return 200 OK
        assert_eq!(response.status(), StatusCode::OK);

        // Should have correct content type
        let content_type = response.headers().get(header::CONTENT_TYPE);
        assert!(content_type.is_some());

        let content_type_str = content_type.unwrap().to_str().unwrap();
        assert!(content_type_str.contains("text/plain"));
    }

    #[tokio::test]
    async fn test_metrics_server_startup() {
        // Test that server can bind to a random port
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();

        // Spawn server in background
        let server_handle = tokio::spawn(async move { start_metrics_server(addr).await });

        // Give it a moment to start
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Abort the server
        server_handle.abort();

        // Test passes if we got here without panicking
    }
}
