//! In-process metrics for the InterLink relayer (Phase 10).
//!
//! Tracks the key metrics from the competitive analysis:
//! - `proof_gen_ms`: histogram (target p99 <100ms vs Wormhole 300-500ms)
//! - `settlement_ms`: histogram (target p99 <30 000ms vs Wormhole 120 000ms+)
//! - `batch_size`: events per flush (target 100-1000 vs Wormhole 1-20)
//! - `queue_depth`: events buffered but not yet dispatched
//!
//! Exposes two scrape formats:
//! - Prometheus text (GET /metrics) — consumed by Grafana/Prometheus
//! - JSON snapshot (GET /metrics/json) — consumed by dashboards / API clients
//!
//! Alert thresholds (matching Phase 10 checklist):
//! - proof_gen >1 000ms   → `proof_gen_alerts` counter incremented + WARN log
//! - settlement >60 000ms → `settlement_alerts` counter incremented + WARN log
//! - queue_depth >1 000   → logged by the caller (not tracked here)

use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

const RLX: Ordering = Ordering::Relaxed;

// ─── Inner state ─────────────────────────────────────────────────────────────

#[derive(Debug)]
struct Inner {
    // ── Proof generation ────────────────────────────────────────────────────
    proof_gen_total: AtomicU64, // all attempts (started)
    proof_gen_success: AtomicU64,
    proof_gen_failure: AtomicU64,
    proof_gen_ms_sum: AtomicU64, // sum → mean = sum / success
    proof_gen_ms_max: AtomicU64,
    /// Count of proofs exceeding 1 000ms alert threshold.
    proof_gen_alerts: AtomicU64,

    // ── Settlement (end-to-end: finality + proof + submission) ──────────────
    settlement_total: AtomicU64,
    settlement_success: AtomicU64,
    settlement_failure: AtomicU64,
    settlement_ms_sum: AtomicU64,
    settlement_ms_max: AtomicU64,
    /// Settlements exceeding 60 000ms SLA threshold.
    settlement_alerts: AtomicU64,

    // ── Batch pipeline ───────────────────────────────────────────────────────
    batches_flushed: AtomicU64,  // total flushes (size+timer)
    events_processed: AtomicU64, // total events across all batches
    batch_size_sum: AtomicU64,   // sum → mean = sum / batches_flushed
    batch_size_max: AtomicU64,

    // ── Queue ────────────────────────────────────────────────────────────────
    queue_depth: AtomicU64, // current snapshot (set, not incremented)

    // ── Verification time ────────────────────────────────────────────────────
    verify_ms_sum: AtomicU64,
    verify_ms_max: AtomicU64,
    verify_total: AtomicU64,
    /// Verifications exceeding 500ms alert threshold.
    verify_alerts: AtomicU64,

    // ── Chain health (per-chain finality lag + RPC latency) ──────────────────
    // Stored in Mutex<HashMap> because chain IDs are dynamic.
    /// chain_id → cumulative finality lag ms sum
    chain_finality_ms_sum: Mutex<HashMap<u32, u64>>,
    /// chain_id → count of finality observations
    chain_finality_count: Mutex<HashMap<u32, u64>>,
    /// chain_id → latest RPC latency ms
    chain_rpc_latency_ms: Mutex<HashMap<u32, u64>>,

    // ── User metrics ────────────────────────────────────────────────────────
    daily_transfers: AtomicU64,
    unique_users: AtomicU64,
    /// corridor key ("src_chain:dst_chain") → transfer count
    corridor_counts: Mutex<HashMap<String, u64>>,

    // ── TVL & Volume (Phase 10 competitive tracking) ────────────────────────
    /// Total value locked in bridge vaults (USD cents)
    tvl_usd_cents: AtomicU64,
    /// Daily transfer volume (USD cents, reset at UTC midnight)
    daily_volume_usd_cents: AtomicU64,
    /// All-time cumulative volume (USD cents)
    cumulative_volume_usd_cents: AtomicU64,
    /// Validator uptime observations: total heartbeats received
    validator_heartbeats_total: AtomicU64,
    /// Validator uptime observations: total expected heartbeats
    validator_heartbeats_expected: AtomicU64,
}

