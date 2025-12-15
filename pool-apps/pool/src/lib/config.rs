//! ## Configuration Module
//!
//! Defines [`PoolConfig`], the configuration structure for the Pool, along with its supporting
//! types.
//!
//! This module handles:
//! - Initializing [`PoolConfig`]
//! - Managing [`TemplateProviderConfig`], [`AuthorityConfig`], [`CoinbaseOutput`], and
//!   [`ConnectionConfig`]
//! - Validating and converting coinbase outputs
#[cfg(feature = "persistence")]
use std::sync::Arc;
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

#[cfg(feature = "persistence")]
use stratum_apps::task_manager::TaskManager;
use stratum_apps::{
    config_helpers::CoinbaseRewardScript,
    key_utils::{Secp256k1PublicKey, Secp256k1SecretKey},
    stratum_core::bitcoin::{Amount, TxOut},
    tp_type::TemplateProviderType,
    utils::types::{SharesBatchSize, SharesPerMinute},
};

/// Configuration for the Pool, including connection, authority, and coinbase settings.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct PoolConfig {
    listen_address: SocketAddr,
    template_provider_type: TemplateProviderType,
    authority_public_key: Secp256k1PublicKey,
    authority_secret_key: Secp256k1SecretKey,
    cert_validity_sec: u64,
    coinbase_reward_script: CoinbaseRewardScript,
    pool_signature: String,
    shares_per_minute: SharesPerMinute,
    share_batch_size: SharesBatchSize,
    log_file: Option<PathBuf>,
    server_id: u16,
    supported_extensions: Vec<u16>,
    required_extensions: Vec<u16>,
    #[cfg(feature = "persistence")]
    persistence: Option<PersistenceConfig>,
}

/// File backend configuration
#[cfg(feature = "persistence")]
#[derive(Clone, Debug, serde::Deserialize)]
pub struct FileBackendConfig {
    /// Path to the persistence file
    pub file_path: PathBuf,
    /// Channel buffer size for async persistence
    #[serde(default = "default_channel_size")]
    pub channel_size: usize,
    /// Entity types this backend handles (e.g., ["shares"])
    #[serde(default = "default_entities")]
    pub entities: Vec<String>,
    /// Optional fallback backend name (e.g., "file" to fall back to file backend)
    #[serde(default)]
    pub fallback: Option<String>,
}

/// Metrics backend configuration (Prometheus)
#[cfg(feature = "metrics")]
#[derive(Clone, Debug, serde::Deserialize)]
pub struct MetricsBackendConfig {
    /// HTTP endpoint path for Prometheus scraping
    pub resource_path: String,
    /// Port for metrics HTTP server
    pub port: u16,
    /// Entity types this backend handles (e.g., ["shares"])
    #[serde(default = "default_entities")]
    pub entities: Vec<String>,
    /// Optional fallback backend name (e.g., "file" to fall back to file backend)
    #[serde(default)]
    pub fallback: Option<String>,
}

/// Vitess backend configuration
#[cfg(feature = "vitess")]
#[derive(Clone, Debug, serde::Deserialize)]
pub struct VitessBackendConfig {
    /// Vitess connection string (MySQL protocol)
    /// Format: mysql://username:password@vtgate:port/keyspace
    pub connection_string: String,
    /// Maximum number of connections in the pool
    #[serde(default = "default_pool_size")]
    pub pool_size: u32,
    /// Channel buffer size for async event queueing
    #[serde(default = "default_channel_size")]
    pub channel_size: usize,
    /// Number of events to batch before inserting
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
    /// Maximum time (ms) to wait before flushing batch
    #[serde(default = "default_batch_timeout_ms")]
    pub batch_timeout_ms: u64,
    /// Maximum retry attempts before falling back
    #[serde(default = "default_retry_max_attempts")]
    pub retry_max_attempts: u32,
    /// Seconds to wait before retrying after failure
    #[serde(default = "default_retry_timeout_secs")]
    pub retry_timeout_secs: u64,
    /// Optional file path for fallback persistence
    pub fallback_file_path: Option<PathBuf>,
    /// Entity types this backend handles (e.g., ["shares"])
    #[serde(default = "default_entities")]
    pub entities: Vec<String>,
    /// Optional fallback backend name (e.g., "file" to fall back to file backend)
    #[serde(default)]
    pub fallback: Option<String>,
}

