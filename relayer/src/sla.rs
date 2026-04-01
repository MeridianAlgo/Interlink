/// SLA monitoring and enforcement for InterLink (Phase 12)
///
/// Tracks uptime, settlement time p99, and response time against
/// published SLA targets. Generates breach reports for enterprise customers.
///
/// SLA targets:
///   - Uptime:         99.9% (8.76 hours max downtime per year)
///   - Settlement p99: <60 seconds
///   - API response:   <500ms p99
///
/// Comparison:
///   Wormhole: 99.95% uptime (guardians), no published settlement SLA
///   Across:   no formal SLA
///   LiFi:     ~200ms API latency target (undocumented)
///   InterLink: published SLA with automatic breach detection + reporting

use std::collections::VecDeque;

// ─── SLA Targets ─────────────────────────────────────────────────────────────

/// Target uptime in basis points (99.9% = 9990 bps).
pub const UPTIME_TARGET_BPS: u32 = 9990;
/// Maximum settlement time in milliseconds for SLA compliance.
pub const SETTLEMENT_P99_TARGET_MS: u64 = 60_000;
/// Maximum API response time in milliseconds.
pub const API_P99_TARGET_MS: u64 = 500;
/// Evaluation window for p99 calculations (max entries retained).
pub const WINDOW_SIZE: usize = 10_000;
/// Maximum allowed downtime per year in seconds (8h 45m for 99.9%).
pub const MAX_ANNUAL_DOWNTIME_SECS: u64 = 31_536; // 365.25 * 24 * 3600 * 0.001

// ─── Types ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlaMetric {
    Uptime,
    SettlementTime,
    ApiResponseTime,
}

#[derive(Debug, Clone)]
pub struct SlaBreach {
    pub metric: SlaMetric,
    pub target: String,
    pub actual: String,
    pub timestamp: u64,
    pub window_entries: usize,
}

#[derive(Debug, Clone)]
pub struct SlaReport {
    pub uptime_bps: u32,
    pub uptime_compliant: bool,
    pub settlement_p99_ms: u64,
    pub settlement_compliant: bool,
    pub api_p99_ms: u64,
    pub api_compliant: bool,
    pub breaches: Vec<SlaBreach>,
    pub overall_compliant: bool,
}

// ─── SLA Monitor ─────────────────────────────────────────────────────────────

pub struct SlaMonitor {
    /// Uptime tracking
    total_secs: u64,
    downtime_secs: u64,
    /// Settlement time samples (ms)
    settlement_samples: VecDeque<u64>,
    /// API response time samples (ms)
    api_samples: VecDeque<u64>,
    /// Historical breaches
    breaches: Vec<SlaBreach>,
}

impl SlaMonitor {
    pub fn new() -> Self {
        SlaMonitor {
            total_secs: 0,
            downtime_secs: 0,
            settlement_samples: VecDeque::new(),
            api_samples: VecDeque::new(),
            breaches: Vec::new(),
        }
    }

    // ── Uptime ──────────────────────────────────────────────────────────────

    /// Record operational seconds since last check.
    pub fn record_uptime(&mut self, operational_secs: u64, downtime_secs: u64) {
        self.total_secs += operational_secs + downtime_secs;
        self.downtime_secs += downtime_secs;
    }

    /// Current uptime in basis points.
    pub fn uptime_bps(&self) -> u32 {
        if self.total_secs == 0 {
            return 10_000; // 100% by default
        }
        let up = self.total_secs.saturating_sub(self.downtime_secs);
        (up as u128 * 10_000 / self.total_secs as u128) as u32
    }

    // ── Settlement Time ─────────────────────────────────────────────────────

    /// Record a settlement time sample (in milliseconds).
    pub fn record_settlement(&mut self, ms: u64) {
        if self.settlement_samples.len() >= WINDOW_SIZE {
            self.settlement_samples.pop_front();
        }
        self.settlement_samples.push_back(ms);
    }

