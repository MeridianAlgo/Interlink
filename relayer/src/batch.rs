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
}
