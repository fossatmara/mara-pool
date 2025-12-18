# Deadlock Analysis Report: mara-pool / sv2-pool Share Processing Halt

**Date:** December 18, 2025  
**Issue:** Pool stops processing shares after 2min-2hr of operation with 10 miners via translation proxy  
**Symptoms:** No panic, no crash, output halts completely, shares stop being found

---

## Executive Summary

After analyzing the logs, screenshots, and source code, I identified **several potential deadlock sources** and **channel backpressure issues** that could cause the observed behavior. The most likely culprits are:

1. **Nested mutex locks** in the channel manager and downstream handling code
2. **Bounded broadcast channel with insufficient capacity** for high-throughput scenarios
3. **Synchronous lock acquisition within async contexts** that can block the Tokio runtime

---

## Analysis Methodology

### 1. Log Timeline Analysis

I examined the timestamps in both logs to understand the sequence of events:

| Time | tproxy-second.log | pool-output-second.log |
|------|-------------------|------------------------|
| 15:57:38 | Processing shares | **Last SubmitSharesExtended received** |
| 15:58:09 | **Last mining.submit processed** | Vardiff loop running (no shares) |
| 16:07:57+ | N/A | RequestedMaxTargetOutOfRange warnings every minute |
| 16:15:58 | N/A | SocketClosed error, downstream disconnected |

**Key Observation:** The pool stopped receiving shares ~30 seconds **before** the tproxy stopped processing them. This indicates the bottleneck is in the pool→tproxy communication path, not the miners→tproxy path.

### 2. Screenshot Analysis

**Screenshot 1 (10:11:35)** - Grafana Dashboard:
- Shows hashrate dropping to zero after ~16:00-17:00
- Valid share rate drops to zero at the same time
- Share difficulty convergence graph shows activity stopping abruptly
- This pattern is consistent with a deadlock (sudden halt, not gradual degradation)

**Screenshot 2 (11:13:34)** - Terminal Output:
- Shows tproxy processes still running (`ps aux` output)
- Both `translator_sv2` and `trans` processes are alive
- CPU usage is minimal (0.0-0.4%), indicating processes are blocked, not spinning

---

## Identified Potential Deadlock Sources

### Issue #1: Nested Mutex Locks (CRITICAL)

**Location:** Multiple files in `pool-apps/pool/src/lib/channel_manager/`

The code uses a custom `Mutex` wrapper with `super_safe_lock()` method. Throughout the codebase, there are **nested lock acquisitions** where an outer lock on `channel_manager_data` contains inner locks on `downstream.downstream_data`.

**Example from `template_distribution_message_handler.rs` (lines 36-47):**
```rust
let messages = self.channel_manager_data.super_safe_lock(|channel_manager_data| {
    // ... 
    for (downstream_id, downstream) in channel_manager_data.downstream.iter_mut() {
        let messages_ = downstream.downstream_data.super_safe_lock(|data| {  // NESTED LOCK
            // ... processing ...
        });
    }
});
```

**Example from `mining_message_handler.rs` (lines 62-72):**
```rust
self.channel_manager_data.super_safe_lock(|channel_manager_data| {
    channel_manager_data.downstream.get(&downstream_id)
        .map(|downstream| {
            downstream.downstream_data.super_safe_lock(|data| {  // NESTED LOCK
                data.negotiated_extensions.clone()
            })
        })
});
```

**Example from `mod.rs` (lines 508-538) - vardiff loop:**
```rust
self.channel_manager_data.super_safe_lock(|channel_manager_data| {
    for (vardiff_key, vardiff_state) in channel_manager_data.vardiff.iter_mut() {
        // ...
        downstream.downstream_data.super_safe_lock(|data| {  // NESTED LOCK
            // vardiff operations
        });
    }
});
```

**Why this causes deadlocks:**

If two tasks attempt to acquire locks in different orders:
- Task A: Acquires `channel_manager_data` lock, then tries to acquire `downstream_data` lock
- Task B: Acquires `downstream_data` lock (from a different code path), then tries to acquire `channel_manager_data` lock

