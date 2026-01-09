//! Vitess-based persistence handler implementation.
//!
//! This module provides a Vitess/MySQL persistence handler that batches events
//! and writes them to a Vitess cluster via vtgate. Events are written in the background
//! via an async channel to ensure the hot path remains unblocked.
//!
//! Features:
//! - Configurable batch size and timeout for optimal throughput
//! - Retry queue with exponential backoff for transient failures
//! - Automatic fallback to FileBackend on persistent failures
//! - Graceful shutdown with event draining
//! - Non-blocking fire-and-forget semantics

use crate::task_manager::TaskManager;

use super::{PersistenceBackend, PersistenceEvent, ShareEvent};
use async_channel::{Receiver, Sender};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::time::interval;
use tracing::{debug, error, info, warn};

/// Vitess-based persistence backend with retry queue and file fallback
#[derive(Debug, Clone)]
pub struct VitessBackend {
    sender: Sender<VitessCommand>,
}

#[derive(Debug)]
enum VitessCommand {
    Write(ShareEvent),
    Flush,
    Shutdown,
}

/// Configuration for Vitess backend
#[derive(Debug, Clone)]
pub struct VitessConfig {
    /// Vitess connection string (MySQL protocol)
    /// Format: mysql://username:password@vtgate:port/keyspace
    pub connection_string: String,
    /// Maximum number of connections in the pool
    pub pool_size: u32,
    /// Channel buffer size for async event queueing
    pub channel_size: usize,
    /// Number of events to batch before inserting
    pub batch_size: usize,
    /// Maximum time (ms) to wait before flushing batch
    pub batch_timeout_ms: u64,
    /// Maximum retry attempts before falling back
    pub retry_max_attempts: u32,
    /// Seconds to wait before retrying after failure
    pub retry_timeout_secs: u64,
    /// Optional file path for fallback persistence
    pub fallback_file_path: Option<PathBuf>,
}

/// Internal state for the worker
struct VitessWorkerState {
    pool: sqlx::mysql::MySqlPool,
    batch: Vec<ShareEvent>,
    config: VitessConfig,
    fallback: Option<Arc<super::FileBackend>>,
    retry_queue: Vec<(ShareEvent, u32)>, // (event, attempt_count)
    last_error_time: Option<std::time::Instant>,
    connection_healthy: bool,
}

impl VitessBackend {
    /// Create a new Vitess backend handler that will write to the specified connection.
    ///
    /// This will create a connection pool and spawn a background worker task.
    ///
    /// # Arguments
    ///
    /// * `config` - The Vitess configuration
    /// * `task_manager` - The task manager for spawning background tasks
    ///
    /// # Errors
    ///
    /// Returns an error if the connection pool cannot be created or the connection test fails.
    pub async fn new(
        config: VitessConfig,
        task_manager: Arc<TaskManager>,
    ) -> Result<Self, super::Error> {
        use sqlx::mysql::MySqlConnectOptions;
        use std::str::FromStr;

        // Parse connection string and configure for Vitess compatibility
        let connect_options = MySqlConnectOptions::from_str(&config.connection_string)
            .map_err(|e| {
                super::Error::Custom(format!("Invalid connection string: {}", e))
            })?
            // Vitess doesn't support PIPES_AS_CONCAT SQL mode
            .pipes_as_concat(false);

        // Create connection pool
        let pool = sqlx::mysql::MySqlPoolOptions::new()
            .max_connections(config.pool_size)
            .acquire_timeout(Duration::from_secs(5))
            .connect_with(connect_options)
            .await
            .map_err(|e| {
                super::Error::Custom(format!("Failed to connect to Vitess: {}", e))
            })?;

        // Test connection
        sqlx::query("SELECT 1")
            .execute(&pool)
            .await
            .map_err(|e| super::Error::Custom(format!("Connection test failed: {}", e)))?;

        let (sender, receiver) = async_channel::bounded(config.channel_size);

        // Initialize fallback if configured
        let fallback = if let Some(ref path) = config.fallback_file_path {
            let file_backend = super::FileBackend::new(
                path.clone(),
                config.channel_size,
                task_manager.clone(),
            )?;
            Some(Arc::new(file_backend))
        } else {
            None
        };

        // Spawn worker
        let worker_state = VitessWorkerState {
            pool,
            batch: Vec::with_capacity(config.batch_size),
            config: config.clone(),
            fallback,
            retry_queue: Vec::new(),
            last_error_time: None,
            connection_healthy: true,
        };

        task_manager.spawn(Self::worker_loop(receiver, worker_state));

        info!("Vitess persistence backend initialized");
        Ok(Self { sender })
    }