/// Persistence configuration for share event logging.
///
/// This is only available when the `persistence` feature is enabled.
///
/// Persistence is automatically enabled when any backend section is present.
/// Each backend declares which entities it handles, and optionally a fallback.
/// The system automatically constructs a composite backend from all configured backends.
///
/// # Example TOML
///
/// ```toml
/// [persistence.metrics]
/// resource_path = "/metrics"
/// port = 9091
/// entities = ["shares"]
/// fallback = "file"  # Fall back to file backend on error
///
/// [persistence.file]
/// file_path = "./pool_shares.log"
/// entities = ["shares"]
/// # No fallback - this is the last resort
/// ```
#[cfg(feature = "persistence")]
#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct PersistenceConfig {
    /// File backend configuration
    #[serde(default)]
    pub file: Option<FileBackendConfig>,
    /// Metrics backend configuration
    #[cfg(feature = "metrics")]
    #[serde(default)]
    pub metrics: Option<MetricsBackendConfig>,
    /// Vitess backend configuration
    #[cfg(feature = "vitess")]
    #[serde(default)]
    pub vitess: Option<VitessBackendConfig>,
}

#[cfg(feature = "persistence")]
fn default_channel_size() -> usize {
    10000
}

#[cfg(feature = "persistence")]
fn default_entities() -> Vec<String> {
    vec!["shares".to_string()]
}

#[cfg(feature = "vitess")]
fn default_pool_size() -> u32 {
    10
}

#[cfg(feature = "vitess")]
fn default_batch_size() -> usize {
    100
}

#[cfg(feature = "vitess")]
fn default_batch_timeout_ms() -> u64 {
    1000 // 1 second
}

#[cfg(feature = "vitess")]
fn default_retry_max_attempts() -> u32 {
    3
}

#[cfg(feature = "vitess")]
fn default_retry_timeout_secs() -> u64 {
    10
}

/// Helper function to parse entity type strings
#[cfg(feature = "persistence")]
fn parse_entity_type(s: &str) -> Option<stratum_apps::persistence::EntityType> {
    use stratum_apps::persistence::EntityType;
    match s {
        "shares" => Some(EntityType::Share),
        // Future: "connections" => Some(EntityType::Connection),
        _ => {
            tracing::warn!("Unknown entity type: {}", s);
            None
        }
    }
}

