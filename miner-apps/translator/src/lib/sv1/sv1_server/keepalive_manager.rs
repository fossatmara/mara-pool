//! Job Keepalive Manager for Stratum v1 Miners
//!
//! This module implements a keepalive mechanism that prevents SV1 miners from disconnecting
//! due to not receiving new work within their internal timeout period (typically 120 seconds
//! in miners like cpuminer).
//!
//! The keepalive manager periodically checks all connected downstreams and sends a new job
//! with an incremented job_id and fresh ntime to any downstream that hasn't received a job
//! within the configured keepalive interval.

use crate::sv1::sv1_server::data::Sv1ServerData;
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use stratum_apps::{
    custom_mutex::Mutex,
    stratum_core::sv1_api::{json_rpc, server_to_client},
    utils::types::{ChannelId, DownstreamId},
};
use tokio::sync::broadcast;
use tracing::{debug, info, trace, warn};

/// Manages job keepalive for SV1 miners.
///
/// This struct is responsible for ensuring that SV1 miners receive periodic job updates
/// even when no new templates arrive from upstream. This prevents miners from timing out
/// and disconnecting due to their internal timeout mechanisms.
pub struct KeepaliveManager {
    /// Counter for generating unique job IDs for keepalive jobs.
    /// Starts from a high number to avoid collision with upstream job IDs.
    keepalive_job_id_counter: AtomicU64,
}

impl KeepaliveManager {
    /// Creates a new KeepaliveManager instance.
    pub fn new() -> Self {
        Self {
            // Start from a high number to avoid collision with upstream job IDs
            keepalive_job_id_counter: AtomicU64::new(1_000_000),
        }
    }

    /// Spawns the keepalive loop that periodically checks and sends keepalive jobs.
    ///
    /// This method starts a background loop that:
    /// 1. Periodically checks all connected downstreams (every `check_interval_secs`)
    /// 2. For each downstream that hasn't received a job within `keepalive_interval_secs`,
    ///    sends a new job with incremented job_id and fresh ntime
    ///
    /// # Arguments
    /// * `sv1_server_data` - Shared server data containing downstream connections
    /// * `sv1_server_to_downstream_sender` - Broadcast channel for sending messages to downstreams
    /// * `keepalive_interval_secs` - Maximum time (in seconds) between jobs for a downstream.
    ///                                Set to 0 to disable keepalive.
    /// * `check_interval_secs` - How often (in seconds) to check all downstreams.
    ///                           Recommended: 5 seconds.
    pub async fn spawn_keepalive_loop(
        self: Arc<Self>,
        sv1_server_data: Arc<Mutex<Sv1ServerData>>,
        sv1_server_to_downstream_sender: broadcast::Sender<(
            ChannelId,
            Option<DownstreamId>,
            json_rpc::Message,
        )>,
        keepalive_interval_secs: u64,
        check_interval_secs: u64,
    ) {
        if keepalive_interval_secs == 0 {
            info!("Job keepalive disabled (interval = 0)");
            // Sleep forever to keep the future pending without consuming resources
            tokio::time::sleep(Duration::MAX).await;
            return;
        }

        info!(
            "Job keepalive enabled: interval={}s, check_interval={}s",
            keepalive_interval_secs, check_interval_secs
        );

        let keepalive_duration = Duration::from_secs(keepalive_interval_secs);
        let mut ticker = tokio::time::interval(Duration::from_secs(check_interval_secs));

        loop {
            ticker.tick().await;
            self.check_and_send_keepalives(
                &sv1_server_data,
                &sv1_server_to_downstream_sender,
                keepalive_duration,
            )
            .await;
        }
    }

