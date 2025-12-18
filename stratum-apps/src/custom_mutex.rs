//! # Collection of Helper Primitives
//!
//! Provides a collection of utilities and helper structures used throughout the Stratum V2
//! protocol implementation. These utilities simplify common tasks, such as ID generation and
//! management, mutex management, difficulty target calculations, merkle root calculations, and
//! more.

use std::sync::{Mutex as Mutex_, MutexGuard, PoisonError, TryLockError};
use std::time::{Duration, Instant};
use tracing::warn;

/// Custom synchronization primitive for managing shared mutable state.
///
/// This custom mutex implementation builds on [`std::sync::Mutex`] to enhance usability and safety
/// in concurrent environments. It provides ergonomic methods to safely access and modify inner
/// values while reducing the risk of deadlocks and panics. It is used throughout SRI applications
/// to managed shared state across multiple threads, such as tracking active mining sessions,
/// routing jobs, and managing connections safely and efficiently.
///
/// ## Advantages
/// - **Closure-Based Locking:** The `safe_lock` method encapsulates the locking process, ensuring
///   the lock is automatically released after the closure completes.
/// - **Error Handling:** `safe_lock` enforces explicit handling of potential [`PoisonError`]
///   conditions, reducing the risk of panics caused by poisoned locks.
/// - **Panic-Safe Option:** The `super_safe_lock` method provides an alternative that unwraps the
///   result of `safe_lock`, with optional runtime safeguards against panics.
/// - **Extensibility:** Includes feature-gated functionality to customize behavior, such as
///   stricter runtime checks using external tools like
///   [`no-panic`](https://github.com/dtolnay/no-panic).
#[derive(Debug)]
pub struct Mutex<T: ?Sized>(Mutex_<T>);

impl<T> Mutex<T> {
    /// Mutex safe lock.
    ///
    /// Safely locks the `Mutex` and executes a closer (`thunk`) with a mutable reference to the
    /// inner value. This ensures that the lock is automatically released after the closure
    /// completes, preventing deadlocks. It explicitly returns a [`PoisonError`] containing a
    /// [`MutexGuard`] to the inner value in cases where the lock is poisoned.
    ///
    /// To prevent poison lock errors, unwraps should never be used within the closure. The result
    /// should always be returned and handled outside of the sage lock.
    pub fn safe_lock<F, Ret>(&self, thunk: F) -> Result<Ret, PoisonError<MutexGuard<'_, T>>>
    where
        F: FnOnce(&mut T) -> Ret,
    {
        let mut lock = self.0.lock()?;
        let return_value = thunk(&mut *lock);
        drop(lock);
        Ok(return_value)
    }

    /// Mutex super safe lock.
    ///
    /// Locks the `Mutex` and executes a closure (`thunk`) with a mutable reference to the inner
    /// value, panicking if the lock is poisoned.
    ///
    /// This is a convenience wrapper around `safe_lock` for cases where explicit error handling is
    /// unnecessary or undesirable. Use with caution in production code.
    #[track_caller]
    pub fn super_safe_lock<F, Ret>(&self, thunk: F) -> Ret
    where
        F: FnOnce(&mut T) -> Ret,
    {
        //#[cfg(feature = "disable_nopanic")]
        {
            // Deadlock detection: warn if lock takes > 5 seconds
            let caller = std::panic::Location::caller();
            let start = Instant::now();
            let warn_threshold = Duration::from_secs(5);
            let mut warned = false;

            loop {
                match self.0.try_lock() {
                    Ok(mut guard) => {
                        let wait_time = start.elapsed();
                        if wait_time > Duration::from_millis(100) {
                            warn!(
                                "Lock acquired after {:?} at {}:{} - potential contention",
                                wait_time,
                                caller.file(),
                                caller.line()
                            );
                        }
                        let return_value = thunk(&mut *guard);
                        drop(guard);
                        return return_value;
                    }
                    Err(TryLockError::WouldBlock) => {
                        if start.elapsed() > warn_threshold && !warned {
                            warn!(
                                "POTENTIAL DEADLOCK at {}:{}: Lock held for > {:?}, still waiting...",
                                caller.file(), caller.line(), warn_threshold
                            );
                            warned = true;
                        }
                        std::thread::yield_now();
                    }
                    Err(TryLockError::Poisoned(e)) => {
                        panic!("Lock poisoned: {e}");
                    }
                }
            }
        }
        //#[cfg(not(feature = "disable_nopanic"))]
        //{
        //    // based on https://github.com/dtolnay/no-panic
        //    struct __NoPanic;
        //    extern "C" {
        //        #[link_name = "super_safe_lock called on a function that may panic"]
        //        fn trigger() -> !;
        //    }
        //    impl core::ops::Drop for __NoPanic {
        //        fn drop(&mut self) {
        //            unsafe {
        //                trigger();
        //            }
        //        }
        //    }
        //    let mut lock = self.0.lock().expect("threads to never panic");
        //    let __guard = __NoPanic;
        //    let return_value = thunk(&mut *lock);
        //    core::mem::forget(__guard);
        //    drop(lock);
        //    return_value
        //}
    }