This creates a classic deadlock scenario. The `std::sync::Mutex` used here will block indefinitely.

### Issue #2: Bounded Broadcast Channel Capacity (HIGH)

**Location:** `pool-apps/pool/src/lib/mod.rs` (line 66)

```rust
let (channel_manager_to_downstream_sender, _channel_manager_to_downstream_receiver) =
    broadcast::channel(10);  // Only 10 slots!
```

With 10 miners each submitting shares rapidly:
- The broadcast channel has only 10 message slots
- If receivers are slow (e.g., blocked waiting for a lock), the channel fills up
- Senders will experience **lagged receivers** or block

**Contrast with translator proxy:** The tproxy uses `broadcast::channel(100)` for its sv1_server_to_downstream channel, which is 10x larger.

**Evidence from logs:** The pool continues receiving new templates and running vardiff loops after shares stop, suggesting the template→pool path works but the pool→downstream path is blocked.

### Issue #3: Synchronous Locks in Async Context (MEDIUM)

**Location:** Throughout the codebase

The `super_safe_lock` method uses `std::sync::Mutex::lock()`, which is a **blocking** operation. When called inside an async task:

```rust
async fn handle_template_provider_message(&mut self) -> PoolResult<()> {
    // ...
    let messages = self.channel_manager_data.super_safe_lock(|data| {  // BLOCKS THREAD
        // Long operation inside lock
    });
    // ...
}
```

If the lock is held for a significant time (especially with nested locks), this blocks the Tokio runtime thread, preventing other tasks from making progress.

### Issue #4: Vardiff RequestedMaxTargetOutOfRange Warnings (LOW-MEDIUM)

**Location:** Logs show repeated warnings

```
WARN pool_sv2::channel_manager: Failed to update extended channel 
     channel_id=1 during vardiff RequestedMaxTargetOutOfRange
```

This occurs every 60 seconds during the vardiff cycle. While not directly causing deadlocks, it indicates:
- The vardiff system is trying to update channel targets
- These updates are failing with an out-of-range error
- The channel state may be inconsistent

This could be a symptom of the upstream channel state being stale due to blocked message delivery.

### Issue #5: Similar Nested Lock Pattern in Translator Proxy

**Location:** `miner-apps/translator/src/lib/sv1/sv1_server/difficulty_manager.rs`

The translator proxy has the same nested lock pattern:

```rust
sv1_server_data.super_safe_lock(|data| {
    data.downstreams.get(downstream_id).and_then(|ds| {
        ds.downstream_data.super_safe_lock(|d| {  // NESTED LOCK
            // ...
        })
    })
});
```

This appears in multiple places (lines 118-132, 261-272, 275-287, etc.) and could cause the same deadlock issues.

---

## Correlation with Observed Behavior

### Why the halt happens after 2min-2hr (not immediately):

1. **Lock contention increases over time** as more shares are submitted
2. **Broadcast channel fills up** as share rate increases with difficulty convergence
3. **Race conditions** require specific timing to trigger the deadlock
4. With 10 miners at 10TH each, share submission rate is high enough to eventually trigger the race

### Why there's no panic:

1. `std::sync::Mutex` blocks forever on contention (no timeout)
2. The `super_safe_lock` method unwraps poison errors but doesn't detect deadlocks
3. Tokio tasks remain alive but blocked

### Why both pool and tproxy are affected:

Both use the same nested lock pattern from the shared codebase architecture. The issue originates in one component but cascades:
1. Pool gets blocked waiting for a lock
2. Pool stops sending responses to tproxy
3. Tproxy's channels fill up waiting for pool
4. Tproxy stops accepting new shares from miners

---

## Recommended Fixes

### Fix #1: Eliminate Nested Locks (Priority: CRITICAL)

Refactor to acquire locks independently and release before acquiring the next:

