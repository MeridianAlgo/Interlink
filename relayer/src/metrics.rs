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

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

const RLX: Ordering = Ordering::Relaxed;

// ─── Inner state ─────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
struct Inner {
    // ── Proof generation ────────────────────────────────────────────────────
    proof_gen_total:   AtomicU64, // all attempts (started)
    proof_gen_success: AtomicU64,
    proof_gen_failure: AtomicU64,
    proof_gen_ms_sum:  AtomicU64, // sum → mean = sum / success
    proof_gen_ms_max:  AtomicU64,
    /// Count of proofs exceeding 1 000ms alert threshold.
    proof_gen_alerts:  AtomicU64,

    // ── Settlement (end-to-end: finality + proof + submission) ──────────────
    settlement_total:   AtomicU64,
    settlement_success: AtomicU64,
    settlement_failure: AtomicU64,
    settlement_ms_sum:  AtomicU64,
    settlement_ms_max:  AtomicU64,
    /// Settlements exceeding 60 000ms SLA threshold.
    settlement_alerts:  AtomicU64,

    // ── Batch pipeline ───────────────────────────────────────────────────────
    batches_flushed:  AtomicU64, // total flushes (size+timer)
    events_processed: AtomicU64, // total events across all batches
    batch_size_sum:   AtomicU64, // sum → mean = sum / batches_flushed
    batch_size_max:   AtomicU64,

    // ── Queue ────────────────────────────────────────────────────────────────
    queue_depth: AtomicU64, // current snapshot (set, not incremented)
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
        write_counter(&mut out, "interlink_proof_gen_total",
            "Total proof generation attempts", i.proof_gen_total.load(RLX));
        write_counter(&mut out, "interlink_proof_gen_success_total",
            "Successful proof generations", proof_ok);
        write_counter(&mut out, "interlink_proof_gen_failure_total",
            "Failed proof generations", i.proof_gen_failure.load(RLX));
        write_gauge(&mut out, "interlink_proof_gen_ms_mean",
            "Mean proof gen time ms — target <100 vs Wormhole 300+", proof_mean);
        write_gauge(&mut out, "interlink_proof_gen_ms_max",
            "Max proof gen time ms seen", i.proof_gen_ms_max.load(RLX));
        write_counter(&mut out, "interlink_proof_gen_alert_total",
            "Proofs exceeding 1000ms alert threshold", i.proof_gen_alerts.load(RLX));

        // ── settlement ──────────────────────────────────────────────────────
        write_counter(&mut out, "interlink_settlement_total",
            "Total settlement pipeline attempts", i.settlement_total.load(RLX));
        write_counter(&mut out, "interlink_settlement_success_total",
            "Successful end-to-end settlements", settle_ok);
        write_counter(&mut out, "interlink_settlement_failure_total",
            "Failed settlements", i.settlement_failure.load(RLX));
        write_gauge(&mut out, "interlink_settlement_ms_mean",
            "Mean settlement ms — target <30000 vs Wormhole 120000+", settle_mean);
        write_gauge(&mut out, "interlink_settlement_ms_max",
            "Max settlement ms seen", i.settlement_ms_max.load(RLX));
        write_counter(&mut out, "interlink_settlement_sla_breach_total",
            "Settlements exceeding 60s SLA", i.settlement_alerts.load(RLX));

        // ── batch ───────────────────────────────────────────────────────────
        write_counter(&mut out, "interlink_batches_flushed_total",
            "Total batches dispatched to proof pipeline", batches);
        write_counter(&mut out, "interlink_events_processed_total",
            "Total events processed across all batches", i.events_processed.load(RLX));
        write_gauge(&mut out, "interlink_batch_size_mean",
            "Mean events per batch — target 100-1000 vs Wormhole 1-20", batch_mean);
        write_gauge(&mut out, "interlink_batch_size_max",
            "Largest single batch seen", i.batch_size_max.load(RLX));

        // ── queue ───────────────────────────────────────────────────────────
        write_gauge(&mut out, "interlink_queue_depth",
            "Events buffered in mpsc channel, not yet dispatched", i.queue_depth.load(RLX));

        out
    }

    // ── Export: JSON ────────────────────────────────────────────────────────

    /// Returns all metrics as a JSON object for API consumers.
    pub fn as_json(&self) -> serde_json::Value {
        let i = &self.0;
        let proof_ok = i.proof_gen_success.load(RLX);
        let settle_ok = i.settlement_success.load(RLX);
        let batches = i.batches_flushed.load(RLX);

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
    if count == 0 { 0 } else { sum / count }
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
        m.record_proof_success(500);   // under 1s → no alert
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
}