impl Default for Inner {
    fn default() -> Self {
        Inner {
            proof_gen_total: AtomicU64::new(0),
            proof_gen_success: AtomicU64::new(0),
            proof_gen_failure: AtomicU64::new(0),
            proof_gen_ms_sum: AtomicU64::new(0),
            proof_gen_ms_max: AtomicU64::new(0),
            proof_gen_alerts: AtomicU64::new(0),
            settlement_total: AtomicU64::new(0),
            settlement_success: AtomicU64::new(0),
            settlement_failure: AtomicU64::new(0),
            settlement_ms_sum: AtomicU64::new(0),
            settlement_ms_max: AtomicU64::new(0),
            settlement_alerts: AtomicU64::new(0),
            batches_flushed: AtomicU64::new(0),
            events_processed: AtomicU64::new(0),
            batch_size_sum: AtomicU64::new(0),
            batch_size_max: AtomicU64::new(0),
            queue_depth: AtomicU64::new(0),
            verify_ms_sum: AtomicU64::new(0),
            verify_ms_max: AtomicU64::new(0),
            verify_total: AtomicU64::new(0),
            verify_alerts: AtomicU64::new(0),
            chain_finality_ms_sum: Mutex::new(HashMap::new()),
            chain_finality_count: Mutex::new(HashMap::new()),
            chain_rpc_latency_ms: Mutex::new(HashMap::new()),
            daily_transfers: AtomicU64::new(0),
            unique_users: AtomicU64::new(0),
            corridor_counts: Mutex::new(HashMap::new()),
            tvl_usd_cents: AtomicU64::new(0),
            daily_volume_usd_cents: AtomicU64::new(0),
            cumulative_volume_usd_cents: AtomicU64::new(0),
            validator_heartbeats_total: AtomicU64::new(0),
            validator_heartbeats_expected: AtomicU64::new(0),
        }
    }
}

// ─── Public handle ────────────────────────────────────────────────────────────