```rust
// BEFORE (deadlock-prone):
self.channel_manager_data.super_safe_lock(|cm_data| {
    for downstream in cm_data.downstream.values() {
        downstream.downstream_data.super_safe_lock(|d| { ... });
    }
});

// AFTER (safe):
let downstream_ids: Vec<_> = self.channel_manager_data
    .super_safe_lock(|cm_data| cm_data.downstream.keys().cloned().collect());

for downstream_id in downstream_ids {
    let downstream = self.channel_manager_data
        .super_safe_lock(|cm_data| cm_data.downstream.get(&downstream_id).cloned());
    if let Some(downstream) = downstream {
        downstream.downstream_data.super_safe_lock(|d| { ... });
    }
}
```

### Fix #2: Increase Broadcast Channel Capacity

```rust
// BEFORE:
let (channel_manager_to_downstream_sender, _) = broadcast::channel(10);

// AFTER:
let (channel_manager_to_downstream_sender, _) = broadcast::channel(1000);
```

### Fix #3: Use Async-Aware Locks

Replace `std::sync::Mutex` with `tokio::sync::Mutex` for locks held across `.await` points:

```rust
// In custom_mutex.rs, add an async variant:
pub struct AsyncMutex<T>(tokio::sync::Mutex<T>);

impl<T> AsyncMutex<T> {
    pub async fn lock_async<F, Ret>(&self, thunk: F) -> Ret
    where
        F: FnOnce(&mut T) -> Ret,
    {
        let mut guard = self.0.lock().await;
        thunk(&mut *guard)
    }
}
```

### Fix #4: Add Lock Timeout and Monitoring

Add a timeout to detect potential deadlocks:

```rust
pub fn safe_lock_with_timeout<F, Ret>(
    &self, 
    thunk: F, 
    timeout: Duration
) -> Result<Ret, DeadlockError>
where
    F: FnOnce(&mut T) -> Ret,
{
    let start = Instant::now();
    loop {
        match self.0.try_lock() {
            Ok(mut guard) => return Ok(thunk(&mut *guard)),
            Err(TryLockError::WouldBlock) => {
                if start.elapsed() > timeout {
                    error!("Potential deadlock detected!");
                    return Err(DeadlockError::Timeout);
                }
                std::thread::yield_now();
            }
            Err(TryLockError::Poisoned(e)) => panic!("Lock poisoned: {e}"),
        }
    }
}
```

### Fix #5: Establish Lock Ordering Convention

Document and enforce a strict lock acquisition order:
1. Always acquire `channel_manager_data` first
2. Always acquire `downstream_data` second
3. Never hold `downstream_data` when attempting to acquire `channel_manager_data`

---

## Testing Recommendations

1. **Stress test with higher miner counts** (20, 50, 100 miners) to trigger the race faster
2. **Add tracing for lock acquisition/release** to identify contention points
3. **Use `tokio-console`** to monitor task states and identify blocked tasks
4. **Consider using `parking_lot::Mutex`** with deadlock detection feature enabled during testing

---

## Files Requiring Changes

| File | Issue | Priority |
|------|-------|----------|
| `pool-apps/pool/src/lib/channel_manager/mod.rs` | Nested locks in vardiff | CRITICAL |
| `pool-apps/pool/src/lib/channel_manager/mining_message_handler.rs` | Nested locks | CRITICAL |
| `pool-apps/pool/src/lib/channel_manager/template_distribution_message_handler.rs` | Nested locks | CRITICAL |
| `pool-apps/pool/src/lib/mod.rs` | Broadcast channel capacity | HIGH |
| `miner-apps/translator/src/lib/sv1/sv1_server/difficulty_manager.rs` | Nested locks | CRITICAL |
| `miner-apps/translator/src/lib/sv2/channel_manager/channel_manager.rs` | Nested locks | HIGH |
| `stratum-apps/src/custom_mutex.rs` | Add async-aware variant | MEDIUM |

---

## Conclusion

The primary cause of the share processing halt is most likely **nested mutex locks** combined with **insufficient broadcast channel capacity**. The nested locks create a classic deadlock scenario that manifests under high-throughput conditions (10 miners × 10TH), while the small broadcast channel capacity (10) creates backpressure that exacerbates lock contention.

The fix requires refactoring the lock acquisition patterns throughout the codebase to eliminate nested locks and increasing channel capacities to handle burst traffic. This is a significant architectural change but necessary for production stability.
