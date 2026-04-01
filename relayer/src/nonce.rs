//! Solana durable nonce pool for parallel transaction submission (Phase 2).
//!
//! # Problem
//!
//! Standard Solana transactions use a recent blockhash as a replay-protection
//! nonce — valid for ~2 minutes / ~300 slots.  When submitting many proofs in
//! parallel, a shared blockhash can expire before slower submissions confirm,
//! causing failures under load.
//!
//! # Solution: durable nonces
//!
//! Solana durable nonces are single-use on-chain accounts.  Each nonce account
//! stores a hash that acts as a permanent, non-expiring transaction nonce.
//! After use, the account issues an `AdvanceNonceAccount` instruction to rotate
//! to the next nonce value, making it reusable.
//!
//! # Phase 2 targets (from checklist)
//!
//! - Current:  1 tx at a time to Solana
//! - Target:   10–100 parallel nonces → simultaneous settlement
//! - Pool size 32 covers typical batch sizes (BATCH_MAX_SIZE = 100 but
//!   parallelism is bounded by CPU cores ≈ 4–16 in practice)
//!
//! # Usage
//!
//! ```rust,ignore
//! let pool = DurableNoncePool::new(32);
//! pool.populate_dev_nonces(32).await;   // in prod: load real accounts
//!
//! let permit = pool.acquire().await.expect("pool not empty");
//! submitter.submit_with_nonce(&package, permit.pubkey()).await?;
//! // permit released automatically on drop → nonce returned to pool
//! ```

use std::sync::Arc;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tracing::{info, warn};

// ─── DurableNonce ─────────────────────────────────────────────────────────────

/// A Solana durable nonce account, ready to be used as a transaction nonce.
#[derive(Debug, Clone)]
pub struct DurableNonce {
    /// Base-58 public key of the nonce account.
    pub pubkey: String,
    /// Path to the authority keypair that can sign `AdvanceNonceAccount`.
    pub authority_keypair_path: String,
    /// Current nonce value (hex-encoded 32 bytes).  Must be refreshed from
    /// on-chain after each use via `getNonce` RPC call.
    pub nonce_value: String,
}

impl DurableNonce {
    pub fn new(
        pubkey: impl Into<String>,
        authority_keypair_path: impl Into<String>,
        nonce_value: impl Into<String>,
    ) -> Self {
        Self {
            pubkey: pubkey.into(),
            authority_keypair_path: authority_keypair_path.into(),
            nonce_value: nonce_value.into(),
        }
    }
}

// ─── NoncePermit ──────────────────────────────────────────────────────────────

/// Exclusive-use permit for a single nonce account.
///
/// Dropping this permit automatically returns the nonce to the pool so another
/// submission task can use it.
pub struct NoncePermit {
    pub nonce: DurableNonce,
    _semaphore_permit: OwnedSemaphorePermit,
    pool_nonces: Arc<Mutex<Vec<DurableNonce>>>,
}

impl NoncePermit {
    /// Base-58 public key of the nonce account held by this permit.
    pub fn pubkey(&self) -> &str {
        &self.nonce.pubkey
    }

    /// Current nonce value for this account (hex-encoded).
    pub fn nonce_value(&self) -> &str {
        &self.nonce.nonce_value
    }

    /// Authority keypair path for signing `AdvanceNonceAccount`.
    pub fn authority_path(&self) -> &str {
        &self.nonce.authority_keypair_path
    }
}

impl Drop for NoncePermit {
    /// Return the nonce account to the pool.  Uses a detached tokio task
    /// because Drop cannot be async.
    fn drop(&mut self) {
        let nonce = self.nonce.clone();
        let pool_nonces = Arc::clone(&self.pool_nonces);
        tokio::spawn(async move {
            pool_nonces.lock().await.push(nonce);
        });
    }
}

// ─── DurableNoncePool ─────────────────────────────────────────────────────────