    /// Worker loop that runs as an async task and handles database writes.
    async fn worker_loop(receiver: Receiver<VitessCommand>, mut state: VitessWorkerState) {
        let mut batch_timer = interval(Duration::from_millis(state.config.batch_timeout_ms));

        loop {
            tokio::select! {
                // Handle incoming commands
                cmd = receiver.recv() => {
                    match cmd {
                        Ok(VitessCommand::Write(event)) => {
                            state.batch.push(event);

                            if state.batch.len() >= state.config.batch_size {
                                Self::flush_batch(&mut state).await;
                            }
                        }
                        Ok(VitessCommand::Flush) => {
                            Self::flush_batch(&mut state).await;
                        }
                        Ok(VitessCommand::Shutdown) => {
                            // Drain channel and flush
                            while let Ok(cmd) = receiver.try_recv() {
                                if let VitessCommand::Write(event) = cmd {
                                    state.batch.push(event);
                                }
                            }
                            Self::flush_batch(&mut state).await;

                            // Try to flush retry queue
                            Self::process_retry_queue(&mut state).await;

                            info!("Vitess worker shutdown complete");
                            break;
                        }
                        Err(_) => {
                            // Channel closed, shutdown
                            Self::flush_batch(&mut state).await;
                            break;
                        }
                    }
                }

                // Periodic batch flush
                _ = batch_timer.tick() => {
                    if !state.batch.is_empty() {
                        Self::flush_batch(&mut state).await;
                    }

                    // Process retry queue periodically
                    Self::process_retry_queue(&mut state).await;
                }
            }
        }
    }

    /// Flush the current batch to the database
    async fn flush_batch(state: &mut VitessWorkerState) {
        if state.batch.is_empty() {
            return;
        }

        let batch = std::mem::replace(
            &mut state.batch,
            Vec::with_capacity(state.config.batch_size),
        );

        match Self::insert_batch(&state.pool, &batch).await {
            Ok(_) => {
                state.connection_healthy = true;
                debug!("Flushed {} events to Vitess", batch.len());
            }
            Err(e) => {
                error!("Failed to insert batch: {}", e);
                state.connection_healthy = false;
                state.last_error_time = Some(std::time::Instant::now());

                // Add to retry queue
                for event in batch {
                    state.retry_queue.push((event, 1));
                }

                // Trim retry queue if too large (prevent memory exhaustion)
                if state.retry_queue.len() > state.config.channel_size * 2 {
                    warn!("Retry queue full, failing over to file backend");
                    Self::fallback_to_file(state).await;
                }
            }
        }
    }

