//! Event-driven metrics for real-time Prometheus counter increments.
//!
//! This module provides metrics that are incremented at the point where events occur
//! (e.g., share acceptance, share rejection, block discovery) rather than being
//! sampled periodically from snapshots.
//!
//! ## Architecture
//!
//! - **EventMetrics** is passed to business logic components (e.g., ChannelManager)
//! - Events trigger immediate counter increments (e.g., `shares_accepted.inc()`)
//! - Prometheus scrapes these counters directly from the registry
//! - No snapshot cache needed for event metrics
//!
//! ## Consolidated Metrics Design
//!
//! Counters use bounded labels to enable dashboard filtering and API linkage:
//! - `stratum_shares_total{direction,channel_type}` - Shares by direction and type (4 combinations)
//! - `stratum_shares_rejected_total{reason}` - Rejections by reason (7 combinations)
//! - `stratum_messages_total{direction,msg_type}` - Messages by direction and type
//! - `stratum_bytes_total{direction}` - Bytes by direction (2 combinations)
//! - `stratum_shares_by_user_total{direction,user_identity}` - Per-user shares (unbounded, opt-in)
//!
//! SV1 metrics (tproxy only):
//! - `sv1_shares_total` - SV1 share count
//!
//! The `direction` label (server/client) maps to API endpoints:
//! - `direction="server"` → `/api/v1/server/channels`
//! - `direction="client"` → `/api/v1/clients`
//!
//! Per-channel share data is available via the REST API for detailed debugging.

use prometheus::{IntCounter, IntCounterVec, Opts, Registry};

// Re-export direction constants for consistency
pub use super::snapshot_metrics::{channel_type, direction};

/// Bounded message type constants for `stratum_messages_total` metric.
///
/// These cover the core SV2 Mining Protocol messages. Using constants
/// ensures bounded cardinality and consistent labeling across components.
pub mod msg_type {
    // === Mining Protocol Messages ===
    pub const SETUP_CONNECTION: &str = "setup_connection";
    pub const SETUP_CONNECTION_SUCCESS: &str = "setup_connection_success";
    pub const SETUP_CONNECTION_ERROR: &str = "setup_connection_error";
    pub const OPEN_STANDARD_MINING_CHANNEL: &str = "open_standard_mining_channel";
    pub const OPEN_STANDARD_MINING_CHANNEL_SUCCESS: &str = "open_standard_mining_channel_success";
    pub const OPEN_EXTENDED_MINING_CHANNEL: &str = "open_extended_mining_channel";
    pub const OPEN_EXTENDED_MINING_CHANNEL_SUCCESS: &str = "open_extended_mining_channel_success";
    pub const OPEN_MINING_CHANNEL_ERROR: &str = "open_mining_channel_error";
    pub const UPDATE_CHANNEL: &str = "update_channel";
    pub const UPDATE_CHANNEL_ERROR: &str = "update_channel_error";
    pub const CLOSE_CHANNEL: &str = "close_channel";
    pub const SET_EXTRANONCE_PREFIX: &str = "set_extranonce_prefix";
    pub const SUBMIT_SHARES_STANDARD: &str = "submit_shares_standard";
    pub const SUBMIT_SHARES_EXTENDED: &str = "submit_shares_extended";
    pub const SUBMIT_SHARES_SUCCESS: &str = "submit_shares_success";
    pub const SUBMIT_SHARES_ERROR: &str = "submit_shares_error";
    pub const NEW_MINING_JOB: &str = "new_mining_job";
    pub const NEW_EXTENDED_MINING_JOB: &str = "new_extended_mining_job";
    pub const SET_NEW_PREV_HASH: &str = "set_new_prev_hash";
    pub const SET_TARGET: &str = "set_target";
    pub const SET_CUSTOM_MINING_JOB: &str = "set_custom_mining_job";
    pub const SET_CUSTOM_MINING_JOB_SUCCESS: &str = "set_custom_mining_job_success";
    pub const SET_CUSTOM_MINING_JOB_ERROR: &str = "set_custom_mining_job_error";
    pub const RECONNECT: &str = "reconnect";
    pub const SET_GROUP_CHANNEL: &str = "set_group_channel";

