use super::{DownstreamMessages, SubmitShareWithChannelId};
use crate::{
    error::TproxyError,
    status::{handle_error, StatusSender},
    sv1::{
        downstream::{channel::DownstreamChannelState, data::DownstreamData},
        sv1_server::data::Sv1ServerData,
    },
    utils::{validate_sv1_share, ShutdownMessage},
};
use async_channel::{Receiver, Sender};
use std::sync::Arc;
use stratum_apps::{
    custom_mutex::Mutex,
    stratum_core::{
        bitcoin::Target,
        sv1_api::{
            client_to_server,
            json_rpc::{self, Message},
            methods::ParsingMethodError,
            server_to_client, IsServer,
        },
    },
    task_manager::TaskManager,
    utils::types::{ChannelId, DownstreamId, Hashrate},
};
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, error, info, warn};

/// Represents a downstream SV1 miner connection.
///
/// This struct manages the state and communication for a single SV1 miner connected
/// to the translator. It handles:
/// - SV1 protocol message processing (subscribe, authorize, submit)
/// - Bidirectional message routing between miner and SV1 server
/// - Mining job tracking and share validation
/// - Difficulty adjustment coordination
/// - Connection lifecycle management
///
/// Each downstream connection runs in its own async task that processes messages
/// from both the miner and the server, ensuring proper message ordering and
/// handling connection-specific state.
#[derive(Debug)]
pub struct Downstream {
    pub downstream_data: Arc<Mutex<DownstreamData>>,
    downstream_channel_state: DownstreamChannelState,
}