/// Implement IntoPersistence trait for pool's config type
///
/// This automatically constructs a composite backend from all configured backend sections.
/// Each backend becomes a route in the composite, with fallbacks wired up as specified.
/// The global entity set is inferred from the union of all backend entity lists.
#[cfg(feature = "persistence")]
impl stratum_apps::persistence::IntoPersistence for PersistenceConfig {
    fn into_persistence(
        self,
        task_manager: Arc<TaskManager>,
    ) -> Result<stratum_apps::persistence::Persistence, stratum_apps::persistence::Error> {
        use std::collections::{HashMap, HashSet};
        #[cfg(feature = "metrics")]
        use stratum_apps::persistence::MetricsBackend;
        use stratum_apps::persistence::{
            Backend, BackendRoute, CompositeBackend, EntityType, FileBackend, Persistence,
        };

        // Build backends and collect entity types
        let mut backends: HashMap<
            String,
            std::sync::Arc<dyn stratum_apps::persistence::PersistenceBackend>,
        > = HashMap::new();
        let mut backend_entities: HashMap<String, HashSet<EntityType>> = HashMap::new();
        let mut backend_fallbacks: HashMap<String, Option<String>> = HashMap::new();
        let mut all_entities: HashSet<EntityType> = HashSet::new();

        // Create file backend if configured
        if let Some(file_config) = &self.file {
            let file_backend = std::sync::Arc::new(FileBackend::new(
                file_config.file_path.clone(),
                file_config.channel_size,
                task_manager.clone(),
            )?);
            backends.insert("file".to_string(), file_backend);

            let entities: HashSet<EntityType> = file_config
                .entities
                .iter()
                .filter_map(|s| parse_entity_type(s))
                .collect();
            all_entities.extend(entities.iter().cloned());
            backend_entities.insert("file".to_string(), entities);
            backend_fallbacks.insert("file".to_string(), file_config.fallback.clone());
        }

        // Create metrics backend if configured
        #[cfg(feature = "metrics")]
        if let Some(metrics_config) = &self.metrics {
            let metrics_backend = std::sync::Arc::new(MetricsBackend::new(
                metrics_config.resource_path.clone().into(),
                metrics_config.port,
                task_manager.clone(),
            )?);
            backends.insert("metrics".to_string(), metrics_backend);

            let entities: HashSet<EntityType> = metrics_config
                .entities
                .iter()
                .filter_map(|s| parse_entity_type(s))
                .collect();
            all_entities.extend(entities.iter().cloned());
            backend_entities.insert("metrics".to_string(), entities);
            backend_fallbacks.insert("metrics".to_string(), metrics_config.fallback.clone());
        }

        // Create Vitess backend if configured
        #[cfg(feature = "vitess")]
        if let Some(vitess_config) = &self.vitess {
            use stratum_apps::persistence::vitess::{VitessBackend, VitessConfig};

            let vitess_cfg = VitessConfig {
                connection_string: vitess_config.connection_string.clone(),
                pool_size: vitess_config.pool_size,
                channel_size: vitess_config.channel_size,
                batch_size: vitess_config.batch_size,
                batch_timeout_ms: vitess_config.batch_timeout_ms,
                retry_max_attempts: vitess_config.retry_max_attempts,
                retry_timeout_secs: vitess_config.retry_timeout_secs,
                fallback_file_path: vitess_config.fallback_file_path.clone(),
            };

            // Create backend synchronously (constructor spawns async worker)
            let runtime = tokio::runtime::Handle::current();
            let vitess_backend = runtime.block_on(async {
                VitessBackend::new(vitess_cfg, task_manager.clone()).await
            })?;

            backends.insert("vitess".to_string(), std::sync::Arc::new(vitess_backend));

            let entities: HashSet<EntityType> = vitess_config
                .entities
                .iter()
                .filter_map(|s| parse_entity_type(s))
                .collect();
            all_entities.extend(entities.iter().cloned());
            backend_entities.insert("vitess".to_string(), entities);
            backend_fallbacks.insert("vitess".to_string(), vitess_config.fallback.clone());
        }

        // If no backends configured, return error
        if backends.is_empty() {
            return Err(stratum_apps::persistence::Error::Custom(
                "No persistence backends configured. Add [persistence.file], [persistence.metrics], or [persistence.vitess] section.".to_string(),
            ));
        }

        // Build routes from backends
        let mut routes = Vec::new();
        for (name, backend) in &backends {
            let entities = backend_entities.get(name).cloned().unwrap_or_default();
            let mut route = BackendRoute::new(name.clone(), backend.clone(), entities);

            // Wire up fallback if specified
            if let Some(Some(fallback_name)) = backend_fallbacks.get(name) {
                if let Some(fallback_backend) = backends.get(fallback_name) {
                    route = route.with_fallback(fallback_backend.clone());
                } else {
                    return Err(stratum_apps::persistence::Error::Custom(format!(
                        "Fallback '{}' not found for backend '{}'",
                        fallback_name, name
                    )));
                }
            }

            routes.push(route);
        }

        // Always use composite backend internally (even for single backend)
        let backend = Backend::Composite(CompositeBackend::new(routes));

        Ok(Persistence::with_backend(backend, all_entities))
    }
}