    // === Template Distribution Messages ===
    pub const NEW_TEMPLATE: &str = "new_template";
    pub const SET_NEW_PREV_HASH_TP: &str = "set_new_prev_hash_tp";
    pub const SUBMIT_SOLUTION: &str = "submit_solution";
    pub const REQUEST_TRANSACTION_DATA: &str = "request_transaction_data";
    pub const REQUEST_TRANSACTION_DATA_SUCCESS: &str = "request_transaction_data_success";
    pub const REQUEST_TRANSACTION_DATA_ERROR: &str = "request_transaction_data_error";

    // === Common Messages ===
    pub const CHANNEL_ENDPOINT_CHANGED: &str = "channel_endpoint_changed";

    // === Fallback ===
    pub const UNKNOWN: &str = "unknown";
}

/// Bounded message type constants for SV1 messages.
///
/// These map to `json_rpc` method names where applicable.
pub mod sv1_msg_type {
    pub const MINING_CONFIGURE: &str = "mining.configure";
    pub const MINING_SUBSCRIBE: &str = "mining.subscribe";
    pub const MINING_AUTHORIZE: &str = "mining.authorize";
    pub const MINING_SUBMIT: &str = "mining.submit";
    pub const MINING_SET_DIFFICULTY: &str = "mining.set_difficulty";
    pub const MINING_NOTIFY: &str = "mining.notify";
    pub const UNKNOWN: &str = "unknown";
}

/// Reasons why a share may be rejected.
///
/// This enum is used as a label value for `stratum_shares_rejected_total`.
/// The variants are bounded to prevent cardinality explosion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareRejectionReason {
    /// Share nonce is invalid
    InvalidNonce,
    /// Share does not meet difficulty target
    InvalidDifficulty,
    /// Share references an old job that is no longer valid
    StaleShare,
    /// Share has already been submitted
    DuplicateShare,
    /// Share has invalid merkle root
    InvalidMerkleRoot,
    /// Job ID not found
    JobNotFound,
    /// Other/unknown reason
    Other,
}

impl ShareRejectionReason {
    /// Convert to string for use as Prometheus label value
    pub fn as_str(&self) -> &'static str {
        match self {
            ShareRejectionReason::InvalidNonce => "invalid_nonce",
            ShareRejectionReason::InvalidDifficulty => "invalid_difficulty",
            ShareRejectionReason::StaleShare => "stale_share",
            ShareRejectionReason::DuplicateShare => "duplicate_share",
            ShareRejectionReason::InvalidMerkleRoot => "invalid_merkle_root",
            ShareRejectionReason::JobNotFound => "job_not_found",
            ShareRejectionReason::Other => "other",
        }
    }
}

/// Event-driven metrics that are incremented at the point where events occur.
///
/// These metrics are passed to business logic components and incremented
/// immediately when events happen, providing real-time data to Prometheus.
///
/// Uses consolidated metrics with bounded labels:
/// - `direction`: "server" (upstream) or "client" (downstream)
/// - `channel_type`: "extended" or "standard"
/// - `reason`: bounded enum for rejection reasons
///
/// SV1 metrics are separate counters (sv1_shares_total) to avoid polluting
/// the direction enum with tproxy-specific values.
///
/// This enables dashboard filtering by direction and API linkage.
#[derive(Clone)]
pub struct EventMetrics {
    // === SV2 Traffic Counters ===
    /// Shares accepted by direction and channel_type
    /// Labels: direction (server|client), channel_type (extended|standard)
    /// Cardinality: 4
    pub stratum_shares_total: IntCounterVec,