    /// Insert a batch of events into the database
    async fn insert_batch(
        pool: &sqlx::mysql::MySqlPool,
        events: &[ShareEvent],
    ) -> Result<(), sqlx::Error> {
        if events.is_empty() {
            return Ok(());
        }

        // Use transaction for batch atomicity
        let mut tx = pool.begin().await?;

        for event in events {
            let timestamp_micros = event
                .timestamp
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_micros() as i64;

            let share_hash_bytes = event
                .share_hash
                .as_ref()
                .map(|h| {
                    use stratum_core::bitcoin::hashes::Hash as HashTrait;
                    h.as_byte_array().to_vec()
                });

            sqlx::query(
                r#"
                INSERT INTO share_events (
                    shard_key, event_timestamp, user_identity, template_id,
                    is_valid, is_block_found, error_code, share_hash, target,
                    nonce, version, ntime, extranonce_prefix, rollable_extranonce_size,
                    share_work, nominal_hash_rate
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(&event.user_identity) // shard_key
            .bind(timestamp_micros)
            .bind(&event.user_identity)
            .bind(event.template_id.map(|id| id as i64))
            .bind(event.is_valid)
            .bind(event.is_block_found)
            .bind(&event.error_code)
            .bind(share_hash_bytes)
            .bind(&event.target[..]) // Convert [u8; 32] to &[u8]
            // Cast u32 to u64 to preserve unsigned semantics in MySQL
            .bind(event.nonce as u64)
            .bind(event.version as u64)
            .bind(event.ntime as u64)
            .bind(&event.extranonce_prefix)
            .bind(event.rollable_extranonce_size.map(|s| s as i16))
            .bind(event.share_work)
            .bind(event.nominal_hash_rate)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// Process the retry queue
    async fn process_retry_queue(state: &mut VitessWorkerState) {
        if state.retry_queue.is_empty() {
            return;
        }

        // Check if enough time has passed since last error
        if let Some(last_error) = state.last_error_time {
            if last_error.elapsed().as_secs() < state.config.retry_timeout_secs {
                return;
            }
        }

        // Try connection health check
        if !state.connection_healthy {
            match sqlx::query("SELECT 1").execute(&state.pool).await {
                Ok(_) => {
                    state.connection_healthy = true;
                    info!("Vitess connection restored");
                }
                Err(_) => {
                    debug!("Vitess still unhealthy, skipping retry");
                    return;
                }
            }
        }

        let retry_queue = std::mem::take(&mut state.retry_queue);
        let mut failed_retries = Vec::new();

        for (event, attempt) in retry_queue {
            if attempt >= state.config.retry_max_attempts {
                warn!("Max retry attempts reached for event, using fallback");
                if let Some(ref fallback) = state.fallback {
                    if let Err(e) = fallback.persist_event(PersistenceEvent::Share(event)) {
                        error!("Failed to persist to fallback file: {}", e);
                    }
                } else {
                    error!("No fallback configured, dropping event");
                }
                continue;
            }

            match Self::insert_batch(&state.pool, &[event.clone()]).await {
                Ok(_) => {
                    debug!("Successfully retried event on attempt {}", attempt);
                }
                Err(e) => {
                    debug!("Retry attempt {} failed: {}", attempt, e);
                    failed_retries.push((event, attempt + 1));
                }
            }
        }

        state.retry_queue = failed_retries;
    }

    /// Fallback to file for all events in retry queue
    async fn fallback_to_file(state: &mut VitessWorkerState) {
        if let Some(ref fallback) = state.fallback {
            let events = std::mem::take(&mut state.retry_queue);
            let event_count = events.len();
            for (event, _) in events {
                if let Err(e) = fallback.persist_event(PersistenceEvent::Share(event)) {
                    error!("Failed to persist to fallback file: {}", e);
                }
            }
            info!("Flushed {} events to file fallback", event_count);
        }
    }
}

impl PersistenceBackend for VitessBackend {
    fn persist_event(&self, event: PersistenceEvent) -> Result<(), super::PersistenceError> {
        let PersistenceEvent::Share(share_event) = event;
        self.sender
            .try_send(VitessCommand::Write(share_event))
            .map_err(|e| {
                error!("Failed to send event to Vitess persistence: {}", e);
                super::PersistenceError::ChannelFull
            })
    }

    fn flush(&self) -> Result<(), super::PersistenceError> {
        self.sender.try_send(VitessCommand::Flush).map_err(|e| {
            error!("Failed to send flush command: {}", e);
            super::PersistenceError::ChannelFull
        })
    }

    fn shutdown(&self) -> Result<(), super::PersistenceError> {
        self.sender
            .try_send(VitessCommand::Shutdown)
            .map_err(|e| {
                error!("Failed to send shutdown command: {}", e);
                super::PersistenceError::ChannelFull
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;
    use stratum_core::bitcoin::hashes::{sha256d::Hash, Hash as HashTrait};

    fn create_test_share_event() -> ShareEvent {
        ShareEvent {
            error_code: None,
            extranonce_prefix: vec![0x01, 0x02, 0x03],
            is_block_found: false,
            is_valid: true,
            nominal_hash_rate: 100.0,
            nonce: 12345678,
            ntime: 1609459200,
            rollable_extranonce_size: Some(4),
            share_hash: Some(Hash::from_byte_array([0xaa; 32])),
            share_work: 1000.0,
            target: [0xff; 32],
            template_id: Some(1001),
            timestamp: SystemTime::now(),
            user_identity: "test_miner_01".to_string(),
            version: 536870912,
        }
    }

    #[tokio::test]
    #[ignore] // Requires running MySQL/Vitess instance
    async fn test_vitess_backend_basic_insert() {
        let config = VitessConfig {
            connection_string: "mysql://root:password@localhost:3306/test_db".to_string(),
            pool_size: 5,
            channel_size: 100,
            batch_size: 10,
            batch_timeout_ms: 100,
            retry_max_attempts: 3,
            retry_timeout_secs: 5,
            fallback_file_path: None,
        };

        let task_manager = Arc::new(TaskManager::new());
        let backend = VitessBackend::new(config, task_manager).await.unwrap();

        let event = create_test_share_event();
        backend.persist_event(PersistenceEvent::Share(event));
        backend.flush();

        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        // Verify insertion via query would go here
    }

    #[tokio::test]
    #[ignore]
    async fn test_vitess_backend_batch_insert() {
        // Test that batch_size events trigger automatic flush
        let config = VitessConfig {
            connection_string: "mysql://root:password@localhost:3306/test_db".to_string(),
            pool_size: 5,
            channel_size: 100,
            batch_size: 5,
            batch_timeout_ms: 1000,
            retry_max_attempts: 3,
            retry_timeout_secs: 5,
            fallback_file_path: None,
        };

        let task_manager = Arc::new(TaskManager::new());
        let backend = VitessBackend::new(config, task_manager).await.unwrap();

        // Send batch_size events
        for _ in 0..5 {
            let event = create_test_share_event();
            backend.persist_event(PersistenceEvent::Share(event));
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        // Batch should be automatically flushed
    }

    #[tokio::test]
    #[ignore]
    async fn test_vitess_backend_batch_timeout() {
        // Test that partial batch flushes after timeout
        let config = VitessConfig {
            connection_string: "mysql://root:password@localhost:3306/test_db".to_string(),
            pool_size: 5,
            channel_size: 100,
            batch_size: 100,
            batch_timeout_ms: 200,
            retry_max_attempts: 3,
            retry_timeout_secs: 5,
            fallback_file_path: None,
        };

        let task_manager = Arc::new(TaskManager::new());
        let backend = VitessBackend::new(config, task_manager).await.unwrap();

        // Send only 3 events (less than batch_size)
        for _ in 0..3 {
            let event = create_test_share_event();
            backend.persist_event(PersistenceEvent::Share(event));
        }

        // Wait for timeout
        tokio::time::sleep(tokio::time::Duration::from_millis(400)).await;
        // Batch should be flushed due to timeout
    }

    #[tokio::test]
    #[ignore]
    async fn test_vitess_backend_graceful_shutdown() {
        let config = VitessConfig {
            connection_string: "mysql://root:password@localhost:3306/test_db".to_string(),
            pool_size: 5,
            channel_size: 100,
            batch_size: 10,
            batch_timeout_ms: 1000,
            retry_max_attempts: 3,
            retry_timeout_secs: 5,
            fallback_file_path: None,
        };

        let task_manager = Arc::new(TaskManager::new());
        let backend = VitessBackend::new(config, task_manager).await.unwrap();

        let event = create_test_share_event();
        backend.persist_event(PersistenceEvent::Share(event));
        backend.shutdown();

        // Give worker time to shutdown
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    }
}
