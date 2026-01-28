//! Snapshot-based Prometheus metrics for Stratum monitoring.
//!
//! These metrics are populated from the snapshot cache during periodic refreshes
//! and Prometheus scrapes. They represent point-in-time state (gauges) rather than
//! real-time event counts (counters/histograms in EventMetrics).
//!
//! ## Consolidated Metrics Design
//!
//! Metrics use bounded labels to reduce metric count while preserving topology:
//! - `stratum_channels{direction,channel_type}` - Channel counts (4 combinations: 2 directions × 2 types)
//! - `stratum_hashrate{direction}` - Hashrate by direction (2 combinations)
//! - `stratum_hashrate_by_user{direction,user_identity}` - Per-user hashrate (unbounded, opt-in)
//! - `stratum_connections{direction}` - Connection counts (2 combinations)
//!
//! Direction values: "server", "client" (SV2 only)
//! SV1 metrics use separate `sv1_*` metrics instead of overloading direction.
//!
//! This design enables:
//! - Prometheus recording rules for aggregates
//! - Dashboard filtering by direction
//! - API linkage via direction label
//! - Optional user_identity breakdown (config-controlled)

use prometheus::{Gauge, GaugeVec, Opts, Registry};

/// Direction label values for SV2 metrics
pub mod direction {
    /// Upstream SV2 connection (to pool/JDS)
    pub const SERVER: &str = "server";
    /// Downstream SV2 connection (from miners)
    pub const CLIENT: &str = "client";
}

/// Channel type label values for metrics
pub mod channel_type {
    pub const EXTENDED: &str = "extended";
    pub const STANDARD: &str = "standard";
}

/// Snapshot-based metrics populated from cached monitoring state.
///
/// Uses consolidated metrics with bounded labels:
/// - `direction`: "server" or "client" (SV2 only)
/// - `channel_type`: "extended" or "standard" (for channel counts)
///
/// SV1 metrics are separate gauges (sv1_connections, sv1_hashrate) to avoid
/// polluting the direction enum with tproxy-specific values.
///
/// This reduces metric count while preserving topology for dashboards and API linkage.
#[derive(Clone)]
pub struct SnapshotMetrics {
    pub registry: Registry,

    /// Whether user_identity labels are enabled (high cardinality)
    pub enable_user_identity_labels: bool,

    // === SV2 Consolidated Metrics with Labels ===
    /// Channel counts by direction and channel_type
    /// Labels: direction (server|client), channel_type (extended|standard)
    /// Cardinality: 4 (2 directions × 2 types)
    pub stratum_channels: Option<GaugeVec>,

    /// Hashrate by direction
    /// Labels: direction (server|client)
    /// Cardinality: 2
    pub stratum_hashrate: Option<GaugeVec>,

    /// Per-user hashrate (opt-in, high cardinality)
    /// Labels: direction (server|client), user_identity
    /// Cardinality: unbounded
    pub stratum_hashrate_by_user: Option<GaugeVec>,

    /// Connection counts by direction
    /// Labels: direction (server|client)
    /// Cardinality: 2
    pub stratum_connections: Option<GaugeVec>,

    // === SV1 Metrics (tproxy only) ===
    /// SV1 connection count (downstream only)
    pub sv1_connections: Option<Gauge>,

    /// SV1 hashrate (downstream only)
    pub sv1_hashrate: Option<Gauge>,
}