    /// Shares rejected by reason
    /// Labels: reason (bounded enum, 7 values)
    /// Cardinality: 7
    pub stratum_shares_rejected_total: IntCounterVec,

    /// Messages by direction and message type
    /// Labels: direction (server|client), msg_type (bounded enum)
    /// Cardinality: ~30
    pub stratum_messages_total: IntCounterVec,

    /// Bytes transferred by direction
    /// Labels: direction (server|client)
    /// Cardinality: 2
    pub stratum_bytes_total: IntCounterVec,

    // === Per-user metrics (opt-in, high cardinality) ===
    /// Per-user shares (opt-in, high cardinality)
    /// Labels: direction (server|client), user_identity
    /// Cardinality: unbounded
    pub stratum_shares_by_user_total: Option<IntCounterVec>,

    // === SV1 Metrics (tproxy only) ===
    /// SV1 shares accepted
    pub sv1_shares_total: Option<IntCounter>,

    /// SV1 messages by direction and message type
    /// Labels: direction (server|client), msg_type (bounded enum)
    pub sv1_messages_total: Option<IntCounterVec>,

    /// SV1 bytes transferred by direction
    /// Labels: direction (server|client)
    pub sv1_bytes_total: Option<IntCounterVec>,

    // === Pool-specific Metrics ===
    /// Templates received from template provider (pool only)
    pub pool_templates_received_total: Option<IntCounter>,

    /// Blocks found by the pool (pool only)
    pub pool_blocks_found_total: Option<IntCounter>,
}

