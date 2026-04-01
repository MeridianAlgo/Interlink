//! Event batch collector for the InterLink relayer pipeline.
//!
//! Instead of processing every gateway event individually (like Wormhole does
//! with per-VAA processing), InterLink collects events into time-bounded batches:
//!
//!   Flush condition A: `max_size` events accumulated
//!   Flush condition B: `flush_interval` elapsed since last flush
//!
//! Why batching beats individual processing:
//! - Amortises ZK proof setup cost across N transfers
//! - Reduces Solana transaction overhead (N submissions → 1 batch)
//! - Enables future O(log N) recursive proof folding (circuits/recursion.rs)
//! - Natural rate limiting prevents Solana mempool flooding
//!
//! Wormhole:  1–20 txs per VAA (fixed small batches, no time-based flushing)
//! InterLink: up to 100 txs per batch, flush every 5s — adapts to traffic

use crate::events::GatewayEvent;
use std::time::{Duration, Instant};
use tracing::info;

/// A completed batch of gateway events ready for proof generation.
#[derive(Debug)]
pub struct EventBatch {
    /// Events in this batch, in the order they were received.
    pub events: Vec<GatewayEvent>,
    /// How long this batch was open (time from first event to flush).
    pub open_duration: Duration,
    /// Monotonically increasing batch ID for tracing.
    pub batch_id: u64,
}

impl EventBatch {
    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Minimum block number across all events in the batch (for finality waiting).
    pub fn min_block(&self) -> u64 {
        self.events
            .iter()
            .map(|e| e.block_number())
            .min()
            .unwrap_or(0)
    }

    /// Maximum block number across all events in the batch.
    pub fn max_block(&self) -> u64 {
        self.events
            .iter()
            .map(|e| e.block_number())
            .max()
            .unwrap_or(0)
    }
}

/// Accumulates gateway events and flushes them as time-bounded batches.
pub struct BatchCollector {
    pending: Vec<GatewayEvent>,
    opened_at: Instant,
    max_size: usize,
    flush_interval: Duration,
    next_batch_id: u64,
}

impl BatchCollector {
    /// Create a new collector.
    ///
    /// - `max_size`: flush immediately when this many events accumulate
    /// - `flush_interval`: flush on timer even if `max_size` is not reached
    pub fn new(max_size: usize, flush_interval: Duration) -> Self {
        Self {
            pending: Vec::with_capacity(max_size),
            opened_at: Instant::now(),
            max_size,
            flush_interval,
            next_batch_id: 0,
        }
    }

    /// Push an event. Returns a flushed batch if the size limit is hit.
    pub fn push(&mut self, event: GatewayEvent) -> Option<EventBatch> {
        self.pending.push(event);
        if self.pending.len() >= self.max_size {
            Some(self.flush("size_limit"))
        } else {
            None
        }
    }

    /// True if the flush interval has elapsed and there are pending events.
    /// Call this on each timer tick and flush if true.
    pub fn is_timer_ready(&self) -> bool {
        !self.pending.is_empty() && self.opened_at.elapsed() >= self.flush_interval
    }

    /// Flush pending events (timer-driven). Returns `None` if nothing is pending.
    pub fn flush_timer(&mut self) -> Option<EventBatch> {
        if self.pending.is_empty() {
            return None;
        }
        Some(self.flush("timer"))
    }

    /// Number of events currently buffered.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    fn flush(&mut self, reason: &str) -> EventBatch {
        let events = std::mem::replace(&mut self.pending, Vec::with_capacity(self.max_size));
        let open_duration = self.opened_at.elapsed();
        self.opened_at = Instant::now();
        let batch_id = self.next_batch_id;
        self.next_batch_id += 1;

        info!(
            batch_id,
            batch_size = events.len(),
            open_ms = open_duration.as_millis(),
            reason,
            "event batch flushed"
        );

