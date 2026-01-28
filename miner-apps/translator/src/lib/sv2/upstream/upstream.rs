use crate::{
    error::{self, TproxyError, TproxyErrorKind, TproxyResult},
    io_task::spawn_io_tasks,
    status::{handle_error, Status, StatusSender},
    sv2::upstream::channel::UpstreamChannelState,
    utils::{ShutdownMessage, UpstreamEntry},
};
use async_channel::{unbounded, Receiver, Sender};
use std::{net::SocketAddr, sync::Arc};
use stratum_apps::{
    custom_mutex::Mutex,
    network_helpers::noise_stream::NoiseTcpStream,
    stratum_core::{
        binary_sv2::Seq064K,
        codec_sv2::HandshakeRole,
        common_messages_sv2::{Protocol, SetupConnection},
        extensions_sv2::RequestExtensions,
        handlers_sv2::HandleCommonMessagesFromServerAsync,
        noise_sv2::Initiator,
        parsers_sv2::{AnyMessage, Mining},
    },
    task_manager::TaskManager,
    utils::{
        protocol_message_type::{protocol_message_type, MessageType},
        types::{Message, Sv2Frame},
    },
};
use tokio::{
    net::TcpStream,
    sync::{broadcast, mpsc},
};
use tracing::{debug, error, info, warn};

/// Manages the upstream SV2 connection to a mining pool or proxy.
///
/// This struct handles the SV2 protocol communication with upstream servers,
/// including:
/// - Connection establishment with multiple upstream fallbacks
/// - SV2 handshake and setup procedures
/// - Message routing between channel manager and upstream
/// - Connection monitoring and error handling
/// - Graceful shutdown coordination
///
/// The upstream connection supports automatic failover between multiple
/// configured upstream servers and implements retry logic for connection
/// establishment.
#[derive(Clone)]
pub struct Upstream {
    pub upstream_channel_state: UpstreamChannelState,
    /// Extensions that the translator requires (must be supported by server)
    pub required_extensions: Vec<u16>,
    address: SocketAddr,
    event_metrics: SharedEventMetrics,
}

type SharedEventMetrics =
    Arc<Mutex<Option<Arc<stratum_apps::monitoring::event_metrics::EventMetrics>>>>;