    /// Compute p99 settlement time from the current window.
    pub fn settlement_p99_ms(&self) -> u64 {
        percentile_99(&self.settlement_samples)
    }

    // ── API Response Time ───────────────────────────────────────────────────

    /// Record an API response time sample (in milliseconds).
    pub fn record_api_response(&mut self, ms: u64) {
        if self.api_samples.len() >= WINDOW_SIZE {
            self.api_samples.pop_front();
        }
        self.api_samples.push_back(ms);
    }

    /// Compute p99 API response time from the current window.
    pub fn api_p99_ms(&self) -> u64 {
        percentile_99(&self.api_samples)
    }

    // ── Evaluation ──────────────────────────────────────────────────────────

    /// Evaluate all SLA metrics and generate a report.
    pub fn evaluate(&mut self, now: u64) -> SlaReport {
        let uptime = self.uptime_bps();
        let settlement_p99 = self.settlement_p99_ms();
        let api_p99 = self.api_p99_ms();

        let uptime_ok = uptime >= UPTIME_TARGET_BPS;
        let settlement_ok = settlement_p99 <= SETTLEMENT_P99_TARGET_MS;
        let api_ok = api_p99 <= API_P99_TARGET_MS;

        let mut report_breaches = Vec::new();

        if !uptime_ok {
            let breach = SlaBreach {
                metric: SlaMetric::Uptime,
                target: format!("{}bps", UPTIME_TARGET_BPS),
                actual: format!("{}bps", uptime),
                timestamp: now,
                window_entries: 0,
            };
            self.breaches.push(breach.clone());
            report_breaches.push(breach);
        }
        if !settlement_ok {
            let breach = SlaBreach {
                metric: SlaMetric::SettlementTime,
                target: format!("{}ms", SETTLEMENT_P99_TARGET_MS),
                actual: format!("{}ms", settlement_p99),
                timestamp: now,
                window_entries: self.settlement_samples.len(),
            };
            self.breaches.push(breach.clone());
            report_breaches.push(breach);
        }
        if !api_ok {
            let breach = SlaBreach {
                metric: SlaMetric::ApiResponseTime,
                target: format!("{}ms", API_P99_TARGET_MS),
                actual: format!("{}ms", api_p99),
                timestamp: now,
                window_entries: self.api_samples.len(),
            };
            self.breaches.push(breach.clone());
            report_breaches.push(breach);
        }

        SlaReport {
            uptime_bps: uptime,
            uptime_compliant: uptime_ok,
            settlement_p99_ms: settlement_p99,
            settlement_compliant: settlement_ok,
            api_p99_ms: api_p99,
            api_compliant: api_ok,
            breaches: report_breaches,
            overall_compliant: uptime_ok && settlement_ok && api_ok,
        }
    }

    /// All historical breaches.
    pub fn breach_history(&self) -> &[SlaBreach] {
        &self.breaches
    }

    /// JSON representation of the current SLA status.
    pub fn status_json(&self) -> serde_json::Value {
        serde_json::json!({
            "uptime_bps": self.uptime_bps(),
            "uptime_target_bps": UPTIME_TARGET_BPS,
            "settlement_p99_ms": self.settlement_p99_ms(),
            "settlement_target_ms": SETTLEMENT_P99_TARGET_MS,
            "api_p99_ms": self.api_p99_ms(),
            "api_target_ms": API_P99_TARGET_MS,
            "total_breaches": self.breaches.len(),
        })
    }
}

impl Default for SlaMonitor {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn percentile_99(samples: &VecDeque<u64>) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    let mut sorted: Vec<u64> = samples.iter().copied().collect();
    sorted.sort_unstable();
    let idx = ((sorted.len() as f64 * 0.99).ceil() as usize).saturating_sub(1);
    sorted[idx.min(sorted.len() - 1)]
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_uptime_100_percent() {
        let m = SlaMonitor::new();
        assert_eq!(m.uptime_bps(), 10_000);
    }