impl EventMetrics {
    /// Create new EventMetrics and register them with the provided Prometheus registry.
    ///
    /// # Arguments
    ///
    /// * `registry` - Prometheus registry to register metrics with
    /// * `_enable_server_metrics` - Unused, kept for API compatibility
    /// * `_enable_clients_metrics` - Unused, kept for API compatibility
    pub fn new(
        registry: &Registry,
        _enable_server_metrics: bool,
        _enable_clients_metrics: bool,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        // Shares accepted by direction and channel_type
        let stratum_shares_total = IntCounterVec::new(
            Opts::new(
                "stratum_shares_total",
                "Shares accepted by direction and channel_type",
            ),
            &["direction", "channel_type"],
        )?;
        registry.register(Box::new(stratum_shares_total.clone()))?;

        // Shares rejected by reason
        let stratum_shares_rejected_total = IntCounterVec::new(
            Opts::new("stratum_shares_rejected_total", "Shares rejected by reason"),
            &["reason"],
        )?;
        registry.register(Box::new(stratum_shares_rejected_total.clone()))?;

        stratum_shares_rejected_total
            .with_label_values(&[ShareRejectionReason::InvalidNonce.as_str()])
            .inc_by(0);
        stratum_shares_rejected_total
            .with_label_values(&[ShareRejectionReason::InvalidDifficulty.as_str()])
            .inc_by(0);
        stratum_shares_rejected_total
            .with_label_values(&[ShareRejectionReason::StaleShare.as_str()])
            .inc_by(0);
        stratum_shares_rejected_total
            .with_label_values(&[ShareRejectionReason::DuplicateShare.as_str()])
            .inc_by(0);
        stratum_shares_rejected_total
            .with_label_values(&[ShareRejectionReason::InvalidMerkleRoot.as_str()])
            .inc_by(0);
        stratum_shares_rejected_total
            .with_label_values(&[ShareRejectionReason::JobNotFound.as_str()])
            .inc_by(0);
        stratum_shares_rejected_total
            .with_label_values(&[ShareRejectionReason::Other.as_str()])
            .inc_by(0);

        // Messages by direction and message type
        let stratum_messages_total = IntCounterVec::new(
            Opts::new(
                "stratum_messages_total",
                "Protocol messages by direction and message type",
            ),
            &["direction", "msg_type"],
        )?;
        registry.register(Box::new(stratum_messages_total.clone()))?;

        // Bytes transferred by direction
        let stratum_bytes_total = IntCounterVec::new(
            Opts::new("stratum_bytes_total", "Bytes transferred by direction"),
            &["direction"],
        )?;
        registry.register(Box::new(stratum_bytes_total.clone()))?;

        // Warm up the most common bounded series so dashboards can discover them even
        // before the first share/message/byte event happens.
        stratum_shares_total
            .with_label_values(&[direction::SERVER, channel_type::EXTENDED])
            .inc_by(0);
        stratum_shares_total
            .with_label_values(&[direction::SERVER, channel_type::STANDARD])
            .inc_by(0);
        stratum_shares_total
            .with_label_values(&[direction::CLIENT, channel_type::EXTENDED])
            .inc_by(0);
        stratum_shares_total
            .with_label_values(&[direction::CLIENT, channel_type::STANDARD])
            .inc_by(0);

        stratum_bytes_total
            .with_label_values(&[direction::SERVER])
            .inc_by(0);
        stratum_bytes_total
            .with_label_values(&[direction::CLIENT])
            .inc_by(0);

        // A default message type keeps the metric family visible even before
        // proper msg_type mapping is wired up end-to-end.
        stratum_messages_total
            .with_label_values(&[direction::SERVER, "unknown"])
            .inc_by(0);
        stratum_messages_total
            .with_label_values(&[direction::CLIENT, "unknown"])
            .inc_by(0);

        // SV1 metrics
        let sv1_shares_total = IntCounter::new("sv1_shares_total", "SV1 shares accepted")?;
        registry.register(Box::new(sv1_shares_total.clone()))?;

        let sv1_messages_total = IntCounterVec::new(
            Opts::new(
                "sv1_messages_total",
                "SV1 protocol messages by direction and message type",
            ),
            &["direction", "msg_type"],
        )?;
        registry.register(Box::new(sv1_messages_total.clone()))?;

        sv1_messages_total
            .with_label_values(&[direction::SERVER, sv1_msg_type::UNKNOWN])
            .inc_by(0);
        sv1_messages_total
            .with_label_values(&[direction::CLIENT, sv1_msg_type::UNKNOWN])
            .inc_by(0);

        let sv1_bytes_total = IntCounterVec::new(
            Opts::new("sv1_bytes_total", "SV1 bytes transferred by direction"),
            &["direction"],
        )?;
        registry.register(Box::new(sv1_bytes_total.clone()))?;

        sv1_bytes_total
            .with_label_values(&[direction::SERVER])
            .inc_by(0);
        sv1_bytes_total
            .with_label_values(&[direction::CLIENT])
            .inc_by(0);

        // Pool metrics
        let pool_templates_received_total = IntCounter::new(
            "pool_templates_received_total",
            "Templates received from template provider",
        )?;
        registry.register(Box::new(pool_templates_received_total.clone()))?;

        let pool_blocks_found_total =
            IntCounter::new("pool_blocks_found_total", "Blocks found by the pool")?;
        registry.register(Box::new(pool_blocks_found_total.clone()))?;

        Ok(Self {
            stratum_shares_total,
            stratum_shares_rejected_total,
            stratum_messages_total,
            stratum_bytes_total,
            stratum_shares_by_user_total: None, // Opt-in, set via with_user_metrics()
            sv1_shares_total: Some(sv1_shares_total),
            sv1_messages_total: Some(sv1_messages_total),
            sv1_bytes_total: Some(sv1_bytes_total),
            pool_templates_received_total: Some(pool_templates_received_total),
            pool_blocks_found_total: Some(pool_blocks_found_total),
        })
    }

