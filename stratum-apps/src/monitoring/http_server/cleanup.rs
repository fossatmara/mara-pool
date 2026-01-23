//! Helper functions for cleaning up stale Prometheus metrics

use super::super::snapshot_metrics::SnapshotMetrics;
use std::collections::HashSet;

/// Generic helper to clean up stale metrics
fn cleanup_stale_metrics<L, F>(old_labels: &HashSet<L>, new_labels: &HashSet<L>, cleanup_fn: F)
where
    L: Eq + std::hash::Hash + Clone,
    F: Fn(&L),
{
    let stale_labels: Vec<_> = old_labels.difference(new_labels).cloned().collect();
    for label in &stale_labels {
        cleanup_fn(label);
    }
}

/// Helper function to clean up stale server metrics
pub(super) fn cleanup_stale_server_metrics(
    old_labels: &HashSet<(String, String)>,
    new_labels: &HashSet<(String, String)>,
    metrics: &SnapshotMetrics,
) {
    cleanup_stale_metrics(old_labels, new_labels, |(channel_id, user)| {
        if let Some(metric) = &metrics.sv2_server_shares_accepted_total {
            let _ = metric.remove_label_values(&[channel_id.as_str(), user.as_str()]);
        }
        if let Some(metric) = &metrics.sv2_server_channel_hashrate {
            let _ = metric.remove_label_values(&[channel_id.as_str(), user.as_str()]);
        }
    });
}

/// Helper function to clean up stale client metrics
pub(super) fn cleanup_stale_client_metrics(
    old_labels: &HashSet<(String, String, String)>,
    new_labels: &HashSet<(String, String, String)>,
    metrics: &SnapshotMetrics,
) {
    cleanup_stale_metrics(old_labels, new_labels, |(client_id, channel_id, user)| {
        if let Some(metric) = &metrics.sv2_client_shares_accepted_total {
            let _ = metric.remove_label_values(&[
                client_id.as_str(),
                channel_id.as_str(),
                user.as_str(),
            ]);
        }
        if let Some(metric) = &metrics.sv2_client_channel_hashrate {
            let _ = metric.remove_label_values(&[
                client_id.as_str(),
                channel_id.as_str(),
                user.as_str(),
            ]);
        }
        if let Some(metric) = &metrics.sv2_client_channel_shares_per_minute {
            let _ = metric.remove_label_values(&[
                client_id.as_str(),
                channel_id.as_str(),
                user.as_str(),
            ]);
        }
    });
}