    #[test]
    fn test_uptime_calculation() {
        let mut m = SlaMonitor::new();
        m.record_uptime(9990, 10); // 9990 up + 10 down = 99.9%
        assert_eq!(m.uptime_bps(), 9990);
    }

    #[test]
    fn test_uptime_below_sla() {
        let mut m = SlaMonitor::new();
        m.record_uptime(990, 10); // 99.0% — below 99.9% target
        let report = m.evaluate(100);
        assert!(!report.uptime_compliant);
        assert!(!report.overall_compliant);
        assert_eq!(report.breaches.len(), 1);
        assert_eq!(report.breaches[0].metric, SlaMetric::Uptime);
    }

    #[test]
    fn test_settlement_p99_within_sla() {
        let mut m = SlaMonitor::new();
        // 100 samples, all under 60s
        for i in 0..100 {
            m.record_settlement(i * 500); // 0ms to 49500ms
        }
        let p99 = m.settlement_p99_ms();
        assert!(p99 <= SETTLEMENT_P99_TARGET_MS, "p99={p99} should be ≤ {SETTLEMENT_P99_TARGET_MS}");
    }

    #[test]
    fn test_settlement_p99_breach() {
        let mut m = SlaMonitor::new();
        // 95 fast, 5 very slow → p99 index hits the slow group
        for _ in 0..95 {
            m.record_settlement(1000); // 1s
        }
        for _ in 0..5 {
            m.record_settlement(120_000); // 120s — breach
        }
        let p99 = m.settlement_p99_ms();
        assert_eq!(p99, 120_000);
        let report = m.evaluate(200);
        assert!(!report.settlement_compliant);
    }

    #[test]
    fn test_api_p99_within_sla() {
        let mut m = SlaMonitor::new();
        for _ in 0..100 {
            m.record_api_response(50); // 50ms
        }
        assert!(m.api_p99_ms() <= API_P99_TARGET_MS);
    }

    #[test]
    fn test_api_p99_breach() {
        let mut m = SlaMonitor::new();
        for _ in 0..95 {
            m.record_api_response(100); // fast
        }
        for _ in 0..5 {
            m.record_api_response(2_000); // 2s — breach
        }
        let report = m.evaluate(300);
        assert!(!report.api_compliant);
    }

    #[test]
    fn test_overall_compliant_all_pass() {
        let mut m = SlaMonitor::new();
        m.record_uptime(99_990, 10); // 99.99%
        for _ in 0..100 { m.record_settlement(5_000); } // 5s each
        for _ in 0..100 { m.record_api_response(100); } // 100ms each
        let report = m.evaluate(400);
        assert!(report.overall_compliant);
        assert!(report.breaches.is_empty());
    }

    #[test]
    fn test_window_eviction() {
        let mut m = SlaMonitor::new();
        // Fill window
        for _ in 0..WINDOW_SIZE {
            m.record_settlement(100);
        }
        // Add a slow one — should evict oldest
        m.record_settlement(200);
        assert_eq!(m.settlement_samples.len(), WINDOW_SIZE);
    }

    #[test]
    fn test_breach_history_accumulates() {
        let mut m = SlaMonitor::new();
        m.record_uptime(900, 100); // 90% — breach
        m.evaluate(1);
        m.evaluate(2);
        assert_eq!(m.breach_history().len(), 2); // two evaluations, two breaches
    }

    #[test]
    fn test_status_json() {
        let mut m = SlaMonitor::new();
        m.record_uptime(9990, 10);
        m.record_settlement(500);
        m.record_api_response(50);
        let j = m.status_json();
        assert_eq!(j["uptime_target_bps"], 9990);
        assert!(j["settlement_p99_ms"].is_number());
    }

    #[test]
    fn test_empty_samples_p99_zero() {
        let m = SlaMonitor::new();
        assert_eq!(m.settlement_p99_ms(), 0);
        assert_eq!(m.api_p99_ms(), 0);
    }
}