    /// Checks all downstreams and sends keepalive jobs to those that need it.
    async fn check_and_send_keepalives(
        &self,
        sv1_server_data: &Arc<Mutex<Sv1ServerData>>,
        sender: &broadcast::Sender<(ChannelId, Option<DownstreamId>, json_rpc::Message)>,
        keepalive_duration: Duration,
    ) {
        // Get list of downstreams that need keepalive
        let downstreams_needing_keepalive: Vec<(DownstreamId, ChannelId, server_to_client::Notify<'static>)> =
            sv1_server_data.super_safe_lock(|data| {
                let mut result = Vec::new();

                for (downstream_id, downstream) in &data.downstreams {
                    let needs_keepalive = downstream.downstream_data.super_safe_lock(|d| {
                        // Check if handshake is complete - don't send keepalive during handshake
                        let handshake_complete = d.sv1_handshake_complete.load(Ordering::SeqCst);
                        if !handshake_complete {
                            return None;
                        }

                        // Check if keepalive is needed
                        if d.last_job_sent.elapsed() < keepalive_duration {
                            return None;
                        }

                        // Get the last notify and channel_id for keepalive
                        match (d.channel_id, d.last_notify_for_keepalive.clone()) {
                            (Some(channel_id), Some(notify)) => Some((channel_id, notify)),
                            _ => None,
                        }
                    });

                    if let Some((channel_id, notify)) = needs_keepalive {
                        result.push((*downstream_id, channel_id, notify));
                    }
                }

                result
            });

        // Send keepalive to each downstream that needs it
        for (downstream_id, channel_id, notify) in downstreams_needing_keepalive {
            self.send_keepalive_to_downstream(
                downstream_id,
                channel_id,
                notify,
                sv1_server_data,
                sender,
            )
            .await;
        }
    }

    /// Sends a keepalive job to a specific downstream.
    ///
    /// This method:
    /// 1. Increments the job_id to ensure the miner treats it as new work
    /// 2. Updates ntime to current time for fresh search space
    /// 3. Sets clean_jobs to false to avoid disrupting active mining
    async fn send_keepalive_to_downstream(
        &self,
        downstream_id: DownstreamId,
        channel_id: ChannelId,
        mut notify: server_to_client::Notify<'static>,
        sv1_server_data: &Arc<Mutex<Sv1ServerData>>,
        sender: &broadcast::Sender<(ChannelId, Option<DownstreamId>, json_rpc::Message)>,
    ) {
        // Generate a new unique job_id
        let new_job_id = self.keepalive_job_id_counter.fetch_add(1, Ordering::SeqCst);
        let job_id_string = format!("keepalive_{}", new_job_id);

        // Update ntime to current time for fresh search space
        let current_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as u32;

        // Modify the notify for keepalive
        // Note: job_id in Notify is a Cow<'static, str>, so we need to handle it properly
        notify.job_id = job_id_string.clone().into();
        notify.time = current_time.into();
        notify.clean_jobs = false; // Don't disrupt active mining

        // Convert to JSON-RPC message
        let notify_msg: json_rpc::Message = notify.clone().into();

        debug!(
            downstream_id,
            channel_id,
            job_id = %job_id_string,
            ntime = current_time,
            "Sending keepalive job with fresh ntime to downstream"
        );

        // Send the keepalive job
        match sender.send((channel_id, Some(downstream_id), notify_msg)) {
            Ok(_) => {
                // Update the downstream's last_job_sent and last_notify_for_keepalive
                sv1_server_data.super_safe_lock(|data| {
                    if let Some(downstream) = data.downstreams.get(&downstream_id) {
                        downstream.downstream_data.super_safe_lock(|d| {
                            d.last_job_sent = Instant::now();
                            d.last_notify_for_keepalive = Some(notify);
                        });
                    }
                });
                trace!(
                    downstream_id,
                    "Successfully sent keepalive job and updated tracking"
                );
            }
            Err(e) => {
                warn!(
                    downstream_id,
                    error = ?e,
                    "Failed to send keepalive job to downstream"
                );
            }
        }
    }
}

impl Default for KeepaliveManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keepalive_manager_creation() {
        let manager = KeepaliveManager::new();
        // First job_id should be 1_000_000
        let first_id = manager
            .keepalive_job_id_counter
            .fetch_add(1, Ordering::SeqCst);
        assert_eq!(first_id, 1_000_000);

        // Second should be 1_000_001
        let second_id = manager
            .keepalive_job_id_counter
            .fetch_add(1, Ordering::SeqCst);
        assert_eq!(second_id, 1_000_001);
    }

    #[test]
    fn test_keepalive_manager_default() {
        let manager = KeepaliveManager::default();
        let first_id = manager
            .keepalive_job_id_counter
            .load(Ordering::SeqCst);
        assert_eq!(first_id, 1_000_000);
    }
}