impl PoolConfig {
    /// Creates a new instance of the [`PoolConfig`].
    ///
    /// # Panics
    ///
    /// Panics if `coinbase_reward_script` is empty.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pool_connection: ConnectionConfig,
        template_provider_type: TemplateProviderType,
        authority_config: AuthorityConfig,
        coinbase_reward_script: CoinbaseRewardScript,
        shares_per_minute: SharesPerMinute,
        share_batch_size: SharesBatchSize,
        server_id: u16,
        supported_extensions: Vec<u16>,
        required_extensions: Vec<u16>,
        #[cfg(feature = "persistence")] persistence: Option<PersistenceConfig>,
    ) -> Self {
        Self {
            listen_address: pool_connection.listen_address,
            template_provider_type,
            authority_public_key: authority_config.public_key,
            authority_secret_key: authority_config.secret_key,
            cert_validity_sec: pool_connection.cert_validity_sec,
            coinbase_reward_script,
            pool_signature: pool_connection.signature,
            shares_per_minute,
            share_batch_size,
            log_file: None,
            server_id,
            supported_extensions,
            required_extensions,
            #[cfg(feature = "persistence")]
            persistence,
        }
    }

    /// Returns the coinbase output.
    pub fn coinbase_reward_script(&self) -> &CoinbaseRewardScript {
        &self.coinbase_reward_script
    }

    /// Returns Pool listenining address.
    pub fn listen_address(&self) -> &SocketAddr {
        &self.listen_address
    }

    /// Returns the authority public key.
    pub fn authority_public_key(&self) -> &Secp256k1PublicKey {
        &self.authority_public_key
    }

    /// Returns the authority secret key.
    pub fn authority_secret_key(&self) -> &Secp256k1SecretKey {
        &self.authority_secret_key
    }

    /// Returns the certificate validity in seconds.
    pub fn cert_validity_sec(&self) -> u64 {
        self.cert_validity_sec
    }

    /// Returns the Pool signature.
    pub fn pool_signature(&self) -> &String {
        &self.pool_signature
    }

    /// Returns the Template Provider type.
    pub fn template_provider_type(&self) -> &TemplateProviderType {
        &self.template_provider_type
    }

    /// Returns the share batch size.
    pub fn share_batch_size(&self) -> usize {
        self.share_batch_size
    }

    /// Sets the coinbase output.
    pub fn set_coinbase_reward_script(&mut self, coinbase_output: CoinbaseRewardScript) {
        self.coinbase_reward_script = coinbase_output;
    }

    /// Returns the shares per minute.
    pub fn shares_per_minute(&self) -> f32 {
        self.shares_per_minute
    }

    /// Returns the supported extensions.
    pub fn supported_extensions(&self) -> &[u16] {
        &self.supported_extensions
    }

    /// Returns the required extensions.
    pub fn required_extensions(&self) -> &[u16] {
        &self.required_extensions
    }

    /// Sets the log directory.
    pub fn set_log_dir(&mut self, log_dir: Option<PathBuf>) {
        if let Some(dir) = log_dir {
            self.log_file = Some(dir);
        }
    }
    /// Returns the log directory.
    pub fn log_dir(&self) -> Option<&Path> {
        self.log_file.as_deref()
    }

    /// Returns the server id.
    pub fn server_id(&self) -> u16 {
        self.server_id
    }

    /// Returns the persistence configuration.
    ///
    /// Only available when the `persistence` feature is enabled.
    #[cfg(feature = "persistence")]
    pub fn persistence(&self) -> Option<&PersistenceConfig> {
        self.persistence.as_ref()
    }

    pub fn get_txout(&self) -> TxOut {
        TxOut {
            value: Amount::from_sat(0),
            script_pubkey: self.coinbase_reward_script.script_pubkey().to_owned(),
        }
    }
}

/// Pool's authority public and secret keys.
pub struct AuthorityConfig {
    pub public_key: Secp256k1PublicKey,
    pub secret_key: Secp256k1SecretKey,
}

impl AuthorityConfig {
    pub fn new(public_key: Secp256k1PublicKey, secret_key: Secp256k1SecretKey) -> Self {
        Self {
            public_key,
            secret_key,
        }
    }
}

/// Connection settings for the Pool listener.
pub struct ConnectionConfig {
    listen_address: SocketAddr,
    cert_validity_sec: u64,
    signature: String,
}

impl ConnectionConfig {
    pub fn new(listen_address: SocketAddr, cert_validity_sec: u64, signature: String) -> Self {
        Self {
            listen_address,
            cert_validity_sec,
            signature,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "persistence")]
    #[tokio::test]
    async fn test_persistence_config_file_backend() {
        use std::path::PathBuf;
        use stratum_apps::persistence::IntoPersistence;

        #[cfg(feature = "metrics")]
        let config = PersistenceConfig {
            file: Some(FileBackendConfig {
                file_path: PathBuf::from("/tmp/test_pool_persistence.log"),
                channel_size: 5000,
                entities: vec!["shares".to_string()],
                fallback: None,
            }),
            metrics: None,
        };
        #[cfg(not(feature = "metrics"))]
        let config = PersistenceConfig {
            file: Some(FileBackendConfig {
                file_path: PathBuf::from("/tmp/test_pool_persistence.log"),
                channel_size: 5000,
                entities: vec!["shares".to_string()],
                fallback: None,
            }),
        };

        // Create a TaskManager for the test
        let task_manager = Arc::new(TaskManager::new());

        // Test that config can be converted to Persistence
        let result = config.into_persistence(task_manager);
        assert!(result.is_ok());

        // Clean up test file if created
        let _ = std::fs::remove_file("/tmp/test_pool_persistence.log");
    }