    /// Enable per-user share tracking (high cardinality).
    ///
    /// This should only be called if user-level metrics are needed for billing/SLA.
    pub fn with_user_metrics(
        mut self,
        registry: &Registry,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let stratum_shares_by_user_total = IntCounterVec::new(
            Opts::new(
                "stratum_shares_by_user_total",
                "Per-user shares by direction and user_identity",
            ),
            &["direction", "user_identity"],
        )?;
        registry.register(Box::new(stratum_shares_by_user_total.clone()))?;
        self.stratum_shares_by_user_total = Some(stratum_shares_by_user_total);
        Ok(self)
    }

    // === SV2 Share Methods ===

    /// Increment share counter for a direction and channel type.
    #[inline]
    pub fn inc_shares(&self, direction: &str, channel_type: &str) {
        self.stratum_shares_total
            .with_label_values(&[direction, channel_type])
            .inc();
    }

    /// Increment shares rejected counter for a specific reason.
    ///
    /// Uses bounded enum labels to prevent cardinality explosion.
    #[inline]
    pub fn inc_shares_rejected(&self, reason: ShareRejectionReason) {
        self.stratum_shares_rejected_total
            .with_label_values(&[reason.as_str()])
            .inc();
    }

    /// Increment per-user shares counter (if enabled).
    #[inline]
    pub fn inc_shares_by_user(&self, direction: &str, user_identity: &str) {
        if let Some(ref metric) = self.stratum_shares_by_user_total {
            metric.with_label_values(&[direction, user_identity]).inc();
        }
    }

    // === Message and Byte Methods ===

    /// Increment message counter for a specific direction and message type.
    #[inline]
    pub fn inc_messages(&self, direction: &str, msg_type: &str) {
        self.stratum_messages_total
            .with_label_values(&[direction, msg_type])
            .inc();
    }

    /// Add bytes to the byte counter for a specific direction.
    #[inline]
    pub fn add_bytes(&self, direction: &str, bytes: u64) {
        self.stratum_bytes_total
            .with_label_values(&[direction])
            .inc_by(bytes);
    }

    // === SV1 Methods (tproxy only) ===

    /// Increment SV1 shares counter (if enabled).
    #[inline]
    pub fn inc_sv1_shares(&self) {
        if let Some(ref metric) = self.sv1_shares_total {
            metric.inc();
        }
    }

    #[inline]
    pub fn inc_sv1_messages(&self, direction: &str, msg_type: &str) {
        if let Some(ref metric) = self.sv1_messages_total {
            metric.with_label_values(&[direction, msg_type]).inc();
        }
    }

    #[inline]
    pub fn add_sv1_bytes(&self, direction: &str, bytes: u64) {
        if let Some(ref metric) = self.sv1_bytes_total {
            metric.with_label_values(&[direction]).inc_by(bytes);
        }
    }

    // === Pool Methods (pool only) ===

    /// Increment templates received counter (if enabled).
    #[inline]
    pub fn inc_templates_received(&self) {
        if let Some(ref metric) = self.pool_templates_received_total {
            metric.inc();
        }
    }

