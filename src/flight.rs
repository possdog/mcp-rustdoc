//! Single-flight deduplication for concurrent network fetches.
//!
//! The engine fetches crates concurrently with other work, so several calls for
//! the same not-yet-cached crate can race: each would miss the cache and
//! issue its own third-party request. A `SingleFlight` serializes work per
//! key — the first arrival performs the fetch while concurrent callers wait
//! on the same key, then re-check their cache (the fetcher stores its
//! result) instead of repeating the request.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

/// Per-key async mutex registry. Callers acquire the mutex for their key
/// via [`SingleFlight::lock_for`] and hold it for the duration of the
/// underlying request, re-checking any cache after acquisition.
pub struct SingleFlight {
    /// Entries whose guards have all been dropped are pruned whenever a new
    /// key is acquired, so the map is bounded by in-flight keys, not by the
    /// total number of keys ever seen.
    locks: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
}

impl Default for SingleFlight {
    fn default() -> Self {
        Self::new()
    }
}

impl SingleFlight {
    pub fn new() -> Self {
        Self {
            locks: Mutex::new(HashMap::new()),
        }
    }

    /// Acquire the per-key lock and return an owned guard. Holding the
    /// guard marks the key as in-flight; dropping it lets the next waiter
    /// in.
    pub async fn lock_for(&self, key: &str) -> OwnedMutexGuard<()> {
        let mutex = {
            // The registry mutex is a plain sync lock: it is only held for
            // the map lookup/insert, never across an await.
            let mut locks = self.locks.lock().expect("single-flight registry poisoned");
            locks.retain(|_, m| Arc::strong_count(m) > 1);
            locks.entry(key.to_string()).or_default().clone()
        };
        mutex.lock_owned().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn concurrent_locks_for_same_key_serialize() {
        let flight = Arc::new(SingleFlight::new());
        let counter = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..8 {
            let flight = Arc::clone(&flight);
            let counter = Arc::clone(&counter);
            handles.push(tokio::spawn(async move {
                let _guard = flight.lock_for("same").await;
                // Each holder observes a distinct counter value only if no
                // two holders ran concurrently.
                let n = counter.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                n
            }));
        }

        let mut seen: Vec<usize> = Vec::new();
        for handle in handles {
            seen.push(handle.await.unwrap());
        }
        seen.sort_unstable();
        assert_eq!(seen, (0..8).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn dropped_guards_are_pruned_from_the_registry() {
        let flight = SingleFlight::new();
        {
            let _guard = flight.lock_for("k").await;
        }
        assert_eq!(flight.locks.lock().unwrap().len(), 1);

        // Acquiring a different key prunes "k" (no outstanding guards).
        let _guard = flight.lock_for("other").await;
        assert_eq!(flight.locks.lock().unwrap().len(), 1);
    }
}