    #[cfg(feature = "persistence")]
    #[tokio::test]
    async fn test_persistence_config_no_backends() {
        use stratum_apps::persistence::IntoPersistence;

        #[cfg(feature = "metrics")]
        let config = PersistenceConfig {
            file: None,
            metrics: None,
        };
        #[cfg(not(feature = "metrics"))]
        let config = PersistenceConfig { file: None };

        // Create a TaskManager for the test
        let task_manager = Arc::new(TaskManager::new());

        // Should fail because no backends are configured
        let result = config.into_persistence(task_manager);
        assert!(result.is_err());
        let err_msg = format!("{:?}", result.unwrap_err());
        assert!(err_msg.contains("No persistence backends configured"));
    }

    #[cfg(feature = "persistence")]
    #[tokio::test]
    async fn test_persistence_config_entity_filtering() {
        use std::path::PathBuf;
        use stratum_apps::persistence::IntoPersistence;

        #[cfg(feature = "metrics")]
        let config = PersistenceConfig {
            file: Some(FileBackendConfig {
                file_path: PathBuf::from("/tmp/test_entity_filter.log"),
                channel_size: 5000,
                entities: vec![
                    "shares".to_string(),
                    "unknown_entity".to_string(), // Should be filtered out
                ],
                fallback: None,
            }),
            metrics: None,
        };
        #[cfg(not(feature = "metrics"))]
        let config = PersistenceConfig {
            file: Some(FileBackendConfig {
                file_path: PathBuf::from("/tmp/test_entity_filter.log"),
                channel_size: 5000,
                entities: vec![
                    "shares".to_string(),
                    "unknown_entity".to_string(), // Should be filtered out
                ],
                fallback: None,
            }),
        };

        // Create a TaskManager for the test
        let task_manager = Arc::new(TaskManager::new());

        // Should succeed and filter out unknown entities
        let result = config.into_persistence(task_manager);
        assert!(result.is_ok());

        // Clean up
        let _ = std::fs::remove_file("/tmp/test_entity_filter.log");
    }

    #[cfg(feature = "persistence")]
    #[test]
    fn test_file_backend_config_channel_size() {
        use std::path::PathBuf;

        // Test that FileBackendConfig can be created with custom channel_size
        let config = FileBackendConfig {
            file_path: PathBuf::from("/tmp/test.log"),
            channel_size: 5000,
            entities: vec!["shares".to_string()],
            fallback: None,
        };
        assert_eq!(config.channel_size, 5000);
    }

    #[cfg(all(feature = "persistence", feature = "metrics"))]
    #[tokio::test]
    async fn test_persistence_config_metrics_with_file_fallback() {
        use std::path::PathBuf;
        use stratum_apps::persistence::IntoPersistence;

        let config = PersistenceConfig {
            file: Some(FileBackendConfig {
                file_path: PathBuf::from("/tmp/test_fallback.log"),
                channel_size: 5000,
                entities: vec!["shares".to_string()],
                fallback: None, // File is the last resort
            }),
            metrics: Some(MetricsBackendConfig {
                resource_path: "/metrics".to_string(),
                port: 19091, // Use different port to avoid conflicts
                entities: vec!["shares".to_string()],
                fallback: Some("file".to_string()), // Fall back to file
            }),
        };

        // Create a TaskManager for the test
        let task_manager = Arc::new(TaskManager::new());

        let result = config.into_persistence(task_manager);
        assert!(result.is_ok());

        // Clean up
        let _ = std::fs::remove_file("/tmp/test_fallback.log");
    }

    #[cfg(feature = "persistence")]
    #[tokio::test]
    async fn test_persistence_config_invalid_fallback() {
        use std::path::PathBuf;
        use stratum_apps::persistence::IntoPersistence;

        #[cfg(feature = "metrics")]
        let config = PersistenceConfig {
            file: Some(FileBackendConfig {
                file_path: PathBuf::from("/tmp/test_invalid_fallback.log"),
                channel_size: 5000,
                entities: vec!["shares".to_string()],
                fallback: Some("nonexistent".to_string()), // Invalid fallback
            }),
            metrics: None,
        };
        #[cfg(not(feature = "metrics"))]
        let config = PersistenceConfig {
            file: Some(FileBackendConfig {
                file_path: PathBuf::from("/tmp/test_invalid_fallback.log"),
                channel_size: 5000,
                entities: vec!["shares".to_string()],
                fallback: Some("nonexistent".to_string()), // Invalid fallback
            }),
        };

        // Create a TaskManager for the test
        let task_manager = Arc::new(TaskManager::new());

        // Should fail because fallback doesn't exist
        let result = config.into_persistence(task_manager);
        assert!(result.is_err());
        let err_msg = format!("{:?}", result.unwrap_err());
        assert!(err_msg.contains("Fallback 'nonexistent' not found"));
    }
}