/// A pool of Solana durable nonce accounts for parallel transaction submission.
///
/// # Design
///
/// The pool is backed by a `Semaphore` (one permit per nonce account) and a
/// `Mutex<Vec<DurableNonce>>` (the actual nonce list).  Acquiring a permit
/// atomically removes one nonce from the vec; dropping the permit spawns a task
/// that returns it.  The semaphore ensures callers block instead of spinning
/// when the pool is exhausted.
///
/// # Exhaustion alert
///
/// Call [`check_exhaustion_alert`] from a health-check loop to log a warning
/// when fewer than 10% of nonces remain available (Phase 10 alerting).
#[derive(Clone)]
pub struct DurableNoncePool {
    nonces: Arc<Mutex<Vec<DurableNonce>>>,
    semaphore: Arc<Semaphore>,
    /// Logical capacity (set at construction, reflects all ever-added nonces).
    capacity: usize,
}

impl DurableNoncePool {
    /// Create an empty pool with the given logical capacity.
    ///
    /// Nonces must be added via [`add_nonce`] or [`populate_dev_nonces`].
    pub fn new(capacity: usize) -> Self {
        info!(capacity, "creating Solana durable nonce pool");
        Self {
            nonces: Arc::new(Mutex::new(Vec::with_capacity(capacity))),
            // Start at 0 permits; each add_nonce call increments by 1.
            semaphore: Arc::new(Semaphore::new(0)),
            capacity,
        }
    }

    /// Add a pre-created on-chain nonce account to the pool.
    pub async fn add_nonce(&self, nonce: DurableNonce) {
        self.nonces.lock().await.push(nonce);
        self.semaphore.add_permits(1);
    }

    /// Populate the pool with synthetic nonces for dev / CI environments.
    ///
    /// **Do not use in production** — these pubkeys are not real on-chain accounts.
    pub async fn populate_dev_nonces(&self, count: usize) {
        for i in 0..count {
            let nonce = DurableNonce::new(
                format!("DevNonce{i:06}"),
                "~/.config/solana/id.json",
                format!("{i:064x}"),
            );
            self.add_nonce(nonce).await;
        }
        info!(count, "populated pool with dev nonces (not for production)");
    }

    /// Acquire a nonce permit.
    ///
    /// Waits asynchronously if all nonces are checked out.
    /// Returns `None` only if the semaphore is closed (pool dropped).
    pub async fn acquire(&self) -> Option<NoncePermit> {
        let permit = self.semaphore.clone().acquire_owned().await.ok()?;
        let nonce = self.nonces.lock().await.pop()?;
        Some(NoncePermit {
            nonce,
            _semaphore_permit: permit,
            pool_nonces: Arc::clone(&self.nonces),
        })
    }

    /// Number of nonces currently available (not checked out).
    pub async fn available(&self) -> usize {
        self.nonces.lock().await.len()
    }