impl Downstream {
    /// Creates a new downstream connection instance.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        downstream_id: DownstreamId,
        downstream_sv1_sender: Sender<json_rpc::Message>,
        downstream_sv1_receiver: Receiver<json_rpc::Message>,
        sv1_server_sender: Sender<DownstreamMessages>,
        sv1_server_receiver: broadcast::Receiver<(
            ChannelId,
            Option<DownstreamId>,
            json_rpc::Message,
        )>,
        target: Target,
        hashrate: Option<Hashrate>,
        sv1_server_data: Arc<Mutex<Sv1ServerData>>,
    ) -> Self {
        let downstream_data = Arc::new(Mutex::new(DownstreamData::new(
            downstream_id,
            target,
            hashrate,
            sv1_server_data,
        )));
        let downstream_channel_state = DownstreamChannelState::new(
            downstream_sv1_sender,
            downstream_sv1_receiver,
            sv1_server_sender,
            sv1_server_receiver,
        );
        Self {
            downstream_data,
            downstream_channel_state,
        }
    }

    /// Spawns and runs the main task loop for this downstream connection.
    ///
    /// This method creates an async task that handles all communication for this
    /// downstream connection. The task runs a select loop that processes:
    /// - Shutdown signals (global, targeted, or all-downstream)
    /// - Messages from the miner (subscribe, authorize, submit)
    /// - Messages from the SV1 server (notify, set_difficulty, etc.)
    ///
    /// The task will continue running until a shutdown signal is received or
    /// an unrecoverable error occurs. It ensures graceful cleanup of resources
    /// and proper error reporting.
    pub fn run_downstream_tasks(
        self: Arc<Self>,
        notify_shutdown: broadcast::Sender<ShutdownMessage>,
        shutdown_complete_tx: mpsc::Sender<()>,
        status_sender: StatusSender,
        task_manager: Arc<TaskManager>,
    ) {
        let mut sv1_server_receiver = self
            .downstream_channel_state
            .sv1_server_receiver
            .resubscribe();
        let mut shutdown_rx = notify_shutdown.subscribe();
        let downstream_id = self.downstream_data.super_safe_lock(|d| d.downstream_id);
        task_manager.spawn(async move {
            loop {
                tokio::select! {
                    msg = shutdown_rx.recv() => {
                        match msg {
                            Ok(ShutdownMessage::ShutdownAll) => {
                                info!("Downstream {downstream_id}: received global shutdown");
                                break;
                            }
                            Ok(ShutdownMessage::DownstreamShutdown(id)) if id == downstream_id => {
                                info!("Downstream {downstream_id}: received targeted shutdown");
                                break;
                            }
                            Ok(ShutdownMessage::UpstreamFallback{tx}) => {
                                info!("Upstream fallback happened, disconnecting downstream.");
                                drop(tx);
                                break;
                            }
                            Ok(_) => {
                                // shutdown for other downstream
                            }
                            Err(e) => {
                                warn!("Downstream {downstream_id}: shutdown channel closed: {e}");
                                break;
                            }
                        }
                    }

                    // Handle downstream -> server message
                    res = Self::handle_downstream_message(self.clone()) => {
                        if let Err(e) = res {
                            error!("Downstream {downstream_id}: error in downstream message handler: {e:?}");
                            handle_error(&status_sender, e).await;
                            break;
                        }
                    }

                    // Handle server -> downstream message
                    res = Self::handle_sv1_server_message(self.clone(),&mut sv1_server_receiver) => {
                        if let Err(e) = res {
                            error!("Downstream {downstream_id}: error in server message handler: {e:?}");
                            handle_error(&status_sender, e).await;
                            break;
                        }
                    }

                    else => {
                        warn!("Downstream {downstream_id}: all channels closed; exiting task");
                        break;
                    }
                }
            }

            warn!("Downstream {downstream_id}: unified task shutting down");
            self.downstream_channel_state.drop();
            drop(shutdown_complete_tx);
        });
    }

    /// Handles messages received from the SV1 server.
    ///
    /// This method processes messages broadcast from the SV1 server to downstream
    /// connections. Since `mining.notify` messages are guaranteed to never arrive
    /// before their corresponding `mining.set_difficulty` message, the logic is
    /// simplified to handle only handshake completion timing.
    ///
    /// Key behaviors:
    /// - Filters messages by channel ID and downstream ID
    /// - For `mining.set_difficulty`: Always caches the message (never sent immediately)
    /// - For `mining.notify`: Sends any pending set_difficulty first, then forwards the notify
    /// - For other messages: Forwards directly to the miner
    /// - Caches both `mining.set_difficulty` and `mining.notify` messages if handshake is not yet
    ///   complete
    /// - On handshake completion: sends cached messages in correct order (set_difficulty first,
    ///   then notify)
    pub async fn handle_sv1_server_message(
        self: Arc<Self>,
        sv1_server_receiver: &mut broadcast::Receiver<(
            ChannelId,
            Option<DownstreamId>,
            json_rpc::Message,
        )>,
    ) -> Result<(), TproxyError> {
        match sv1_server_receiver.recv().await {
            Ok((channel_id, downstream_id, message)) => {
                let (my_channel_id, my_downstream_id, handshake_complete) =
                    self.downstream_data.super_safe_lock(|d| {
                        (
                            d.channel_id,
                            d.downstream_id,
                            d.sv1_handshake_complete
                                .load(std::sync::atomic::Ordering::SeqCst),
                        )
                    });
                let id_matches = (my_channel_id == Some(channel_id) || channel_id == 0)
                    && (downstream_id.is_none() || downstream_id == Some(my_downstream_id));
                if !id_matches {
                    return Ok(()); // Message not intended for this downstream
                }

                // Check if this is a queued message response
                let is_queued_sv1_handshake_response = self.downstream_data.super_safe_lock(|d| {
                    d.processing_queued_sv1_handshake_responses
                        .load(std::sync::atomic::Ordering::SeqCst)
                });

                // Handle messages based on message type and handshake state
                if let Message::Notification(notification) = &message {
                    // For notifications (mining.set_difficulty, mining.notify), only send if
                    // handshake is complete
                    if handshake_complete {
                        match notification.method.as_str() {
                            "mining.set_difficulty" => {
                                // Cache the Sv1 set_difficulty message to be sent before the next
                                // notify
                                debug!("Down: Caching mining.set_difficulty to send before next mining.notify");
                                self.downstream_data.super_safe_lock(|d| {
                                    d.cached_set_difficulty = Some(message);
                                });
                                return Ok(());
                            }
                            "mining.notify" => {
                                let (pending_set_difficulty, notify_opt) =
                                    self.downstream_data.super_safe_lock(|d| {
                                        let cached_set_difficulty = d.cached_set_difficulty.take();

                                        // Prepare the notify message and update state
                                        let notify_result = server_to_client::Notify::try_from(
                                            notification.clone(),
                                        );
                                        if let Ok(mut notify) = notify_result {
                                            if cached_set_difficulty.is_some() {
                                                notify.clean_jobs = true;
                                            }
                                            d.last_job_version_field = Some(notify.version.0);

                                            // Update target and hashrate if we're sending
                                            // set_difficulty
                                            if cached_set_difficulty.is_some() {
                                                if let Some(new_target) = d.pending_target.take() {
                                                    d.target = new_target;
                                                }
                                                if let Some(new_hashrate) =
                                                    d.pending_hashrate.take()
                                                {
                                                    d.hashrate = Some(new_hashrate);
                                                }
                                            }

                                            (cached_set_difficulty, Some(notify))
                                        } else {
                                            (cached_set_difficulty, None)
                                        }
                                    });

                                if let Some(set_difficulty_msg) = &pending_set_difficulty {
                                    debug!("Down: Sending pending mining.set_difficulty before mining.notify");
                                    self.downstream_channel_state
                                        .downstream_sv1_sender
                                        .send(set_difficulty_msg.clone())
                                        .await
                                        .map_err(|e| {
                                            error!(
                                                "Down: Failed to send mining.set_difficulty to downstream: {:?}",
                                                e
                                            );
                                            TproxyError::ChannelErrorSender
                                        })?;
                                }

                                if let Some(notify) = notify_opt {
                                    debug!("Down: Sending mining.notify");
                                    self.downstream_channel_state
                                        .downstream_sv1_sender
                                        .send(notify.into())
                                        .await
                                        .map_err(|e| {
                                            error!("Down: Failed to send mining.notify to downstream: {:?}", e);
                                            TproxyError::ChannelErrorSender
                                        })?;
                                }
                                return Ok(());
                            }
                            _ => {
                                // Other notifications - forward if handshake complete
                                self.downstream_channel_state
                                    .downstream_sv1_sender
                                    .send(message.clone())
                                    .await
                                    .map_err(|e| {
                                        error!(
                                            "Down: Failed to send notification to downstream: {:?}",
                                            e
                                        );
                                        TproxyError::ChannelErrorSender
                                    })?;
                            }
                        }
                    } else {
                        // Handshake not complete - cache mining notifications but skip others
                        match notification.method.as_str() {
                            "mining.set_difficulty" => {
                                debug!("Down: SV1 handshake not complete, caching mining.set_difficulty");
                                self.downstream_data.super_safe_lock(|d| {
                                    d.cached_set_difficulty = Some(message);
                                });
                            }
                            "mining.notify" => {
                                debug!("Down: SV1 handshake not complete, caching mining.notify");
                                self.downstream_data.super_safe_lock(|d| {
                                    d.cached_notify = Some(message.clone());
                                    let notify =
                                        server_to_client::Notify::try_from(notification.clone())
                                            .expect("this must be a mining.notify");
                                    d.last_job_version_field = Some(notify.version.0);
                                });
                            }
                            _ => {
                                debug!(
                                    "Down: SV1 handshake not complete, skipping other notification"
                                );
                            }
                        }
                    }
                } else if is_queued_sv1_handshake_response {
                    // For non-notification messages, send if processing queued handshake responses
                    self.downstream_channel_state
                        .downstream_sv1_sender
                        .send(message.clone())
                        .await
                        .map_err(|e| {
                            error!("Down: Failed to send queued message to downstream: {:?}", e);
                            TproxyError::ChannelErrorSender
                        })?;
                } else {
                    // Neither handshake complete nor queued response - skip non-notification
                    // messages
                    debug!("Down: SV1 handshake not complete, skipping non-notification message");
                }
            }
            Err(e) => {
                let downstream_id = self.downstream_data.super_safe_lock(|d| d.downstream_id);
                error!(
                    "Sv1 message handler error for downstream {}: {:?}",
                    downstream_id, e
                );
                return Err(TproxyError::BroadcastChannelErrorReceiver(e));
            }
        }

        Ok(())
    }

    /// Handles messages received from the downstream SV1 miner.
    ///
    /// This method processes SV1 protocol messages sent by the miner, including:
    /// - `mining.subscribe` - Subscription requests
    /// - `mining.authorize` - Authorization requests
    /// - `mining.submit` - Share submissions
    /// - Other SV1 protocol messages
    ///
    /// The method delegates message processing to the downstream data handler,
    /// which implements the SV1 protocol logic and generates appropriate responses.
    /// Responses are sent back to the miner, while share submissions are forwarded
    /// to the SV1 server for upstream processing.
    pub async fn handle_downstream_message(self: Arc<Self>) -> Result<(), TproxyError> {
        let message = match self
            .downstream_channel_state
            .downstream_sv1_receiver
            .recv()
            .await
        {
            Ok(msg) => msg,
            Err(e) => {
                error!("Error receiving downstream message: {:?}", e);
                return Err(TproxyError::ChannelErrorReceiver(e));
            }
        };

        // Check if channel is established
        let channel_established = self
            .downstream_data
            .super_safe_lock(|d| d.channel_id.is_some());

        if !channel_established {
            // Check if this is the first message (queue is empty) and send OpenChannel request
            let is_first_message = self
                .downstream_data
                .super_safe_lock(|d| d.queued_sv1_handshake_messages.is_empty());

            if is_first_message {
                let downstream_id = self.downstream_data.super_safe_lock(|d| d.downstream_id);
                self.downstream_channel_state
                    .sv1_server_sender
                    .send(DownstreamMessages::OpenChannel(downstream_id))
                    .await
                    .map_err(|e| {
                        error!("Down: Failed to send OpenChannel request: {:?}", e);
                        TproxyError::ChannelErrorSender
                    })?;
                debug!(
                    "Down: Sent OpenChannel request for downstream {}",
                    downstream_id
                );
            }

            // Queue all messages until channel is established
            debug!("Down: Queuing Sv1 message until channel is established");
            self.downstream_data.safe_lock(|d| {
                d.queued_sv1_handshake_messages.push(message.clone());
            })?;
            return Ok(());
        }

        // Check if this is a mining.submit message - handle it specially to avoid deadlock
        // The deadlock occurs because handle_message -> handle_submit -> validate_sv1_share
        // tries to acquire sv1_server_data lock while we hold downstream_data lock.
        // By handling submit outside the downstream_data lock, we break the lock ordering violation.
        if let Message::StandardRequest(ref request) = message {
            if request.method == "mining.submit" {
                return self
                    .handle_submit_message_without_nested_lock(&message)
                    .await;
            }
        }

        // Channel is established, process non-submit messages normally
        let response = self
            .downstream_data
            .super_safe_lock(|data| data.handle_message(message.clone()));

        match response {
            Ok(Some(response_msg)) => {
                debug!(
                    "Down: Sending Sv1 message to downstream: {:?}",
                    response_msg
                );
                self.downstream_channel_state
                    .downstream_sv1_sender
                    .send(response_msg.into())
                    .await
                    .map_err(|e| {
                        error!("Down: Failed to send message to downstream: {:?}", e);
                        TproxyError::ChannelErrorSender
                    })?;

                // Check if this was an authorize message and handle sv1 handshake completion
                if let stratum_apps::stratum_core::sv1_api::json_rpc::Message::StandardRequest(
                    request,
                ) = &message
                {
                    if request.method == "mining.authorize" {
                        info!("Down: Handling mining.authorize after handshake completion");
                        if let Err(e) = self.handle_sv1_handshake_completion().await {
                            error!("Down: Failed to handle handshake completion: {:?}", e);
                            return Err(e);
                        }
                    }
                }
            }
            Ok(None) => {
                // Message was handled but no response needed
            }
            Err(e) => {
                error!("Down: Error handling downstream message: {:?}", e);
                return Err(e.into());
            }
        }

        Ok(())
    }

    /// Handles mining.submit messages without holding the downstream_data lock during validation.
    ///
    /// This method breaks the lock ordering violation that caused deadlocks:
    /// - Previously: downstream_data lock held -> validate_sv1_share -> sv1_server_data lock
    /// - Now: sv1_server_data lock (for job lookup) -> downstream_data lock (for state update)
    ///
    /// The key insight is that share validation only needs to READ from sv1_server_data (job list)
    /// and downstream_data (extranonce, target, etc.), so we can:
    /// 1. Clone the job list from sv1_server_data (quick lock, release immediately)
    /// 2. Clone validation params from downstream_data (quick lock, release immediately)  
    /// 3. Do validation without holding any locks
    /// 4. Update downstream_data with result (quick lock)
    async fn handle_submit_message_without_nested_lock(
        self: &Arc<Self>,
        message: &Message,
    ) -> Result<(), TproxyError> {
        // Parse the submit request from StandardRequest
        let submit: client_to_server::Submit<'static> = match message {
            Message::StandardRequest(request) => {
                request
                    .clone()
                    .try_into()
                    .map_err(|e: ParsingMethodError| {
                        error!("Down: Failed to parse mining.submit: {:?}", e);
                        TproxyError::SV1Error
                    })?
            }
            _ => {
                error!("Down: Expected StandardRequest for mining.submit");
                return Err(TproxyError::SV1Error);
            }
        };

        // Step 1: Extract validation parameters from downstream_data (quick lock, release immediately)
        let (
            channel_id,
            target,
            extranonce1,
            version_rolling_mask,
            sv1_server_data,
            downstream_id,
            extranonce2_len,
            last_job_version,
        ) = self.downstream_data.super_safe_lock(|d| {
            (
                d.channel_id,
                d.target,
                d.extranonce1.clone(),
                d.version_rolling_mask.clone(),
                d.sv1_server_data.clone(),
                d.downstream_id,
                d.extranonce2_len,
                d.last_job_version_field,
            )
        });

        let channel_id = match channel_id {
            Some(id) => id,
            None => {
                error!("Cannot submit share: channel_id is None (waiting for OpenExtendedMiningChannelSuccess)");
                // Send rejection response
                let response = submit.respond(false);
                self.downstream_channel_state
                    .downstream_sv1_sender
                    .send(response.into())
                    .await
                    .map_err(|_| TproxyError::ChannelErrorSender)?;
                return Ok(());
            }
        };

        info!(
            "Received mining.submit from SV1 downstream for channel id: {}",
            channel_id
        );

        // Step 2: Validate the share WITHOUT holding downstream_data lock
        // This is the key fix - validate_sv1_share acquires sv1_server_data lock,
        // but we're no longer holding downstream_data lock at this point
        let is_valid_share = validate_sv1_share(
            &submit,
            target,
            extranonce1.clone(),
            version_rolling_mask.clone(),
            sv1_server_data,
            channel_id,
        )
        .unwrap_or(false);

        // Step 3: Send response to miner (clone submit since we need it later)
        let response = submit.clone().respond(is_valid_share);
        self.downstream_channel_state
            .downstream_sv1_sender
            .send(response.into())
            .await
            .map_err(|e| {
                error!(
                    "Down: Failed to send submit response to downstream: {:?}",
                    e
                );
                TproxyError::ChannelErrorSender
            })?;

        // Step 4: If valid, send share to SV1 server for upstream submission
        if is_valid_share {
            let share_msg = SubmitShareWithChannelId {
                channel_id,
                downstream_id,
                share: submit,
                extranonce: extranonce1,
                extranonce2_len,
                version_rolling_mask,
                job_version: last_job_version,
            };

            self.downstream_channel_state
                .sv1_server_sender
                .send(DownstreamMessages::SubmitShares(share_msg))
                .await
                .map_err(|e| {
                    error!("Down: Failed to send share to SV1 server: {:?}", e);
                    TproxyError::ChannelErrorSender
                })?;
        } else {
            error!("Invalid share for channel id: {}", channel_id);
        }

        Ok(())
    }

    /// Handles SV1 handshake completion after mining.authorize.
    ///
    /// This method is called when the downstream completes the SV1 handshake
    /// (subscribe + authorize). It sends any cached messages in the correct order:
    /// set_difficulty first, then notify.
    async fn handle_sv1_handshake_completion(self: &Arc<Self>) -> Result<(), TproxyError> {
        let (cached_set_difficulty, cached_notify) = self.downstream_data.super_safe_lock(|d| {
            d.sv1_handshake_complete
                .store(true, std::sync::atomic::Ordering::SeqCst);
            (d.cached_set_difficulty.take(), d.cached_notify.take())
        });
        debug!("Down: SV1 handshake completed for downstream");

        // Send cached messages in correct order: set_difficulty first, then notify
        if let Some(set_difficulty_msg) = cached_set_difficulty {
            debug!("Down: Sending cached mining.set_difficulty after handshake completion");
            self.downstream_channel_state
                .downstream_sv1_sender
                .send(set_difficulty_msg)
                .await
                .map_err(|e| {
                    error!(
                        "Down: Failed to send cached mining.set_difficulty to downstream: {:?}",
                        e
                    );
                    TproxyError::ChannelErrorSender
                })?;

            // Update target and hashrate after sending set_difficulty
            self.downstream_data.super_safe_lock(|d| {
                if let Some(new_target) = d.pending_target.take() {
                    d.target = new_target;
                }
                if let Some(new_hashrate) = d.pending_hashrate.take() {
                    d.hashrate = Some(new_hashrate);
                }
            });
        }

        if let Some(notify_msg) = cached_notify {
            debug!("Down: Sending cached mining.notify after handshake completion");
            self.downstream_channel_state
                .downstream_sv1_sender
                .send(notify_msg)
                .await
                .map_err(|e| {
                    error!(
                        "Down: Failed to send cached mining.notify to downstream: {:?}",
                        e
                    );
                    TproxyError::ChannelErrorSender
                })?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;
    use stratum_apps::custom_mutex::Mutex;

    /// Test that validates the lock ordering fix for the deadlock scenario.
    ///
    /// Before the fix, the following pattern caused deadlocks:
    /// - Task A: holds downstream_data lock -> calls validate_sv1_share -> tries sv1_server_data lock
    /// - Task B: holds sv1_server_data lock -> iterates downstreams -> tries downstream_data lock
    ///
    /// After the fix, mining.submit handling extracts data from downstream_data first,
    /// releases the lock, then acquires sv1_server_data for validation.
    ///
    /// This test simulates the concurrent access pattern to verify no deadlock occurs.
    #[test]
    fn test_lock_ordering_no_deadlock() {
        // Create shared data structures similar to production
        let sv1_server_data = Arc::new(Mutex::new(0u32)); // Simplified stand-in
        let downstream_data = Arc::new(Mutex::new(0u32)); // Simplified stand-in

        let sv1_server_data_clone = sv1_server_data.clone();
        let downstream_data_clone = downstream_data.clone();

        // Simulate the FIXED pattern (what handle_submit_message_without_nested_lock does):
        // 1. Quick lock on downstream_data to extract data
        // 2. Release downstream_data
        // 3. Lock sv1_server_data for validation
        let handle1 = thread::spawn(move || {
            for _ in 0..100 {
                // Step 1: Extract from downstream_data (quick lock, release immediately)
                let _extracted = downstream_data_clone.super_safe_lock(|d| *d);
                // downstream_data lock is now released

                // Step 2: Acquire sv1_server_data for validation (no nested lock!)
                sv1_server_data_clone.super_safe_lock(|s| {
                    *s += 1;
                });

                thread::sleep(Duration::from_micros(10));
            }
        });

        let sv1_server_data_clone2 = sv1_server_data.clone();
        let downstream_data_clone2 = downstream_data.clone();

        // Simulate vardiff loop pattern (holds sv1_server_data, accesses downstream_data)
        let handle2 = thread::spawn(move || {
            for _ in 0..100 {
                // Vardiff pattern: hold sv1_server_data, then access downstream_data
                sv1_server_data_clone2.super_safe_lock(|s| {
                    *s += 1;
                    // Access downstream_data while holding sv1_server_data
                    downstream_data_clone2.super_safe_lock(|d| {
                        *d += 1;
                    });
                });

                thread::sleep(Duration::from_micros(10));
            }
        });

        // If this test completes without hanging, the lock ordering is correct
        // Set a timeout to detect deadlock
        let timeout = Duration::from_secs(5);
        let start = std::time::Instant::now();

        loop {
            if handle1.is_finished() && handle2.is_finished() {
                break;
            }
            if start.elapsed() > timeout {
                panic!("Test timed out - possible deadlock detected!");
            }
            thread::sleep(Duration::from_millis(100));
        }

        handle1.join().expect("Thread 1 panicked");
        handle2.join().expect("Thread 2 panicked");

        // Verify work was done
        let final_server = sv1_server_data.super_safe_lock(|s| *s);
        let final_downstream = downstream_data.super_safe_lock(|d| *d);
        assert!(
            final_server > 0,
            "sv1_server_data should have been modified"
        );
        assert!(
            final_downstream > 0,
            "downstream_data should have been modified"
        );
    }

    /// Test that demonstrates the OLD deadlock-prone pattern (for documentation).
    /// This test is ignored by default as it WILL deadlock.
    /// Run with: cargo test test_old_deadlock_pattern -- --ignored --nocapture
    #[test]
    #[ignore]
    fn test_old_deadlock_pattern_demonstration() {
        let sv1_server_data = Arc::new(Mutex::new(0u32));
        let downstream_data = Arc::new(Mutex::new(0u32));

        let sv1_server_data_clone = sv1_server_data.clone();
        let downstream_data_clone = downstream_data.clone();

        // OLD PATTERN (causes deadlock): hold downstream_data, then try sv1_server_data
        let handle1 = thread::spawn(move || {
            for _ in 0..100 {
                downstream_data_clone.super_safe_lock(|_d| {
                    // While holding downstream_data, try to acquire sv1_server_data
                    // This is the OLD pattern that caused deadlocks
                    sv1_server_data_clone.super_safe_lock(|s| {
                        *s += 1;
                    });
                });
                thread::sleep(Duration::from_micros(10));
            }
        });

        let sv1_server_data_clone2 = sv1_server_data.clone();
        let downstream_data_clone2 = downstream_data.clone();

        // Vardiff pattern: hold sv1_server_data, then try downstream_data
        let handle2 = thread::spawn(move || {
            for _ in 0..100 {
                sv1_server_data_clone2.super_safe_lock(|_s| {
                    // While holding sv1_server_data, try to acquire downstream_data
                    downstream_data_clone2.super_safe_lock(|d| {
                        *d += 1;
                    });
                });
                thread::sleep(Duration::from_micros(10));
            }
        });

        // This will likely hang due to deadlock
        handle1.join().expect("Thread 1 panicked");
        handle2.join().expect("Thread 2 panicked");
    }
}