        EventBatch {
            events,
            open_duration,
            batch_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{DepositEvent, GatewayEvent};

    fn make_deposit(seq: u64, block: u64) -> GatewayEvent {
        GatewayEvent::Deposit(DepositEvent {
            block_number: block,
            tx_hash: [0u8; 32],
            sequence: seq,
            sender: [0u8; 20],
            recipient: vec![0u8; 20],
            amount: 1000,
            destination_chain: 2,
            payload_hash: [0xAB; 32],
        })
    }

    #[test]
    fn test_flush_on_size() {
        let mut collector = BatchCollector::new(3, Duration::from_secs(60));

        assert!(collector.push(make_deposit(1, 100)).is_none());
        assert!(collector.push(make_deposit(2, 101)).is_none());
        // Third event triggers flush
        let batch = collector.push(make_deposit(3, 102)).expect("should flush");
        assert_eq!(batch.len(), 3);
        assert_eq!(batch.batch_id, 0);
        assert_eq!(collector.pending_count(), 0);
    }

    #[test]
    fn test_flush_on_timer() {
        let mut collector = BatchCollector::new(100, Duration::from_millis(1));

        collector.push(make_deposit(1, 100));
        collector.push(make_deposit(2, 101));

        // Wait for flush interval
        std::thread::sleep(Duration::from_millis(5));

        assert!(collector.is_timer_ready());
        let batch = collector.flush_timer().expect("should flush");
        assert_eq!(batch.len(), 2);
        assert!(!collector.is_timer_ready());
    }

    #[test]
    fn test_min_max_block() {
        let mut collector = BatchCollector::new(10, Duration::from_secs(60));
        collector.push(make_deposit(1, 50));
        collector.push(make_deposit(2, 100));
        collector.push(make_deposit(3, 75));
        let batch = collector.flush_timer().unwrap();
        assert_eq!(batch.min_block(), 50);
        assert_eq!(batch.max_block(), 100);
    }

    #[test]
    fn test_batch_ids_increment() {
        let mut collector = BatchCollector::new(1, Duration::from_secs(60));
        let b0 = collector.push(make_deposit(1, 100)).unwrap();
        let b1 = collector.push(make_deposit(2, 101)).unwrap();
        assert_eq!(b0.batch_id, 0);
        assert_eq!(b1.batch_id, 1);
    }

    #[test]
    fn test_no_flush_when_empty() {
        let mut collector = BatchCollector::new(10, Duration::from_millis(1));
        std::thread::sleep(Duration::from_millis(5));
        assert!(!collector.is_timer_ready());
        assert!(collector.flush_timer().is_none());
    }

    // ── Phase 1: Batch size scaling tests (100, 500, 1000 tx batches) ──────

    #[test]
    fn test_batch_100_events() {
        let mut collector = BatchCollector::new(100, Duration::from_secs(60));
        for i in 0..99 {
            assert!(collector.push(make_deposit(i, 1000 + i)).is_none());
        }
        let batch = collector.push(make_deposit(99, 1099)).expect("100th should flush");
        assert_eq!(batch.len(), 100);
        assert_eq!(batch.min_block(), 1000);
        assert_eq!(batch.max_block(), 1099);
    }

    #[test]
    fn test_batch_500_events() {
        let mut collector = BatchCollector::new(500, Duration::from_secs(60));
        for i in 0..499 {
            assert!(collector.push(make_deposit(i, 2000 + i)).is_none());
        }
        let batch = collector.push(make_deposit(499, 2499)).expect("500th should flush");
        assert_eq!(batch.len(), 500);
        assert_eq!(batch.min_block(), 2000);
        assert_eq!(batch.max_block(), 2499);
    }

    #[test]
    fn test_batch_1000_events() {
        let mut collector = BatchCollector::new(1000, Duration::from_secs(60));
        for i in 0..999 {
            assert!(collector.push(make_deposit(i, 3000 + i)).is_none());
        }
        let batch = collector.push(make_deposit(999, 3999)).expect("1000th should flush");
        assert_eq!(batch.len(), 1000);
        assert_eq!(batch.min_block(), 3000);
        assert_eq!(batch.max_block(), 3999);
    }

    // ── Phase 1: Batch overhead comparison (batch vs per-tx) ───────────────

    #[test]
    fn test_batch_overhead_vs_per_tx() {
        // Simulate per-tx settlement: 100 individual flushes (batch_size=1)
        let mut per_tx = BatchCollector::new(1, Duration::from_secs(60));
        let mut per_tx_batches = 0u64;
        for i in 0..100 {
            if per_tx.push(make_deposit(i, 5000 + i)).is_some() {
                per_tx_batches += 1;
            }
        }
        assert_eq!(per_tx_batches, 100, "per-tx: each event = 1 flush");

        // Simulate batch settlement: 1 flush for 100 events
        let mut batched = BatchCollector::new(100, Duration::from_secs(60));
        let mut batch_flushes = 0u64;
        for i in 0..100 {
            if batched.push(make_deposit(i, 5000 + i)).is_some() {
                batch_flushes += 1;
            }
        }
        assert_eq!(batch_flushes, 1, "batched: 100 events = 1 flush");

        // Overhead comparison: batching reduces flushes by 100x
        // Each flush = 1 Solana tx + 1 proof setup → batching amortises both
        assert!(
            per_tx_batches >= batch_flushes * 50,
            "batch mode should reduce flush overhead by at least 50x"
        );
    }

    #[test]
    fn test_multiple_batch_sizes_sequential() {
        // Verify collector handles sequential batches of varying sizes
        let sizes = [100, 500, 1000];
        for &size in &sizes {
            let mut collector = BatchCollector::new(size, Duration::from_secs(60));
            for i in 0..(size as u64) {
                let _ = collector.push(make_deposit(i, i));
            }
            // All events should have flushed
            assert_eq!(collector.pending_count(), 0);
        }
    }
}