impl SnapshotMetrics {
    /// Create new snapshot metrics.
    ///
    /// # Arguments
    /// * `enable_server_metrics` - Enable server (upstream) metrics
    /// * `enable_clients_metrics` - Enable client (downstream) metrics
    /// * `enable_sv1_metrics` - Enable SV1 client metrics (tproxy only)
    /// * `enable_user_identity_labels` - Enable per-user hashrate metric (high cardinality)
    pub fn new(
        enable_server_metrics: bool,
        enable_clients_metrics: bool,
        enable_sv1_metrics: bool,
        enable_user_identity_labels: bool,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let registry = Registry::new();

        // SV2 consolidated metrics (registered if server or client metrics enabled)
        let enable_sv2 = enable_server_metrics || enable_clients_metrics;

        let stratum_channels = if enable_sv2 {
            let metric = GaugeVec::new(
                Opts::new(
                    "stratum_channels",
                    "Channel counts by direction and channel_type",
                ),
                &["direction", "channel_type"],
            )?;
            registry.register(Box::new(metric.clone()))?;
            Some(metric)
        } else {
            None
        };

        // Aggregate hashrate by direction (always enabled if SV2 metrics enabled)
        let stratum_hashrate = if enable_sv2 {
            let metric = GaugeVec::new(
                Opts::new("stratum_hashrate", "Hashrate by direction"),
                &["direction"],
            )?;
            registry.register(Box::new(metric.clone()))?;
            Some(metric)
        } else {
            None
        };

        // Per-user hashrate (opt-in, high cardinality)
        let stratum_hashrate_by_user = if enable_sv2 && enable_user_identity_labels {
            let metric = GaugeVec::new(
                Opts::new("stratum_hashrate_by_user", "Per-user hashrate"),
                &["direction", "user_identity"],
            )?;
            registry.register(Box::new(metric.clone()))?;
            Some(metric)
        } else {
            None
        };

        let stratum_connections = if enable_sv2 {
            let metric = GaugeVec::new(
                Opts::new("stratum_connections", "Connection counts by direction"),
                &["direction"],
            )?;
            registry.register(Box::new(metric.clone()))?;
            Some(metric)
        } else {
            None
        };

        // SV1 metrics (tproxy only) - separate gauges, not using direction label
        let sv1_connections = if enable_sv1_metrics {
            let metric = Gauge::new("sv1_connections", "SV1 miner connection count")?;
            registry.register(Box::new(metric.clone()))?;
            Some(metric)
        } else {
            None
        };

        let sv1_hashrate = if enable_sv1_metrics {
            let metric = Gauge::new("sv1_hashrate", "SV1 miner hashrate")?;
            registry.register(Box::new(metric.clone()))?;
            Some(metric)
        } else {
            None
        };

        Ok(Self {
            registry,
            enable_user_identity_labels,
            stratum_channels,
            stratum_hashrate,
            stratum_hashrate_by_user,
            stratum_connections,
            sv1_connections,
            sv1_hashrate,
        })
    }

    pub fn enable_sv1_metrics(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if self.sv1_connections.is_none() {
            let metric = Gauge::new("sv1_connections", "SV1 miner connection count")?;
            self.registry.register(Box::new(metric.clone()))?;
            self.sv1_connections = Some(metric);
        }

        if self.sv1_hashrate.is_none() {
            let metric = Gauge::new("sv1_hashrate", "SV1 miner hashrate")?;
            self.registry.register(Box::new(metric.clone()))?;
            self.sv1_hashrate = Some(metric);
        }

        Ok(())
    }

    // === Helper methods for setting SV2 consolidated metrics ===

    /// Set channel count for a specific direction and channel_type
    pub fn set_channels(&self, direction: &str, channel_type: &str, count: f64) {
        if let Some(ref metric) = self.stratum_channels {
            metric
                .with_label_values(&[direction, channel_type])
                .set(count);
        }
    }

    /// Set aggregate hashrate for a specific direction.
    pub fn set_hashrate(&self, direction: &str, hashrate: f64) {
        if let Some(ref metric) = self.stratum_hashrate {
            metric.with_label_values(&[direction]).set(hashrate);
        }
    }

    /// Set hashrate for a specific direction and user_identity.
    /// Only works when user_identity labels are enabled.
    pub fn set_hashrate_by_user(&self, direction: &str, user_identity: &str, hashrate: f64) {
        if let Some(ref metric) = self.stratum_hashrate_by_user {
            metric
                .with_label_values(&[direction, user_identity])
                .set(hashrate);
        }
    }

    /// Set connection count for a specific direction
    pub fn set_connections(&self, direction: &str, count: f64) {
        if let Some(ref metric) = self.stratum_connections {
            metric.with_label_values(&[direction]).set(count);
        }
    }

    // === Helper methods for SV1 metrics (tproxy only) ===

    /// Set SV1 connection count
    pub fn set_sv1_connections(&self, count: f64) {
        if let Some(ref metric) = self.sv1_connections {
            metric.set(count);
        }
    }

    /// Set SV1 hashrate
    pub fn set_sv1_hashrate(&self, hashrate: f64) {
        if let Some(ref metric) = self.sv1_hashrate {
            metric.set(hashrate);
        }
    }
}