#[cfg_attr(not(test), hotpath::measure_all)]
impl Upstream {
    /// Creates a new upstream connection by attempting to connect to configured servers.
    ///
    /// This method tries to establish a connection to one of the provided upstream
    /// servers, implementing retry logic and fallback behavior. It will attempt
    /// to connect to each server multiple times before giving up.
    ///
    /// # Arguments
    /// * `upstreams` - A single `UpstreamEntry` representing the upstream candidate currently being
    ///   attempted. The `tried_or_flagged` is set once the upstream has either been connected to
    ///   successfully or marked as malicious. Because `new` is only called from
    ///   `try_initialize_upstream`, we can treat this flag as the definitive state for that
    ///   upstream.
    /// * `channel_manager_sender` - Channel to send messages to the channel manager
    /// * `channel_manager_receiver` - Channel to receive messages from the channel manager
    /// * `notify_shutdown` - Broadcast channel for shutdown coordination
    /// * `shutdown_complete_tx` - Channel to signal shutdown completion
    ///
    /// # Returns
    /// * `Ok(Upstream)` - Successfully connected to an upstream server
    /// * `Err(TproxyError)` - Failed to connect to any upstream server
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        upstream: &UpstreamEntry,
        channel_manager_sender: Sender<Sv2Frame>,
        channel_manager_receiver: Receiver<Sv2Frame>,
        notify_shutdown: broadcast::Sender<ShutdownMessage>,
        shutdown_complete_tx: mpsc::Sender<()>,
        task_manager: Arc<TaskManager>,
        required_extensions: Vec<u16>,
        event_metrics: SharedEventMetrics,
    ) -> TproxyResult<Self, error::Upstream> {
        let mut shutdown_rx = notify_shutdown.subscribe();

        info!("Trying to connect to upstream at {}", upstream.addr);

        if shutdown_rx.try_recv().is_ok() {
            info!("Shutdown signal received during upstream connection attempt. Aborting.");
            drop(shutdown_complete_tx);
            return Err(TproxyError::shutdown(
                TproxyErrorKind::CouldNotInitiateSystem,
            ));
        }

        match TcpStream::connect(upstream.addr).await {
            Ok(socket) => {
                info!("Connected to upstream at {}", upstream.addr);

                let initiator = Initiator::from_raw_k(upstream.authority_pubkey.into_bytes())
                    .map_err(TproxyError::fallback)?;
                match NoiseTcpStream::new(socket, HandshakeRole::Initiator(initiator)).await {
                    Ok(stream) => {
                        let (reader, writer) = stream.into_split();

                        let (outbound_tx, outbound_rx) = unbounded();
                        let (inbound_tx, inbound_rx) = unbounded();

                        spawn_io_tasks(
                            task_manager,
                            reader,
                            writer,
                            outbound_rx,
                            inbound_tx,
                            notify_shutdown,
                        );

                        let upstream_channel_state = UpstreamChannelState::new(
                            inbound_rx,
                            outbound_tx,
                            channel_manager_sender,
                            channel_manager_receiver,
                        );
                        debug!(
                            "Successfully initialized upstream channel with {}",
                            upstream.addr
                        );

                        return Ok(Self {
                            upstream_channel_state,
                            required_extensions: required_extensions.clone(),
                            address: upstream.addr,
                            event_metrics,
                        });
                    }
                    Err(e) => {
                        error!(
                            "Failed Noise handshake with {}: {e}. Retrying...",
                            upstream.addr
                        );
                    }
                }
            }
            Err(e) => {
                error!("Failed to connect to {}: {e}.", upstream.addr);
            }
        }

        error!("Failed to connect to any configured upstream.");
        drop(shutdown_complete_tx);
        Err(TproxyError::shutdown(
            TproxyErrorKind::CouldNotInitiateSystem,
        ))
    }

    /// Starts the upstream connection and begins message processing.
    ///
    /// This method:
    /// - Completes the SV2 handshake with the upstream server
    /// - Spawns the main message processing task
    /// - Handles graceful shutdown coordination
    ///
    /// The method will first attempt to complete the SV2 setup connection
    /// handshake. If successful, it spawns a task to handle bidirectional
    /// message flow between the channel manager and upstream server.
    pub async fn start(
        mut self,
        notify_shutdown: broadcast::Sender<ShutdownMessage>,
        shutdown_complete_tx: mpsc::Sender<()>,
        status_sender: Sender<Status>,
        task_manager: Arc<TaskManager>,
    ) -> TproxyResult<(), error::Upstream> {
        let mut shutdown_rx = notify_shutdown.subscribe();
        // Wait for connection setup or shutdown signal
        tokio::select! {
            result = self.setup_connection() => {
                if let Err(e) = result {
                    error!("Upstream: failed to set up SV2 connection: {e:?}");
                    drop(shutdown_complete_tx);
                    return Err(e);
                }
            }
            message = shutdown_rx.recv() => {
                match message {
                    Ok(ShutdownMessage::ShutdownAll) => {
                        info!("Upstream: shutdown signal received during connection setup.");
                        drop(shutdown_complete_tx);
                        return Ok(());
                    }
                    Ok(ShutdownMessage::UpstreamFallback{tx}) => {
                        info!("Upstream: shutdown signal received during connection setup.");
                        drop(shutdown_complete_tx);
                        drop(tx);
                        return Ok(());
                    }
                    Ok(_) => {}

                    Err(e) => {
                        error!("Upstream: failed to receive shutdown signal: {e}");
                        drop(shutdown_complete_tx);
                        return Ok(());
                    }
                }
            }
        }

        // Wrap status sender and start upstream task
        let wrapped_status_sender = StatusSender::Upstream(status_sender);

        self.run_upstream_task(
            notify_shutdown,
            shutdown_complete_tx,
            wrapped_status_sender,
            task_manager,
        )?;

        Ok(())
    }

    /// Performs the SV2 handshake setup with the upstream server.
    ///
    /// This method handles the initial SV2 protocol handshake by:
    /// - Creating and sending a SetupConnection message
    /// - Waiting for the handshake response
    /// - Validating and processing the response
    ///
    /// The handshake establishes the protocol version, capabilities, and
    /// other connection parameters needed for SV2 communication.
    pub async fn setup_connection(&mut self) -> TproxyResult<(), error::Upstream> {
        debug!("Upstream: initiating SV2 handshake...");
        // Build SetupConnection message
        let setup_conn_msg = Self::get_setup_connection_message(2, 2, &self.address, false)
            .map_err(TproxyError::shutdown)?;
        let sv2_frame: Sv2Frame =
            Message::Common(setup_conn_msg.into())
                .try_into()
                .map_err(|error| {
                    error!("Failed to serialize SetupConnection message: {error:?}");
                    TproxyError::shutdown(error)
                })?;

        // Send SetupConnection message to upstream
        self.upstream_channel_state
            .upstream_sender
            .send(sv2_frame)
            .await
            .map_err(|e| {
                error!("Failed to send SetupConnection to upstream: {:?}", e);
                TproxyError::fallback(TproxyErrorKind::ChannelErrorSender)
            })?;

        let mut incoming: Sv2Frame =
            match self.upstream_channel_state.upstream_receiver.recv().await {
                Ok(frame) => {
                    debug!("Received handshake response from upstream.");
                    frame
                }
                Err(e) => {
                    error!("Failed to receive handshake response from upstream: {}", e);
                    return Err(TproxyError::fallback(e));
                }
            };

        let header = incoming.get_header().ok_or_else(|| {
            error!("Expected handshake frame but no header found.");
            TproxyError::fallback(TproxyErrorKind::UnexpectedMessage(0, 0))
        })?;

        let payload = incoming.payload();

        self.handle_common_message_frame_from_server(None, header, payload)
            .await?;
        debug!("Upstream: handshake completed successfully.");

        // Send RequestExtensions message if there are any required extensions
        if !self.required_extensions.is_empty() {
            let require_extensions = RequestExtensions {
                request_id: 1,
                requested_extensions: Seq064K::new(self.required_extensions.clone()).unwrap(),
            };

            let sv2_frame: Sv2Frame =
                AnyMessage::Extensions(require_extensions.into_static().into())
                    .try_into()
                    .map_err(TproxyError::shutdown)?;

            info!(
                "Sending RequestExtensions message to upstream: {:?}",
                sv2_frame
            );

            self.upstream_channel_state
                .upstream_sender
                .send(sv2_frame)
                .await
                .map_err(|e| {
                    error!("Failed to send message to upstream: {:?}", e);
                    TproxyError::fallback(TproxyErrorKind::ChannelErrorSender)
                })?;
        }
        Ok(())
    }

    /// Processes incoming messages from the upstream SV2 server.
    ///
    /// This method handles different types of frames received from upstream:
    /// - SV2 frames: Parses and routes mining/common messages appropriately
    /// - Handshake frames: Logs for debugging (shouldn't occur during normal operation)
    ///
    /// Common messages are handled directly, while mining messages are forwarded
    /// to the channel manager for processing and distribution to downstream connections.
    pub async fn on_upstream_message(
        &mut self,
        mut sv2_frame: Sv2Frame,
    ) -> TproxyResult<(), error::Upstream> {
        debug!("Received SV2 frame from upstream.");
        let Some(header) = sv2_frame.get_header() else {
            return Err(TproxyError::fallback(TproxyErrorKind::UnexpectedMessage(
                0, 0,
            )));
        };

        self.event_metrics.super_safe_lock(|m| {
            if let Some(ref metrics) = m.as_ref() {
                metrics.inc_messages(
                    stratum_apps::monitoring::direction::SERVER,
                    header_msg_type_str(header.ext_type(), header.msg_type()),
                );
                metrics.add_bytes(
                    stratum_apps::monitoring::direction::SERVER,
                    sv2_frame.encoded_length() as u64,
                );
            }
        });

        match protocol_message_type(header.ext_type(), header.msg_type()) {
            MessageType::Common => {
                info!(
                    extension_type = header.ext_type(),
                    message_type = header.msg_type(),
                    "Handling common message from Upstream."
                );
                self.handle_common_message_frame_from_server(None, header, sv2_frame.payload())
                    .await?;
            }
            MessageType::Mining | MessageType::Extensions => {
                self.upstream_channel_state
                    .channel_manager_sender
                    .send(sv2_frame)
                    .await
                    .map_err(|e| {
                        error!("Failed to send mining message to channel manager: {:?}", e);
                        TproxyError::shutdown(TproxyErrorKind::ChannelErrorSender)
                    })?;
            }
            _ => {
                warn!(
                    extension_type = header.ext_type(),
                    message_type = header.msg_type(),
                    "Received unsupported message type from upstream."
                );
                return Err(TproxyError::fallback(TproxyErrorKind::UnexpectedMessage(
                    header.ext_type(),
                    header.msg_type(),
                )));
            }
        }
        Ok(())
    }

    /// Spawns a unified task to handle upstream message I/O and shutdown logic.
    #[allow(clippy::result_large_err)]
    fn run_upstream_task(
        mut self,
        notify_shutdown: broadcast::Sender<ShutdownMessage>,
        shutdown_complete_tx: mpsc::Sender<()>,
        status_sender: StatusSender,
        task_manager: Arc<TaskManager>,
    ) -> TproxyResult<(), error::Upstream> {
        let mut shutdown_rx = notify_shutdown.subscribe();
        let shutdown_complete_tx = shutdown_complete_tx.clone();

        task_manager.spawn(async move {
            loop {
                tokio::select! {
                    // Handle shutdown signals
                    shutdown = shutdown_rx.recv() => {
                        match shutdown {
                            Ok(ShutdownMessage::ShutdownAll) => {
                                info!("Upstream: received ShutdownAll signal. Exiting loop.");
                                break;
                            }
                            Ok(ShutdownMessage::UpstreamFallback{tx}) => {
                                info!("Upstream: fallback initiated");
                                drop(tx);
                                break;
                            }
                            Ok(_) => {
                                // Ignore other shutdown variants for upstream
                            }
                            Err(e) => {
                                error!("Upstream: failed to receive shutdown signal: {e}");
                                break;
                            }
                        }
                    }

                    // Handle incoming SV2 messages from upstream
                    result = self.upstream_channel_state.upstream_receiver.recv() => {
                        match result {
                            Ok(frame) => {
                                debug!("Upstream: received frame.");
                                if let Err(e) = self.on_upstream_message(frame).await {
                                    error!("Upstream: error while processing message: {e:?}");
                                    handle_error(&status_sender, e).await;
                                }
                            }
                            Err(e) => {
                                error!("Upstream: receiver channel closed unexpectedly: {e}");
                                handle_error(&status_sender, TproxyError::<error::Upstream>::fallback(e)).await;
                                break;
                            }
                        }
                    }

                    // Handle messages from channel manager to send upstream
                    result = self.upstream_channel_state.channel_manager_receiver.recv() => {
                        match result {
                            Ok(sv2_frame) => {
                                debug!("Upstream: sending sv2 frame from channel manager: {:?}", sv2_frame);

                                if let Some(header) = sv2_frame.get_header() {
                                    self.event_metrics.super_safe_lock(|m| {
                                        if let Some(ref metrics) = m.as_ref() {
                                            metrics.inc_messages(
                                                stratum_apps::monitoring::direction::SERVER,
                                                header_msg_type_str(header.ext_type(), header.msg_type()),
                                            );
                                            metrics.add_bytes(
                                                stratum_apps::monitoring::direction::SERVER,
                                                sv2_frame.encoded_length() as u64,
                                            );
                                        }
                                    });
                                }

                                if let Err(e) = self
                                    .upstream_channel_state
                                    .upstream_sender
                                    .send(sv2_frame)
                                    .await
                                    .map_err(|e| {
                                        error!("Upstream: failed to send sv2 frame: {e:?}");
                                        TproxyError::<error::Upstream>::fallback(TproxyErrorKind::ChannelErrorSender)
                                    })
                                {
                                    handle_error(&status_sender, e).await;
                                }
                            }
                            Err(e) => {
                                error!("Upstream: channel manager receiver closed: {e}");
                                handle_error(&status_sender, TproxyError::<error::Upstream>::shutdown(e)).await;
                                break;
                            }
                        }
                    }
                }
            }

            self.upstream_channel_state.drop();
            warn!("Upstream: task shutting down cleanly.");
            drop(shutdown_complete_tx);
        });

        Ok(())
    }

    /// Sends a message to the upstream SV2 server.
    ///
    /// This method forwards messages from the channel manager to the upstream
    /// server. Messages are typically mining-related (share submissions, channel
    /// requests, etc.) that need to be sent upstream.
    ///
    /// # Arguments
    /// * `sv2_frame` - The SV2 frame to send to the upstream server
    ///
    /// # Returns
    /// * `Ok(())` - Message sent successfully
    /// * `Err(TproxyError)` - Error sending the message
    pub async fn send_upstream(
        &self,
        message: Mining<'static>,
    ) -> TproxyResult<(), error::Upstream> {
        debug!("Sending message to upstream.");
        let message = AnyMessage::Mining(message);
        let sv2_frame: Sv2Frame = message.try_into().map_err(TproxyError::shutdown)?;

        if let Some(header) = sv2_frame.get_header() {
            self.event_metrics.super_safe_lock(|m| {
                if let Some(ref metrics) = m.as_ref() {
                    metrics.inc_messages(
                        stratum_apps::monitoring::direction::SERVER,
                        header_msg_type_str(header.ext_type(), header.msg_type()),
                    );
                    metrics.add_bytes(
                        stratum_apps::monitoring::direction::SERVER,
                        sv2_frame.encoded_length() as u64,
                    );
                }
            });
        }

        self.upstream_channel_state
            .upstream_sender
            .send(sv2_frame)
            .await
            .map_err(|e| {
                error!("Failed to send message to upstream: {:?}", e);
                TproxyError::fallback(TproxyErrorKind::ChannelErrorSender)
            })?;

        Ok(())
    }

    /// Constructs the `SetupConnection` message.
    #[allow(clippy::result_large_err)]
    fn get_setup_connection_message(
        min_version: u16,
        max_version: u16,
        address: &SocketAddr,
        is_work_selection_enabled: bool,
    ) -> Result<SetupConnection<'static>, TproxyErrorKind> {
        let endpoint_host = address.ip().to_string().into_bytes().try_into()?;
        let vendor = "SRI".to_string().try_into()?;
        let hardware_version = "Translator Proxy".to_string().try_into()?;
        let firmware = String::new().try_into()?;
        let device_id = String::new().try_into()?;
        let flags = if is_work_selection_enabled {
            0b110
        } else {
            0b100
        };

        Ok(SetupConnection {
            protocol: Protocol::MiningProtocol,
            min_version,
            max_version,
            flags,
            endpoint_host,
            endpoint_port: address.port(),
            vendor,
            hardware_version,
            firmware,
            device_id,
        })
    }
}

fn header_msg_type_str(ext_type: u16, msg_type: u8) -> &'static str {
    stratum_apps::monitoring::msg_type_mapping::header_to_msg_type(ext_type, msg_type)
}