    /// Creates a new [`Mutex`] instance, storing the initial value inside.
    pub fn new(v: T) -> Self {
        Mutex(Mutex_::new(v))
    }

    /// Removes lock for direct access.
    ///
    /// Acquires a lock on the [`Mutex`] and returns a [`MutexGuard`] for direct access to the
    /// inner value. Allows for manual lock handling and is useful in scenarios where closures are
    /// not convenient.
    pub fn to_remove(&self) -> Result<MutexGuard<'_, T>, PoisonError<MutexGuard<'_, T>>> {
        self.0.lock()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_super_safe_lock() {
        let m = super::Mutex::new(1u32);
        m.safe_lock(|i| *i += 1).unwrap();
        // m.super_safe_lock(|i| *i = (*i).checked_add(1).unwrap()); // will not compile
        m.super_safe_lock(|i| *i = (*i).checked_add(1).unwrap_or_default()); // compiles
    }

    /// Test that demonstrates nested lock pattern that can cause deadlock.
    /// This test will timeout/hang if run with --ignored flag if deadlock occurs.
    /// Run with: cargo test test_nested_lock_deadlock_scenario -- --ignored --nocapture
    #[test]
    #[ignore] // Ignored by default as it may hang
    fn test_nested_lock_deadlock_scenario() {
        // Simulates the pattern found in channel_manager code:
        // - Lock A contains references to Lock B
        // - Two threads try to acquire in different orders

        struct Inner {
            value: u32,
        }

        struct Outer {
            inner: Arc<Mutex<Inner>>,
        }

        let inner = Arc::new(Mutex::new(Inner { value: 0 }));
        let outer = Arc::new(Mutex::new(Outer {
            inner: inner.clone(),
        }));

        let outer_clone = outer.clone();
        let inner_clone = inner.clone();

        // Thread 1: Acquires outer, then inner (like template_distribution_message_handler)
        let t1 = thread::spawn(move || {
            for _ in 0..1000 {
                outer_clone.super_safe_lock(|o| {
                    o.inner.super_safe_lock(|i| {
                        i.value += 1;
                    });
                });
            }
        });

        // Thread 2: Acquires inner directly (like a downstream handler might)
        let t2 = thread::spawn(move || {
            for _ in 0..1000 {
                inner_clone.super_safe_lock(|i| {
                    i.value += 1;
                    // Simulate some work
                    thread::sleep(Duration::from_micros(1));
                });
            }
        });

        // If this test hangs, it confirms the nested lock pattern can deadlock
        t1.join().expect("Thread 1 should complete");
        t2.join().expect("Thread 2 should complete");

        println!("Test completed without deadlock (this time)");
    }

    /// Test high contention scenario - simulates 10 miners submitting shares
    /// Run with: cargo test test_high_contention -- --ignored --nocapture
    #[test]
    #[ignore]
    fn test_high_contention() {
        let shared_data = Arc::new(Mutex::new(vec![0u64; 10]));
        let mut handles = vec![];

        // Simulate 10 "miners" all trying to update shared state
        for miner_id in 0..10 {
            let data = shared_data.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..10000 {
                    data.super_safe_lock(|v| {
                        v[miner_id] += 1;
                    });
                }
            }));
        }

        for h in handles {
            h.join().expect("Miner thread should complete");
        }

        let total: u64 = shared_data.super_safe_lock(|v| v.iter().sum());
        assert_eq!(total, 100000, "All increments should be counted");
        println!("High contention test passed - total: {}", total);
    }
}