/// Shared, cheaply-cloneable metrics handle.
#[derive(Clone, Debug)]
pub struct Metrics(Arc<Inner>);

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    pub fn new() -> Self {
        Self(Arc::new(Inner::default()))
    }

    // ── Proof generation ────────────────────────────────────────────────────

    /// Call when a proof generation task begins.
    pub fn record_proof_start(&self) {
        self.0.proof_gen_total.fetch_add(1, RLX);
    }

    /// Call on successful proof, passing elapsed milliseconds.
    pub fn record_proof_success(&self, ms: u64) {
        self.0.proof_gen_success.fetch_add(1, RLX);
        self.0.proof_gen_ms_sum.fetch_add(ms, RLX);
        atomic_max(&self.0.proof_gen_ms_max, ms);
        if ms > 1_000 {
            self.0.proof_gen_alerts.fetch_add(1, RLX);
        }
    }

    /// Call when proof generation fails.
    pub fn record_proof_failure(&self) {
        self.0.proof_gen_failure.fetch_add(1, RLX);
    }

    // ── Settlement ──────────────────────────────────────────────────────────

    /// Call when a settlement pipeline begins (includes finality wait).
    pub fn record_settlement_start(&self) {
        self.0.settlement_total.fetch_add(1, RLX);
    }

    /// Call on successful end-to-end settlement.
    pub fn record_settlement_success(&self, ms: u64) {
        self.0.settlement_success.fetch_add(1, RLX);
        self.0.settlement_ms_sum.fetch_add(ms, RLX);
        atomic_max(&self.0.settlement_ms_max, ms);
        if ms > 60_000 {
            self.0.settlement_alerts.fetch_add(1, RLX);
        }
    }

    /// Call when settlement fails (finality timeout, proof error, or submit error).
    pub fn record_settlement_failure(&self) {
        self.0.settlement_failure.fetch_add(1, RLX);
    }

    // ── Batch ───────────────────────────────────────────────────────────────

    /// Call each time a batch is dispatched to the proof pipeline.
    pub fn record_batch_flushed(&self, size: usize) {
        let n = size as u64;
        self.0.batches_flushed.fetch_add(1, RLX);
        self.0.events_processed.fetch_add(n, RLX);
        self.0.batch_size_sum.fetch_add(n, RLX);
        atomic_max(&self.0.batch_size_max, n);
    }

    /// Set current queue depth (point-in-time snapshot from mpsc channel).
    pub fn set_queue_depth(&self, depth: usize) {
        self.0.queue_depth.store(depth as u64, RLX);
    }

    // ── Verification time ────────────────────────────────────────────────────

    /// Record on-chain verification latency in ms. Alert threshold: >500ms.
    pub fn record_verification(&self, ms: u64) {
        self.0.verify_total.fetch_add(1, RLX);
        self.0.verify_ms_sum.fetch_add(ms, RLX);
        atomic_max(&self.0.verify_ms_max, ms);
        if ms > 500 {
            self.0.verify_alerts.fetch_add(1, RLX);
        }
    }

    // ── Chain health ─────────────────────────────────────────────────────────

    /// Record finality lag for a chain (milliseconds from block emission to confirmation).
    pub fn record_chain_finality(&self, chain_id: u32, lag_ms: u64) {
        let mut sums = self.0.chain_finality_ms_sum.lock().unwrap();
        let mut counts = self.0.chain_finality_count.lock().unwrap();
        *sums.entry(chain_id).or_insert(0) += lag_ms;
        *counts.entry(chain_id).or_insert(0) += 1;
    }

    /// Update the latest RPC latency for a chain.
    pub fn set_chain_rpc_latency(&self, chain_id: u32, latency_ms: u64) {
        self.0
            .chain_rpc_latency_ms
            .lock()
            .unwrap()
            .insert(chain_id, latency_ms);
    }

    /// Mean finality lag for a chain (ms), or 0 if no data.
    pub fn chain_finality_mean_ms(&self, chain_id: u32) -> u64 {
        let sums = self.0.chain_finality_ms_sum.lock().unwrap();
        let counts = self.0.chain_finality_count.lock().unwrap();
        let s = sums.get(&chain_id).copied().unwrap_or(0);
        let c = counts.get(&chain_id).copied().unwrap_or(0);
        mean(s, c)
    }

    // ── User metrics ─────────────────────────────────────────────────────────

    /// Increment daily transfer counter (reset externally at midnight UTC).
    pub fn record_transfer(&self, src_chain: u32, dst_chain: u32) {
        self.0.daily_transfers.fetch_add(1, RLX);
        let key = format!("{src_chain}:{dst_chain}");
        *self
            .0
            .corridor_counts
            .lock()
            .unwrap()
            .entry(key)
            .or_insert(0) += 1;
    }

    /// Record a unique user (e.g., call once per distinct sender address per session).
    pub fn record_unique_user(&self) {
        self.0.unique_users.fetch_add(1, RLX);
    }

    /// Reset daily counters (call at UTC midnight).
    pub fn reset_daily(&self) {
        self.0.daily_transfers.store(0, RLX);
        self.0.daily_volume_usd_cents.store(0, RLX);
        self.0.corridor_counts.lock().unwrap().clear();
    }

    // ── TVL & Volume (Phase 10) ─────────────────────────────────────────────

    /// Update the current total value locked (absolute value in USD cents).
    pub fn set_tvl_usd_cents(&self, cents: u64) {
        self.0.tvl_usd_cents.store(cents, RLX);
    }

    /// Record a transfer volume (adds to daily + cumulative).
    pub fn record_volume_usd_cents(&self, cents: u64) {
        self.0.daily_volume_usd_cents.fetch_add(cents, RLX);
        self.0.cumulative_volume_usd_cents.fetch_add(cents, RLX);
    }

    /// Current TVL in USD cents.
    pub fn tvl_usd_cents(&self) -> u64 {
        self.0.tvl_usd_cents.load(RLX)
    }

    /// Daily volume in USD cents.
    pub fn daily_volume_usd_cents(&self) -> u64 {
        self.0.daily_volume_usd_cents.load(RLX)
    }

    /// Cumulative all-time volume in USD cents.
    pub fn cumulative_volume_usd_cents(&self) -> u64 {
        self.0.cumulative_volume_usd_cents.load(RLX)
    }

    // ── Validator uptime tracking (Phase 10) ────────────────────────────────

    /// Record validator heartbeat observations.
    pub fn record_validator_heartbeats(&self, received: u64, expected: u64) {
        self.0.validator_heartbeats_total.fetch_add(received, RLX);
        self.0.validator_heartbeats_expected.fetch_add(expected, RLX);
    }

    /// Validator uptime percentage (0.0 - 100.0).
    pub fn validator_uptime_pct(&self) -> f64 {
        let total = self.0.validator_heartbeats_total.load(RLX);
        let expected = self.0.validator_heartbeats_expected.load(RLX);
        if expected == 0 { 100.0 } else { total as f64 / expected as f64 * 100.0 }
    }

    /// Top corridors by transfer count, sorted descending. Returns up to `n` entries.
    pub fn top_corridors(&self, n: usize) -> Vec<(String, u64)> {
        let counts = self.0.corridor_counts.lock().unwrap();
        let mut pairs: Vec<(String, u64)> = counts.iter().map(|(k, v)| (k.clone(), *v)).collect();
        pairs.sort_by(|a, b| b.1.cmp(&a.1));
        pairs.truncate(n);
        pairs
    }

    // ── Export: Prometheus text ─────────────────────────────────────────────

    /// Returns a Prometheus text-format string for scraping.
    ///
    /// Compatible with Prometheus 0.0.4 scrape format.
    /// Mount at GET /metrics and point Prometheus / Grafana at it.
    pub fn prometheus_text(&self) -> String {
        let i = &self.0;
        let mut out = String::with_capacity(4096);

        let proof_ok = i.proof_gen_success.load(RLX);
        let proof_mean = mean(i.proof_gen_ms_sum.load(RLX), proof_ok);

        let settle_ok = i.settlement_success.load(RLX);
        let settle_mean = mean(i.settlement_ms_sum.load(RLX), settle_ok);

        let batches = i.batches_flushed.load(RLX);
        let batch_mean = mean(i.batch_size_sum.load(RLX), batches);

        // ── proof_gen ───────────────────────────────────────────────────────
        write_counter(
            &mut out,
            "interlink_proof_gen_total",
            "Total proof generation attempts",
            i.proof_gen_total.load(RLX),
        );
        write_counter(
            &mut out,
            "interlink_proof_gen_success_total",
            "Successful proof generations",
            proof_ok,
        );
        write_counter(
            &mut out,
            "interlink_proof_gen_failure_total",
            "Failed proof generations",
            i.proof_gen_failure.load(RLX),
        );
        write_gauge(
            &mut out,
            "interlink_proof_gen_ms_mean",
            "Mean proof gen time ms — target <100 vs Wormhole 300+",
            proof_mean,
        );
        write_gauge(
            &mut out,
            "interlink_proof_gen_ms_max",
            "Max proof gen time ms seen",
            i.proof_gen_ms_max.load(RLX),
        );
        write_counter(
            &mut out,
            "interlink_proof_gen_alert_total",
            "Proofs exceeding 1000ms alert threshold",
            i.proof_gen_alerts.load(RLX),
        );

        // ── settlement ──────────────────────────────────────────────────────
        write_counter(
            &mut out,
            "interlink_settlement_total",
            "Total settlement pipeline attempts",
            i.settlement_total.load(RLX),
        );
        write_counter(
            &mut out,
            "interlink_settlement_success_total",
            "Successful end-to-end settlements",
            settle_ok,
        );
        write_counter(
            &mut out,
            "interlink_settlement_failure_total",
            "Failed settlements",
            i.settlement_failure.load(RLX),
        );
        write_gauge(
            &mut out,
            "interlink_settlement_ms_mean",
            "Mean settlement ms — target <30000 vs Wormhole 120000+",
            settle_mean,
        );
        write_gauge(
            &mut out,
            "interlink_settlement_ms_max",
            "Max settlement ms seen",
            i.settlement_ms_max.load(RLX),
        );
        write_counter(
            &mut out,
            "interlink_settlement_sla_breach_total",
            "Settlements exceeding 60s SLA",
            i.settlement_alerts.load(RLX),
        );

        // ── batch ───────────────────────────────────────────────────────────
        write_counter(
            &mut out,
            "interlink_batches_flushed_total",
            "Total batches dispatched to proof pipeline",
            batches,
        );
        write_counter(
            &mut out,
            "interlink_events_processed_total",
            "Total events processed across all batches",
            i.events_processed.load(RLX),
        );
        write_gauge(
            &mut out,
            "interlink_batch_size_mean",
            "Mean events per batch — target 100-1000 vs Wormhole 1-20",
            batch_mean,
        );
        write_gauge(
            &mut out,
            "interlink_batch_size_max",
            "Largest single batch seen",
            i.batch_size_max.load(RLX),
        );

        // ── queue ───────────────────────────────────────────────────────────
        write_gauge(
            &mut out,
            "interlink_queue_depth",
            "Events buffered in mpsc channel, not yet dispatched",
            i.queue_depth.load(RLX),
        );

        // ── verification ────────────────────────────────────────────────────
        let verify_total = i.verify_total.load(RLX);
        let verify_mean = mean(i.verify_ms_sum.load(RLX), verify_total);
        write_counter(
            &mut out,
            "interlink_verify_total",
            "On-chain verification calls",
            verify_total,
        );
        write_gauge(
            &mut out,
            "interlink_verify_ms_mean",
            "Mean on-chain verification latency ms",
            verify_mean,
        );
        write_gauge(
            &mut out,
            "interlink_verify_ms_max",
            "Max on-chain verification latency ms",
            i.verify_ms_max.load(RLX),
        );
        write_counter(
            &mut out,
            "interlink_verify_alert_total",
            "Verifications exceeding 500ms alert threshold",
            i.verify_alerts.load(RLX),
        );

        // ── chain health ─────────────────────────────────────────────────────
        {
            let sums = i.chain_finality_ms_sum.lock().unwrap();
            let counts = i.chain_finality_count.lock().unwrap();
            for (chain_id, &sum) in sums.iter() {
                let count = counts.get(chain_id).copied().unwrap_or(1);
                let m = mean(sum, count);
                write_gauge(
                    &mut out,
                    &format!("interlink_chain_{chain_id}_finality_ms_mean"),
                    &format!("Mean finality lag ms for chain {chain_id}"),
                    m,
                );
            }
            let rpc = i.chain_rpc_latency_ms.lock().unwrap();
            for (chain_id, &lat) in rpc.iter() {
                write_gauge(
                    &mut out,
                    &format!("interlink_chain_{chain_id}_rpc_latency_ms"),
                    &format!("Latest RPC latency ms for chain {chain_id}"),
                    lat,
                );
            }
        }

        // ── user metrics ─────────────────────────────────────────────────────
        write_gauge(
            &mut out,
            "interlink_daily_transfers",
            "Transfers in the current UTC day",
            i.daily_transfers.load(RLX),
        );
        write_gauge(
            &mut out,
            "interlink_unique_users_total",
            "Distinct sender addresses seen (lifetime)",
            i.unique_users.load(RLX),
        );

        // ── tvl & volume ────────────────────────────────────────────────────
        write_gauge(&mut out, "interlink_tvl_usd_cents",
            "Total value locked in bridge vaults (USD cents)", i.tvl_usd_cents.load(RLX));
        write_gauge(&mut out, "interlink_daily_volume_usd_cents",
            "Daily transfer volume (USD cents)", i.daily_volume_usd_cents.load(RLX));
        write_counter(&mut out, "interlink_cumulative_volume_usd_cents",
            "All-time cumulative transfer volume (USD cents)", i.cumulative_volume_usd_cents.load(RLX));

        // ── validator uptime ────────────────────────────────────────────────
        write_counter(&mut out, "interlink_validator_heartbeats_total",
            "Validator heartbeats received", i.validator_heartbeats_total.load(RLX));
        write_counter(&mut out, "interlink_validator_heartbeats_expected",
            "Validator heartbeats expected", i.validator_heartbeats_expected.load(RLX));

        out
    }

    // ── Export: JSON ────────────────────────────────────────────────────────

    /// Returns all metrics as a JSON object for API consumers.
    pub fn as_json(&self) -> serde_json::Value {
        let i = &self.0;
        let proof_ok = i.proof_gen_success.load(RLX);
        let settle_ok = i.settlement_success.load(RLX);
        let batches = i.batches_flushed.load(RLX);
        let verify_total = i.verify_total.load(RLX);

        serde_json::json!({
            "proof_gen": {
                "total":           i.proof_gen_total.load(RLX),
                "success":         proof_ok,
                "failure":         i.proof_gen_failure.load(RLX),
                "mean_ms":         mean(i.proof_gen_ms_sum.load(RLX), proof_ok),
                "max_ms":          i.proof_gen_ms_max.load(RLX),
                "alerts_over_1s":  i.proof_gen_alerts.load(RLX),
                "target_p99_ms":   100,
                "wormhole_ms":     300,
            },
            "settlement": {
                "total":            i.settlement_total.load(RLX),
                "success":          settle_ok,
                "failure":          i.settlement_failure.load(RLX),
                "mean_ms":          mean(i.settlement_ms_sum.load(RLX), settle_ok),
                "max_ms":           i.settlement_ms_max.load(RLX),
                "sla_breaches":     i.settlement_alerts.load(RLX),
                "target_ms":        30_000,
                "wormhole_min_ms":  120_000,
            },
            "batches": {
                "total_flushed":  batches,
                "events_total":   i.events_processed.load(RLX),
                "mean_size":      mean(i.batch_size_sum.load(RLX), batches),
                "max_size":       i.batch_size_max.load(RLX),
                "wormhole_max":   20,
            },
            "queue": {
                "depth": i.queue_depth.load(RLX),
            },
            "verification": {
                "total":          verify_total,
                "mean_ms":        mean(i.verify_ms_sum.load(RLX), verify_total),
                "max_ms":         i.verify_ms_max.load(RLX),
                "alerts_over_500ms": i.verify_alerts.load(RLX),
                "alert_threshold_ms": 500,
            },
            "user": {
                "daily_transfers": i.daily_transfers.load(RLX),
                "unique_users":    i.unique_users.load(RLX),
                "top_corridors":   self.top_corridors(5),
            },
            "tvl": {
                "usd_cents": i.tvl_usd_cents.load(RLX),
                "daily_volume_usd_cents": i.daily_volume_usd_cents.load(RLX),
                "cumulative_volume_usd_cents": i.cumulative_volume_usd_cents.load(RLX),
            },
            "validators": {
                "heartbeats_received": i.validator_heartbeats_total.load(RLX),
                "heartbeats_expected": i.validator_heartbeats_expected.load(RLX),
                "uptime_pct": self.validator_uptime_pct(),
            },
        })
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn atomic_max(cell: &AtomicU64, val: u64) {
    let mut cur = cell.load(RLX);
    while val > cur {
        match cell.compare_exchange_weak(cur, val, RLX, RLX) {
            Ok(_) => break,
            Err(actual) => cur = actual,
        }
    }
}

fn mean(sum: u64, count: u64) -> u64 {
    if count == 0 {
        0
    } else {
        sum / count
    }
}

fn write_counter(out: &mut String, name: &str, help: &str, val: u64) {
    out.push_str(&format!(
        "# HELP {name} {help}\n# TYPE {name} counter\n{name} {val}\n"
    ));
}

fn write_gauge(out: &mut String, name: &str, help: &str, val: u64) {
    out.push_str(&format!(
        "# HELP {name} {help}\n# TYPE {name} gauge\n{name} {val}\n"
    ));
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proof_metrics_accumulate() {
        let m = Metrics::new();
        m.record_proof_start();
        m.record_proof_start();
        m.record_proof_success(50);
        m.record_proof_success(150);
        m.record_proof_failure();

        let j = m.as_json();
        assert_eq!(j["proof_gen"]["total"], 2);
        assert_eq!(j["proof_gen"]["success"], 2);
        assert_eq!(j["proof_gen"]["failure"], 1);
        assert_eq!(j["proof_gen"]["max_ms"], 150);
        assert_eq!(j["proof_gen"]["mean_ms"], 100); // (50+150)/2
    }

    #[test]
    fn test_proof_alert_threshold() {
        let m = Metrics::new();
        m.record_proof_success(500); // under 1s → no alert
        m.record_proof_success(1_001); // over 1s → alert
        assert_eq!(m.as_json()["proof_gen"]["alerts_over_1s"], 1);
    }

    #[test]
    fn test_settlement_sla_breach() {
        let m = Metrics::new();
        m.record_settlement_success(29_000); // under 30s
        m.record_settlement_success(61_000); // over 60s → breach
        assert_eq!(m.as_json()["settlement"]["sla_breaches"], 1);
    }

    #[test]
    fn test_batch_statistics() {
        let m = Metrics::new();
        m.record_batch_flushed(10);
        m.record_batch_flushed(100);
        m.record_batch_flushed(50);

        let j = m.as_json();
        assert_eq!(j["batches"]["total_flushed"], 3);
        assert_eq!(j["batches"]["events_total"], 160);
        assert_eq!(j["batches"]["max_size"], 100);
        assert_eq!(j["batches"]["mean_size"], 53); // 160/3
    }

    #[test]
    fn test_queue_depth_set() {
        let m = Metrics::new();
        m.set_queue_depth(42);
        assert_eq!(m.as_json()["queue"]["depth"], 42);
        m.set_queue_depth(0);
        assert_eq!(m.as_json()["queue"]["depth"], 0);
    }

    #[test]
    fn test_prometheus_text_contains_required_metrics() {
        let m = Metrics::new();
        m.record_proof_start();
        m.record_proof_success(75);
        m.record_batch_flushed(20);
        m.set_queue_depth(5);

        let text = m.prometheus_text();
        // All required metrics present
        assert!(text.contains("interlink_proof_gen_total 1"));
        assert!(text.contains("interlink_proof_gen_success_total 1"));
        assert!(text.contains("interlink_batch_size_max 20"));
        assert!(text.contains("interlink_queue_depth 5"));
        // Prometheus type annotations present
        assert!(text.contains("# TYPE interlink_proof_gen_total counter"));
        assert!(text.contains("# TYPE interlink_queue_depth gauge"));
    }

    #[test]
    fn test_metrics_clone_shares_state() {
        let m1 = Metrics::new();
        let m2 = m1.clone();

        m1.record_proof_start();
        m1.record_proof_success(100);

        // m2 shares the same Arc, so must see the same state
        assert_eq!(m2.as_json()["proof_gen"]["success"], 1);
    }

    #[test]
    fn test_zero_division_guard() {
        // mean() must not panic when count=0
        let m = Metrics::new();
        let j = m.as_json();
        assert_eq!(j["proof_gen"]["mean_ms"], 0);
        assert_eq!(j["settlement"]["mean_ms"], 0);
        assert_eq!(j["batches"]["mean_size"], 0);
    }

    #[test]
    fn test_verification_alert_threshold() {
        let m = Metrics::new();
        m.record_verification(200); // fast, no alert
        m.record_verification(501); // slow, alert
        let j = m.as_json();
        assert_eq!(j["verification"]["total"], 2);
        assert_eq!(j["verification"]["alerts_over_500ms"], 1);
        assert_eq!(j["verification"]["max_ms"], 501);
    }

    #[test]
    fn test_chain_finality_mean() {
        let m = Metrics::new();
        m.record_chain_finality(1, 2000); // Ethereum
        m.record_chain_finality(1, 4000);
        m.record_chain_finality(10, 500); // Optimism
        assert_eq!(m.chain_finality_mean_ms(1), 3000);
        assert_eq!(m.chain_finality_mean_ms(10), 500);
        assert_eq!(m.chain_finality_mean_ms(999), 0); // unknown chain
    }

    #[test]
    fn test_chain_rpc_latency() {
        let m = Metrics::new();
        m.set_chain_rpc_latency(1, 45);
        m.set_chain_rpc_latency(1, 30); // update
        let prom = m.prometheus_text();
        assert!(prom.contains("interlink_chain_1_rpc_latency_ms 30"));
    }

    #[test]
    fn test_user_metrics_daily_transfers() {
        let m = Metrics::new();
        m.record_transfer(1, 900); // ETH → SOL
        m.record_transfer(1, 900);
        m.record_transfer(10, 900); // OP → SOL
        let j = m.as_json();
        assert_eq!(j["user"]["daily_transfers"], 3);
    }

    #[test]
    fn test_top_corridors() {
        let m = Metrics::new();
        for _ in 0..5 {
            m.record_transfer(1, 900);
        } // ETH→SOL ×5
        for _ in 0..3 {
            m.record_transfer(10, 900);
        } // OP→SOL ×3
        m.record_transfer(137, 900); // MATIC→SOL ×1

        let top = m.top_corridors(2);
        assert_eq!(top[0].0, "1:900");
        assert_eq!(top[0].1, 5);
        assert_eq!(top[1].0, "10:900");
        assert_eq!(top[1].1, 3);
        // Only 2 returned
        assert_eq!(top.len(), 2);
    }

    #[test]
    fn test_reset_daily() {
        let m = Metrics::new();
        m.record_transfer(1, 900);
        m.record_transfer(1, 900);
        m.reset_daily();
        assert_eq!(m.as_json()["user"]["daily_transfers"], 0);
        assert!(m.top_corridors(5).is_empty());
    }

    #[test]
    fn test_unique_users() {
        let m = Metrics::new();
        m.record_unique_user();
        m.record_unique_user();
        assert_eq!(m.as_json()["user"]["unique_users"], 2);
    }

    // ── TVL & Volume tests (Phase 10) ───────────────────────────────────────

    #[test]
    fn test_tvl_tracking() {
        let m = Metrics::new();
        m.set_tvl_usd_cents(100_000_000_00); // $100M
        assert_eq!(m.tvl_usd_cents(), 100_000_000_00);
        let j = m.as_json();
        assert_eq!(j["tvl"]["usd_cents"], 100_000_000_00u64);
    }

    #[test]
    fn test_daily_volume_tracking() {
        let m = Metrics::new();
        m.record_volume_usd_cents(500_000_00); // $500k
        m.record_volume_usd_cents(300_000_00); // $300k
        assert_eq!(m.daily_volume_usd_cents(), 800_000_00); // $800k
        assert_eq!(m.cumulative_volume_usd_cents(), 800_000_00);
    }

    #[test]
    fn test_volume_reset_daily() {
        let m = Metrics::new();
        m.record_volume_usd_cents(100_00);
        m.reset_daily();
        assert_eq!(m.daily_volume_usd_cents(), 0);
        // Cumulative should NOT reset
        assert_eq!(m.cumulative_volume_usd_cents(), 100_00);
    }

    #[test]
    fn test_tvl_prometheus_export() {
        let m = Metrics::new();
        m.set_tvl_usd_cents(50_000_00);
        let text = m.prometheus_text();
        assert!(text.contains("interlink_tvl_usd_cents"));
        assert!(text.contains("interlink_daily_volume_usd_cents"));
    }

    // ── Validator uptime tests (Phase 10) ───────────────────────────────────

    #[test]
    fn test_validator_uptime_tracking() {
        let m = Metrics::new();
        m.record_validator_heartbeats(950, 1000); // 95% uptime
        let uptime = m.validator_uptime_pct();
        assert!((uptime - 95.0).abs() < 0.1);
    }

    #[test]
    fn test_validator_uptime_perfect() {
        let m = Metrics::new();
        m.record_validator_heartbeats(1000, 1000); // 100%
        assert!((m.validator_uptime_pct() - 100.0).abs() < 0.01);
    }

    #[test]
    fn test_validator_uptime_no_data() {
        let m = Metrics::new();
        assert_eq!(m.validator_uptime_pct(), 100.0); // default
    }

    #[test]
    fn test_validator_uptime_json() {
        let m = Metrics::new();
        m.record_validator_heartbeats(9995, 10000);
        let j = m.as_json();
        assert_eq!(j["validators"]["heartbeats_received"], 9995);
        assert_eq!(j["validators"]["heartbeats_expected"], 10000);
    }
}
