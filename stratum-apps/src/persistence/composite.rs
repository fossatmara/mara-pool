//! Composite persistence backend that routes events to multiple child backends.
//!
//! This module provides a `CompositeBackend` that can:
//! - Route events to multiple backends simultaneously
//! - Filter entities per-backend (e.g., shares to metrics, blocks to database)
//! - Define fallback chains for error recovery
//!
//! # Example Configuration
//!
//! ```toml
//! [persistence]
//! backend = "composite"
//! entities = ["shares", "blocks"]
//!
//! [[persistence.composite.routes]]
//! name = "database"
//! backend = "database"
//! entities = ["shares", "blocks"]
//! fallback = "file"
//!
//! [[persistence.composite.routes]]
//! name = "metrics"
//! backend = "metrics"
//! entities = ["shares"]
//! # No fallback - errors are dropped
//! ```

use super::{EntityType, PersistenceBackend, PersistenceError, PersistenceEvent};
use std::collections::HashSet;
use std::sync::Arc;

/// A single route in the composite backend.
///
/// Each route has:
/// - A primary backend to send events to
/// - A set of entity types it handles
/// - An optional fallback backend for error recovery
#[derive(Debug)]
pub struct BackendRoute {
    /// Human-readable name for this route (for logging)
    pub name: String,
    /// The primary backend for this route
    pub backend: Arc<dyn PersistenceBackend>,
    /// Entity types this route handles (filters from global entities)
    pub entities: HashSet<EntityType>,
    /// Optional fallback backend if primary fails
    pub fallback: Option<Arc<dyn PersistenceBackend>>,
}

impl BackendRoute {
    /// Create a new backend route.
    pub fn new(
        name: impl Into<String>,
        backend: Arc<dyn PersistenceBackend>,
        entities: impl IntoIterator<Item = EntityType>,
    ) -> Self {
        Self {
            name: name.into(),
            backend,
            entities: entities.into_iter().collect(),
            fallback: None,
        }
    }

    /// Set a fallback backend for this route.
    pub fn with_fallback(mut self, fallback: Arc<dyn PersistenceBackend>) -> Self {
        self.fallback = Some(fallback);
        self
    }

    /// Check if this route handles the given entity type.
    #[inline]
    pub fn handles(&self, entity_type: EntityType) -> bool {
        self.entities.contains(&entity_type)
    }
}

/// Composite backend that routes events to multiple child backends.
///
/// This backend enables:
/// - **Multi-destination persistence**: Send events to multiple backends
/// - **Entity filtering**: Each route can handle a subset of entity types
/// - **Fallback chains**: Define backup backends for error recovery
///
/// # Example
///
/// ```ignore
/// use stratum_apps::persistence::{CompositeBackend, BackendRoute, FileBackend, MetricsBackend};
///
/// let file_backend = Arc::new(FileBackend::new(...)?);
/// let metrics_backend = Arc::new(MetricsBackend::new(...)?);
///
/// let routes = vec![
///     BackendRoute::new("file", file_backend.clone(), vec![EntityType::Share, EntityType::Block]),
///     BackendRoute::new("metrics", metrics_backend, vec![EntityType::Share]),
/// ];
///
/// let composite = CompositeBackend::new(routes);
/// ```
#[derive(Debug)]
pub struct CompositeBackend {
    routes: Vec<BackendRoute>,
}

impl CompositeBackend {
    /// Create a new composite backend with the given routes.
    pub fn new(routes: Vec<BackendRoute>) -> Self {
        Self { routes }
    }

    /// Create a builder for constructing a composite backend.
    pub fn builder() -> CompositeBackendBuilder {
        CompositeBackendBuilder::new()
    }

    /// Get the number of routes in this composite backend.
    pub fn route_count(&self) -> usize {
        self.routes.len()
    }
}

impl Clone for CompositeBackend {
    fn clone(&self) -> Self {
        // Routes contain Arc, so we need to clone the Vec but Arcs are cheap to clone
        Self {
            routes: self
                .routes
                .iter()
                .map(|r| BackendRoute {
                    name: r.name.clone(),
                    backend: r.backend.clone(),
                    entities: r.entities.clone(),
                    fallback: r.fallback.clone(),
                })
                .collect(),
        }
    }
}