    /// Total capacity as set at construction.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns `true` and emits a WARN log if fewer than 10% of nonces remain.
    ///
    /// Call from a monitoring loop; corresponds to the Phase 10 alerting rule
    /// "validator downtime > 15 min" (here: nonce exhaustion = submission stall).
    pub async fn check_exhaustion_alert(&self) -> bool {
        let avail = self.available().await;
        let threshold = (self.capacity / 10).max(1);
        if avail < threshold {
            warn!(
                available = avail,
                capacity = self.capacity,
                threshold,
                "ALERT: nonce pool nearly exhausted — parallel submissions will stall"
            );
            true
        } else {
            false
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_acquire_returns_nonce_and_reduces_available() {
        let pool = DurableNoncePool::new(4);
        pool.populate_dev_nonces(4).await;

        assert_eq!(pool.available().await, 4);
        let permit = pool.acquire().await.expect("should get permit");
        assert_eq!(pool.available().await, 3);
        assert!(!permit.pubkey().is_empty());
    }

    #[tokio::test]
    async fn test_drop_returns_nonce_to_pool() {
        let pool = DurableNoncePool::new(2);
        pool.populate_dev_nonces(2).await;

        let permit = pool.acquire().await.unwrap();
        assert_eq!(pool.available().await, 1);
        drop(permit);

        // The spawn in Drop is async; give it a tick to execute.
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
        assert_eq!(pool.available().await, 2);
    }

    #[tokio::test]
    async fn test_empty_pool_blocks_until_timeout() {
        let pool = DurableNoncePool::new(2);
        // No nonces added → semaphore has 0 permits.
        let result =
            tokio::time::timeout(tokio::time::Duration::from_millis(20), pool.acquire()).await;
        assert!(result.is_err(), "empty pool must block, not return None");
    }

    #[tokio::test]
    async fn test_exhaustion_alert_below_threshold() {
        let pool = DurableNoncePool::new(10);
        // 1 nonce → threshold = max(10/10, 1) = 1, avail=1 not < 1 → no alert
        pool.add_nonce(DurableNonce::new("K1", "/dev/null", "0".repeat(64)))
            .await;
        assert!(!pool.check_exhaustion_alert().await);

        // Acquire the only nonce → avail=0 < threshold=1 → alert
        let _p = pool.acquire().await.unwrap();
        assert!(pool.check_exhaustion_alert().await);
    }

    #[tokio::test]
    async fn test_concurrent_acquire_all_nonces() {
        let pool = Arc::new(DurableNoncePool::new(5));
        pool.populate_dev_nonces(5).await;

        let mut handles = vec![];
        for _ in 0..5 {
            let p = Arc::clone(&pool);
            handles.push(tokio::spawn(async move {
                let permit = p.acquire().await.expect("must get permit");
                tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
                drop(permit);
            }));
        }
        for h in handles {
            h.await.expect("task panicked");
        }
        // All permits released
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        assert_eq!(pool.available().await, 5);
    }

    #[test]
    fn test_capacity_reflects_constructor_arg() {
        let pool = DurableNoncePool::new(32);
        assert_eq!(pool.capacity(), 32);
    }

    // ── Phase 2: Finality consistency across nonces ────────────────────────

    #[tokio::test]
    async fn test_all_nonces_unique_pubkeys() {
        let pool = DurableNoncePool::new(10);
        pool.populate_dev_nonces(10).await;

        let mut pubkeys = Vec::new();
        let mut permits = Vec::new();
        for _ in 0..10 {
            let permit = pool.acquire().await.unwrap();
            pubkeys.push(permit.pubkey().to_string());
            permits.push(permit);
        }

        // All nonce pubkeys must be unique (no double-use)
        pubkeys.sort();
        pubkeys.dedup();
        assert_eq!(pubkeys.len(), 10, "all nonces must have unique pubkeys");
    }

    #[tokio::test]
    async fn test_concurrent_nonce_consistency() {
        // Simulate parallel submissions using all nonces, ensure no overlap
        let pool = Arc::new(DurableNoncePool::new(16));
        pool.populate_dev_nonces(16).await;

        let used_pubkeys = Arc::new(Mutex::new(Vec::new()));
        let mut handles = vec![];

        for _ in 0..16 {
            let p = Arc::clone(&pool);
            let keys = Arc::clone(&used_pubkeys);
            handles.push(tokio::spawn(async move {
                let permit = p.acquire().await.expect("must get permit");
                let pk = permit.pubkey().to_string();
                keys.lock().await.push(pk.clone());
                // Simulate proof submission latency
                tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                drop(permit);
                pk
            }));
        }

        let mut results = Vec::new();
        for h in handles {
            results.push(h.await.unwrap());
        }

        // Every concurrent task got a unique nonce
        results.sort();
        results.dedup();
        assert_eq!(results.len(), 16, "all concurrent nonces must be unique");

        // All nonces returned to pool
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        assert_eq!(pool.available().await, 16);
    }

    #[tokio::test]
    async fn test_nonce_reuse_after_return() {
        // After returning a nonce, the same pubkey can be re-acquired
        let pool = DurableNoncePool::new(1);
        pool.populate_dev_nonces(1).await;

        let permit1 = pool.acquire().await.unwrap();
        let pk1 = permit1.pubkey().to_string();
        drop(permit1);
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;

        let permit2 = pool.acquire().await.unwrap();
        let pk2 = permit2.pubkey().to_string();
        assert_eq!(pk1, pk2, "nonce should be reusable after return");
    }
}