    /// Increment blocks found counter (if enabled).
    #[inline]
    pub fn inc_blocks_found(&self) {
        if let Some(ref metric) = self.pool_blocks_found_total {
            metric.inc();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shares_by_direction_and_type() {
        let registry = Registry::new();
        let metrics = EventMetrics::new(&registry, true, true).unwrap();

        metrics.inc_shares(direction::SERVER, channel_type::EXTENDED);
        metrics.inc_shares(direction::SERVER, channel_type::EXTENDED);
        metrics.inc_shares(direction::CLIENT, channel_type::STANDARD);

        let server_extended = metrics
            .stratum_shares_total
            .with_label_values(&[direction::SERVER, channel_type::EXTENDED])
            .get();
        let client_standard = metrics
            .stratum_shares_total
            .with_label_values(&[direction::CLIENT, channel_type::STANDARD])
            .get();

        assert_eq!(server_extended, 2);
        assert_eq!(client_standard, 1);
    }

    #[test]
    fn test_shares_cardinality_bounded() {
        let registry = Registry::new();
        let metrics = EventMetrics::new(&registry, true, true).unwrap();

        // Use all 4 combinations
        metrics.inc_shares(direction::SERVER, channel_type::EXTENDED);
        metrics.inc_shares(direction::SERVER, channel_type::STANDARD);
        metrics.inc_shares(direction::CLIENT, channel_type::EXTENDED);
        metrics.inc_shares(direction::CLIENT, channel_type::STANDARD);

        // Should have exactly 4 label combinations
        let metric_families = registry.gather();
        let accepted_family = metric_families
            .iter()
            .find(|f| f.get_name() == "stratum_shares_total")
            .unwrap();

        assert_eq!(accepted_family.get_metric().len(), 4);
    }

    #[test]
    fn test_rejection_counter_by_reason() {
        let registry = Registry::new();
        let metrics = EventMetrics::new(&registry, true, true).unwrap();

        metrics.inc_shares_rejected(ShareRejectionReason::StaleShare);
        metrics.inc_shares_rejected(ShareRejectionReason::StaleShare);
        metrics.inc_shares_rejected(ShareRejectionReason::InvalidNonce);

        let stale = metrics
            .stratum_shares_rejected_total
            .with_label_values(&["stale_share"])
            .get();
        let invalid = metrics
            .stratum_shares_rejected_total
            .with_label_values(&["invalid_nonce"])
            .get();

        assert_eq!(stale, 2);
        assert_eq!(invalid, 1);
    }

    #[test]
    fn test_rejection_reason_cardinality_bounded() {
        let reasons = [
            ShareRejectionReason::InvalidNonce,
            ShareRejectionReason::InvalidDifficulty,
            ShareRejectionReason::StaleShare,
            ShareRejectionReason::DuplicateShare,
            ShareRejectionReason::InvalidMerkleRoot,
            ShareRejectionReason::JobNotFound,
            ShareRejectionReason::Other,
        ];

        let registry = Registry::new();
        let metrics = EventMetrics::new(&registry, true, true).unwrap();

        for reason in reasons {
            metrics.inc_shares_rejected(reason);
        }

        // Should have exactly 7 label combinations
        let metric_families = registry.gather();
        let rejected_family = metric_families
            .iter()
            .find(|f| f.get_name() == "stratum_shares_rejected_total")
            .unwrap();

        assert_eq!(rejected_family.get_metric().len(), 7);
    }

    #[test]
    fn test_counters_are_monotonic() {
        let registry = Registry::new();
        let metrics = EventMetrics::new(&registry, true, true).unwrap();

        let mut prev = 0;
        for _ in 0..100 {
            metrics.inc_shares(direction::SERVER, channel_type::EXTENDED);
            let current = metrics
                .stratum_shares_total
                .with_label_values(&[direction::SERVER, channel_type::EXTENDED])
                .get();
            assert!(current > prev, "Counter must be monotonically increasing");
            prev = current;
        }
    }

    #[test]
    fn test_messages_and_bytes() {
        let registry = Registry::new();
        let metrics = EventMetrics::new(&registry, true, true).unwrap();

        metrics.inc_messages(direction::CLIENT, "submit_shares");
        metrics.inc_messages(direction::CLIENT, "submit_shares");
        metrics.inc_messages(direction::SERVER, "new_mining_job");
        metrics.add_bytes(direction::CLIENT, 100);
        metrics.add_bytes(direction::CLIENT, 50);

        let client_submit = metrics
            .stratum_messages_total
            .with_label_values(&[direction::CLIENT, "submit_shares"])
            .get();
        let server_job = metrics
            .stratum_messages_total
            .with_label_values(&[direction::SERVER, "new_mining_job"])
            .get();
        let client_bytes = metrics
            .stratum_bytes_total
            .with_label_values(&[direction::CLIENT])
            .get();

        assert_eq!(client_submit, 2);
        assert_eq!(server_job, 1);
        assert_eq!(client_bytes, 150);
    }
}