impl PersistenceBackend for CompositeBackend {
    fn persist_event(&self, event: PersistenceEvent) -> Result<(), PersistenceError> {
        let entity_type = event.entity_type();
        let mut last_error: Option<PersistenceError> = None;
        // Track fallbacks that have already received this event to avoid duplicates
        // when multiple routes share the same fallback backend
        let mut used_fallbacks: HashSet<usize> = HashSet::new();

        for route in &self.routes {
            // Skip if this route doesn't handle this entity type
            if !route.handles(entity_type) {
                continue;
            }

            // Try primary backend
            match route.backend.persist_event(event.clone()) {
                Ok(()) => {
                    tracing::trace!(route = %route.name, "Event persisted successfully");
                }
                Err(e) => {
                    tracing::warn!(
                        route = %route.name,
                        error = %e,
                        "Primary backend failed"
                    );

                    // Try fallback if configured
                    if let Some(fallback) = &route.fallback {
                        // Use pointer address to identify unique fallback instances
                        // Cast through *const () first since dyn Trait is a fat pointer
                        let fallback_ptr = Arc::as_ptr(fallback) as *const () as usize;

                        // Skip if we've already written to this fallback
                        if used_fallbacks.contains(&fallback_ptr) {
                            tracing::debug!(
                                route = %route.name,
                                "Skipping duplicate fallback write"
                            );
                            continue;
                        }

                        match fallback.persist_event(event.clone()) {
                            Ok(()) => {
                                used_fallbacks.insert(fallback_ptr);
                                tracing::debug!(
                                    route = %route.name,
                                    "Fallback backend succeeded"
                                );
                            }
                            Err(e2) => {
                                tracing::error!(
                                    route = %route.name,
                                    error = %e2,
                                    "Fallback backend also failed"
                                );
                                last_error = Some(e2);
                            }
                        }
                    } else {
                        // No fallback configured - this route drops the error
                        tracing::debug!(
                            route = %route.name,
                            "No fallback configured, event dropped"
                        );
                    }
                }
            }
        }

        // Return last error if any route failed without recovery
        // This allows the caller to know something went wrong, but we still
        // attempted all routes
        match last_error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    fn flush(&self) -> Result<(), PersistenceError> {
        let mut last_error: Option<PersistenceError> = None;

        for route in &self.routes {
            if let Err(e) = route.backend.flush() {
                tracing::warn!(route = %route.name, error = %e, "Flush failed");
                last_error = Some(e);
            }

            if let Some(fallback) = &route.fallback {
                if let Err(e) = fallback.flush() {
                    tracing::warn!(route = %route.name, error = %e, "Fallback flush failed");
                    last_error = Some(e);
                }
            }
        }

        match last_error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    fn shutdown(&self) -> Result<(), PersistenceError> {
        let mut last_error: Option<PersistenceError> = None;

        for route in &self.routes {
            if let Err(e) = route.backend.shutdown() {
                tracing::warn!(route = %route.name, error = %e, "Shutdown failed");
                last_error = Some(e);
            }

            if let Some(fallback) = &route.fallback {
                if let Err(e) = fallback.shutdown() {
                    tracing::warn!(route = %route.name, error = %e, "Fallback shutdown failed");
                    last_error = Some(e);
                }
            }
        }

        match last_error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

/// Builder for constructing a `CompositeBackend`.
#[derive(Default)]
pub struct CompositeBackendBuilder {
    routes: Vec<BackendRoute>,
}

impl CompositeBackendBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self { routes: Vec::new() }
    }

    /// Add a route to the composite backend.
    pub fn add_route(mut self, route: BackendRoute) -> Self {
        self.routes.push(route);
        self
    }

    /// Build the composite backend.
    pub fn build(self) -> CompositeBackend {
        CompositeBackend::new(self.routes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::{NoOpBackend, ShareEvent};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::SystemTime;
    use stratum_core::bitcoin::hashes::{sha256d::Hash, Hash as HashTrait};

    fn create_test_event() -> PersistenceEvent {
        let share_hash = Some(Hash::from_byte_array([0u8; 32]));
        PersistenceEvent::Share(ShareEvent {
            error_code: None,
            extranonce_prefix: vec![],
            is_block_found: false,
            is_valid: true,
            nominal_hash_rate: 1.0,
            nonce: 1,
            ntime: 1,
            rollable_extranonce_size: None,
            share_hash,
            share_work: 1.0,
            target: [0; 32],
            template_id: None,
            timestamp: SystemTime::now(),
            user_identity: "test".to_string(),
            version: 1,
        })
    }

    /// A test backend that counts calls
    #[derive(Debug, Clone)]
    struct CountingBackend {
        persist_count: Arc<AtomicUsize>,
        should_fail: bool,
    }

    impl CountingBackend {
        fn new() -> Self {
            Self {
                persist_count: Arc::new(AtomicUsize::new(0)),
                should_fail: false,
            }
        }

        fn failing() -> Self {
            Self {
                persist_count: Arc::new(AtomicUsize::new(0)),
                should_fail: true,
            }
        }

        fn count(&self) -> usize {
            self.persist_count.load(Ordering::SeqCst)
        }
    }

    impl PersistenceBackend for CountingBackend {
        fn persist_event(&self, _event: PersistenceEvent) -> Result<(), PersistenceError> {
            self.persist_count.fetch_add(1, Ordering::SeqCst);
            if self.should_fail {
                Err(PersistenceError::Custom("Test failure".to_string()))
            } else {
                Ok(())
            }
        }

        fn flush(&self) -> Result<(), PersistenceError> {
            Ok(())
        }

        fn shutdown(&self) -> Result<(), PersistenceError> {
            Ok(())
        }
    }

    #[test]
    fn test_composite_routes_to_multiple_backends() {
        let backend1 = Arc::new(CountingBackend::new());
        let backend2 = Arc::new(CountingBackend::new());

        let routes = vec![
            BackendRoute::new("backend1", backend1.clone(), vec![EntityType::Share]),
            BackendRoute::new("backend2", backend2.clone(), vec![EntityType::Share]),
        ];

        let composite = CompositeBackend::new(routes);
        let event = create_test_event();

        assert!(PersistenceBackend::persist_event(&composite, event).is_ok());
        assert_eq!(backend1.count(), 1);
        assert_eq!(backend2.count(), 1);
    }

    #[test]
    fn test_composite_filters_by_entity_type() {
        let backend1 = Arc::new(CountingBackend::new());
        let backend2 = Arc::new(CountingBackend::new());

        let routes = vec![
            BackendRoute::new("backend1", backend1.clone(), vec![EntityType::Share]),
            // backend2 doesn't handle Share events (empty entities)
            BackendRoute::new("backend2", backend2.clone(), vec![]),
        ];

        let composite = CompositeBackend::new(routes);
        let event = create_test_event();

        assert!(PersistenceBackend::persist_event(&composite, event).is_ok());
        assert_eq!(backend1.count(), 1);
        assert_eq!(backend2.count(), 0); // Not called because it doesn't handle Share
    }

    #[test]
    fn test_composite_fallback_on_error() {
        let primary = Arc::new(CountingBackend::failing());
        let fallback = Arc::new(CountingBackend::new());

        let route = BackendRoute::new("test", primary.clone(), vec![EntityType::Share])
            .with_fallback(fallback.clone());

        let composite = CompositeBackend::new(vec![route]);
        let event = create_test_event();

        // Should succeed because fallback works
        assert!(PersistenceBackend::persist_event(&composite, event).is_ok());
        assert_eq!(primary.count(), 1); // Primary was tried
        assert_eq!(fallback.count(), 1); // Fallback was used
    }

    #[test]
    fn test_composite_no_fallback_drops_error() {
        let primary = Arc::new(CountingBackend::failing());

        // No fallback configured
        let route = BackendRoute::new("test", primary.clone(), vec![EntityType::Share]);

        let composite = CompositeBackend::new(vec![route]);
        let event = create_test_event();

        // Should return Ok because the error is dropped (no fallback)
        // The composite doesn't propagate errors when there's no fallback
        assert!(PersistenceBackend::persist_event(&composite, event).is_ok());
        assert_eq!(primary.count(), 1);
    }

    #[test]
    fn test_composite_builder() {
        let backend = Arc::new(NoOpBackend::new());

        let composite = CompositeBackend::builder()
            .add_route(BackendRoute::new("noop", backend, vec![EntityType::Share]))
            .build();

        assert_eq!(composite.route_count(), 1);
    }

    #[test]
    fn test_composite_clone() {
        let backend = Arc::new(CountingBackend::new());
        let route = BackendRoute::new("test", backend.clone(), vec![EntityType::Share]);
        let composite = CompositeBackend::new(vec![route]);

        let cloned = composite.clone();
        let event = create_test_event();

        // Both should share the same underlying backend (Arc)
        assert!(PersistenceBackend::persist_event(&composite, event.clone()).is_ok());
        assert!(PersistenceBackend::persist_event(&cloned, event).is_ok());
        assert_eq!(backend.count(), 2);
    }

    #[test]
    fn test_composite_flush_all_routes() {
        let backend1 = Arc::new(CountingBackend::new());
        let backend2 = Arc::new(CountingBackend::new());

        let routes = vec![
            BackendRoute::new("backend1", backend1.clone(), vec![EntityType::Share]),
            BackendRoute::new("backend2", backend2.clone(), vec![EntityType::Share]),
        ];

        let composite = CompositeBackend::new(routes);
        assert!(PersistenceBackend::flush(&composite).is_ok());
    }

    #[test]
    fn test_composite_shutdown_all_routes() {
        let backend1 = Arc::new(CountingBackend::new());
        let backend2 = Arc::new(CountingBackend::new());

        let routes = vec![
            BackendRoute::new("backend1", backend1.clone(), vec![EntityType::Share]),
            BackendRoute::new("backend2", backend2.clone(), vec![EntityType::Share]),
        ];

        let composite = CompositeBackend::new(routes);
        assert!(PersistenceBackend::shutdown(&composite).is_ok());
    }

    #[test]
    fn test_composite_empty_routes() {
        let composite = CompositeBackend::new(vec![]);
        let event = create_test_event();

        // Should succeed even with no routes
        assert!(PersistenceBackend::persist_event(&composite, event).is_ok());
        assert_eq!(composite.route_count(), 0);
    }

    #[test]
    fn test_composite_fallback_chain() {
        // Test a chain: primary -> fallback1 (fails) -> fallback2 (succeeds)
        // Note: Current implementation only supports single fallback, not chains
        // This test verifies the single fallback behavior
        let primary = Arc::new(CountingBackend::failing());
        let fallback = Arc::new(CountingBackend::new());

        let route = BackendRoute::new("test", primary.clone(), vec![EntityType::Share])
            .with_fallback(fallback.clone());

        let composite = CompositeBackend::new(vec![route]);

        // Send multiple events
        for _ in 0..5 {
            let event = create_test_event();
            assert!(PersistenceBackend::persist_event(&composite, event).is_ok());
        }

        assert_eq!(primary.count(), 5); // All attempts hit primary
        assert_eq!(fallback.count(), 5); // All fell back
    }

    #[test]
    fn test_composite_both_primary_and_fallback_fail() {
        let primary = Arc::new(CountingBackend::failing());
        let fallback = Arc::new(CountingBackend::failing());

        let route = BackendRoute::new("test", primary.clone(), vec![EntityType::Share])
            .with_fallback(fallback.clone());

        let composite = CompositeBackend::new(vec![route]);
        let event = create_test_event();

        // Should return Err when both primary and fallback fail
        // This allows the caller to know the event was not persisted
        let result = PersistenceBackend::persist_event(&composite, event);
        assert!(result.is_err());
        assert_eq!(primary.count(), 1);
        assert_eq!(fallback.count(), 1);
    }

    #[test]
    fn test_backend_route_handles() {
        let backend = Arc::new(NoOpBackend::new());
        let route = BackendRoute::new("test", backend, vec![EntityType::Share]);

        assert!(route.handles(EntityType::Share));
        // Note: Currently only Share entity type exists
    }

    #[test]
    fn test_backend_route_empty_entities_handles_nothing() {
        let backend = Arc::new(NoOpBackend::new());
        let route = BackendRoute::new("test", backend, vec![]);

        assert!(!route.handles(EntityType::Share));
    }

    #[test]
    fn test_composite_multiple_routes_independent_failures() {
        // Route 1: fails, has fallback
        // Route 2: succeeds
        // Both should be attempted independently
        let primary1 = Arc::new(CountingBackend::failing());
        let fallback1 = Arc::new(CountingBackend::new());
        let backend2 = Arc::new(CountingBackend::new());

        let routes = vec![
            BackendRoute::new("route1", primary1.clone(), vec![EntityType::Share])
                .with_fallback(fallback1.clone()),
            BackendRoute::new("route2", backend2.clone(), vec![EntityType::Share]),
        ];

        let composite = CompositeBackend::new(routes);
        let event = create_test_event();

        assert!(PersistenceBackend::persist_event(&composite, event).is_ok());
        assert_eq!(primary1.count(), 1); // Tried
        assert_eq!(fallback1.count(), 1); // Used as fallback
        assert_eq!(backend2.count(), 1); // Independent, succeeded
    }

    #[test]
    fn test_composite_shared_fallback_no_duplicates() {
        // When multiple routes share the same fallback backend and both primaries fail,
        // the fallback should only receive the event once (not once per failing route)
        let primary1 = Arc::new(CountingBackend::failing());
        let primary2 = Arc::new(CountingBackend::failing());
        let shared_fallback = Arc::new(CountingBackend::new());

        let routes = vec![
            BackendRoute::new("route1", primary1.clone(), vec![EntityType::Share])
                .with_fallback(shared_fallback.clone()),
            BackendRoute::new("route2", primary2.clone(), vec![EntityType::Share])
                .with_fallback(shared_fallback.clone()),
        ];

        let composite = CompositeBackend::new(routes);
        let event = create_test_event();

        assert!(PersistenceBackend::persist_event(&composite, event).is_ok());
        assert_eq!(primary1.count(), 1); // Primary 1 was tried
        assert_eq!(primary2.count(), 1); // Primary 2 was tried
        // Fallback should only be called ONCE, not twice
        assert_eq!(shared_fallback.count(), 1);
    }

    #[test]
    fn test_composite_different_fallbacks_both_called() {
        // When routes have different fallback backends, both should be called
        let primary1 = Arc::new(CountingBackend::failing());
        let primary2 = Arc::new(CountingBackend::failing());
        let fallback1 = Arc::new(CountingBackend::new());
        let fallback2 = Arc::new(CountingBackend::new());

        let routes = vec![
            BackendRoute::new("route1", primary1.clone(), vec![EntityType::Share])
                .with_fallback(fallback1.clone()),
            BackendRoute::new("route2", primary2.clone(), vec![EntityType::Share])
                .with_fallback(fallback2.clone()),
        ];

        let composite = CompositeBackend::new(routes);
        let event = create_test_event();

        assert!(PersistenceBackend::persist_event(&composite, event).is_ok());
        assert_eq!(primary1.count(), 1);
        assert_eq!(primary2.count(), 1);
        // Different fallbacks should each be called once
        assert_eq!(fallback1.count(), 1);
        assert_eq!(fallback2.count(), 1);
    }

    /// A backend that tracks flush and shutdown calls
    #[derive(Debug)]
    struct LifecycleTrackingBackend {
        flush_count: AtomicUsize,
        shutdown_count: AtomicUsize,
    }

    impl LifecycleTrackingBackend {
        fn new() -> Self {
            Self {
                flush_count: AtomicUsize::new(0),
                shutdown_count: AtomicUsize::new(0),
            }
        }

        fn flush_count(&self) -> usize {
            self.flush_count.load(Ordering::SeqCst)
        }

        fn shutdown_count(&self) -> usize {
            self.shutdown_count.load(Ordering::SeqCst)
        }
    }

    impl PersistenceBackend for LifecycleTrackingBackend {
        fn persist_event(&self, _event: PersistenceEvent) -> Result<(), PersistenceError> {
            Ok(())
        }

        fn flush(&self) -> Result<(), PersistenceError> {
            self.flush_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn shutdown(&self) -> Result<(), PersistenceError> {
            self.shutdown_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn test_composite_flush_calls_all_backends() {
        let backend1 = Arc::new(LifecycleTrackingBackend::new());
        let backend2 = Arc::new(LifecycleTrackingBackend::new());
        let fallback = Arc::new(LifecycleTrackingBackend::new());

        let routes = vec![
            BackendRoute::new("route1", backend1.clone(), vec![EntityType::Share])
                .with_fallback(fallback.clone()),
            BackendRoute::new("route2", backend2.clone(), vec![EntityType::Share]),
        ];

        let composite = CompositeBackend::new(routes);
        assert!(PersistenceBackend::flush(&composite).is_ok());

        assert_eq!(backend1.flush_count(), 1);
        assert_eq!(backend2.flush_count(), 1);
        assert_eq!(fallback.flush_count(), 1);
    }

    #[test]
    fn test_composite_shutdown_calls_all_backends() {
        let backend1 = Arc::new(LifecycleTrackingBackend::new());
        let backend2 = Arc::new(LifecycleTrackingBackend::new());
        let fallback = Arc::new(LifecycleTrackingBackend::new());

        let routes = vec![
            BackendRoute::new("route1", backend1.clone(), vec![EntityType::Share])
                .with_fallback(fallback.clone()),
            BackendRoute::new("route2", backend2.clone(), vec![EntityType::Share]),
        ];

        let composite = CompositeBackend::new(routes);
        assert!(PersistenceBackend::shutdown(&composite).is_ok());

        assert_eq!(backend1.shutdown_count(), 1);
        assert_eq!(backend2.shutdown_count(), 1);
        assert_eq!(fallback.shutdown_count(), 1);
    }
}
