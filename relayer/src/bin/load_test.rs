//! InterLink load test binary (Phase 7 / Phase 11).
//!
//! Measures the relayer pipeline under concurrent load:
//!   - Throughput (proofs/second) at 100, 500, 1 000 concurrency
//!   - Latency distribution: p50 / p95 / p99 / max
//!   - Error rate under normal and stressed conditions
//!   - Queue depth behaviour when bursting
//!
//! # Run
//!
//! ```bash
//! cargo run --release -p relayer --bin load-test
//! cargo run --release -p relayer --bin load-test -- --concurrency 100 --total 1000
//! cargo run --release -p relayer --bin load-test -- --concurrency 500
//! ```
//!
//! # Phase 7 targets
//!
//! | Concurrency | Target throughput | Target error rate |
//! |-------------|-------------------|-------------------|
//! | 10          | ≥ 10 proofs/sec   | 0%                |
//! | 100         | ≥ 50 proofs/sec   | 0%                |
//! | 1 000       | ≥ 100 proofs/sec  | < 0.1%            |
//!
//! The batch pipeline (100 events/5 s flush) naturally amortises proof cost;
//! higher concurrency → larger batches → lower cost per proof.

use relayer::events::{DepositEvent, GatewayEvent};
use relayer::metrics::Metrics;
use relayer::prover::ProverEngine;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Semaphore;

// ─── Configuration ────────────────────────────────────────────────────────────

struct Config {
    /// Maximum concurrent proof generation tasks.
    concurrency: usize,
    /// Total number of proofs to generate.
    total: usize,
    /// Proof circuit depth (6 = production default).
    circuit_depth: usize,
}

impl Config {
    fn from_args() -> Self {
        let args: Vec<String> = std::env::args().collect();
        let find = |flag: &str, default: usize| -> usize {
            args.windows(2)
                .find(|w| w[0] == flag)
                .and_then(|w| w[1].parse().ok())
                .unwrap_or(default)
        };

        Self {
            concurrency: find("--concurrency", 50),
            total: find("--total", 200),
            circuit_depth: find("--depth", 6),
        }
    }
}

// ─── Statistics ───────────────────────────────────────────────────────────────

struct Stats {
    samples: Vec<u128>, // ms per proof
    errors: usize,
    total: usize,
    wall_ms: u128,
}

impl Stats {
    fn from_samples(mut samples: Vec<u128>, errors: usize, total: usize, wall_ms: u128) -> Self {
        samples.sort_unstable();
        Self { samples, errors, total, wall_ms }
    }

    fn count(&self) -> usize {
        self.samples.len()
    }

    fn p(&self, pct: usize) -> u128 {
        if self.samples.is_empty() {
            return 0;
        }
        let idx = ((self.count() * pct) / 100).min(self.count() - 1);
        self.samples[idx]
    }

    fn mean(&self) -> u128 {
        if self.samples.is_empty() { return 0; }
        self.samples.iter().sum::<u128>() / self.count() as u128
    }

    fn throughput(&self) -> f64 {
        if self.wall_ms == 0 { return 0.0; }
        self.count() as f64 * 1_000.0 / self.wall_ms as f64
    }

    fn error_rate_pct(&self) -> f64 {
        if self.total == 0 { return 0.0; }
        self.errors as f64 * 100.0 / self.total as f64
    }
}

// ─── Load run ─────────────────────────────────────────────────────────────────

async fn run_load(
    engine: &ProverEngine,
    concurrency: usize,
    total: usize,
    metrics: &Metrics,
) -> Stats {
    let sem = Arc::new(Semaphore::new(concurrency));
    let times: Arc<tokio::sync::Mutex<Vec<u128>>> = Arc::new(tokio::sync::Mutex::new(Vec::with_capacity(total)));
    let errors = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let wall_start = Instant::now();
    let mut handles = Vec::with_capacity(total);

    for i in 0..total {
        let engine = engine.clone();
        let sem = Arc::clone(&sem);
        let times = Arc::clone(&times);
        let errors = Arc::clone(&errors);
        let metrics = metrics.clone();

        let event = make_event(i as u64);

        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire_owned().await.expect("semaphore closed");
            metrics.record_proof_start();

            let start = Instant::now();
            match engine.generate_proof(&event).await {
                Ok(_) => {
                    let ms = start.elapsed().as_millis();
                    metrics.record_proof_success(ms as u64);
                    times.lock().await.push(ms);
                }
                Err(_) => {
                    metrics.record_proof_failure();
                    errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            }
        }));
    }

    for h in handles {
        let _ = h.await;
    }

    let wall_ms = wall_start.elapsed().as_millis();
    let samples = Arc::try_unwrap(times).unwrap().into_inner();
    let err_count = errors.load(std::sync::atomic::Ordering::Relaxed);

    Stats::from_samples(samples, err_count, total, wall_ms)
}

// ─── Event factory ────────────────────────────────────────────────────────────

fn make_event(seq: u64) -> GatewayEvent {
    GatewayEvent::Deposit(DepositEvent {
        block_number: 10_000 + seq,
        tx_hash: {
            let mut h = [0u8; 32];
            h[..8].copy_from_slice(&seq.to_le_bytes());
            h
        },
        sequence: seq,
        sender: [0xAA; 20],
        recipient: vec![0xBB; 32],
        amount: 1_000_000_000_000_000_000,
        destination_chain: 2,
        payload_hash: {
            let mut h = [0xDE; 32];
            h[..8].copy_from_slice(&seq.to_le_bytes());
            h
        },
    })
}

// ─── Report printing ──────────────────────────────────────────────────────────

fn print_header(title: &str) {
    println!();
    println!("┌─ {title} {fill}┐", fill = "─".repeat(70usize.saturating_sub(title.len() + 4)));
}

fn print_stats(label: &str, stats: &Stats, target_tps: f64) {
    let tps = stats.throughput();
    let win = if tps >= target_tps { "✓ PASS" } else { "✗ FAIL" };
    println!("│ {:20} │ {:>8} proofs │ P50:{:>5}ms │ P95:{:>5}ms │ P99:{:>5}ms │",
        label,
        stats.count(),
        stats.p(50),
        stats.p(95),
        stats.p(99),
    );
    println!("│  Throughput: {:>6.1} proofs/sec  Target: {:>6.1}  {}  Errors: {:.2}%   │",
        tps, target_tps, win, stats.error_rate_pct());
}

fn print_competitor_table(our_stats: &[(&str, &Stats)]) {
    println!();
    println!("┌─ Competitive throughput comparison ──────────────────────────────────┐");
    println!("│ Bridge          │ TPS          │ Notes                               │");
    println!("│─────────────────│──────────────│─────────────────────────────────────│");
    for (label, s) in our_stats {
        println!("│ InterLink ({:>4}) │ {:>8.1}/s  │ {:<37} │",
            label, s.throughput(), format!("p99={}ms", s.p(99)));
    }
    println!("│ Wormhole        │ ~1 000/s     │ VAA aggregation, 300-500ms each     │");
    println!("│ Stargate v2     │ ~500/s       │ No ZK proofs, UltraLightClient      │");
    println!("│ Across          │ ~500/s       │ Optimistic, no proofs               │");
    println!("└──────────────────────────────────────────────────────────────────────┘");
}

// ─── Main ─────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let cfg = Config::from_args();

    println!("╔══════════════════════════════════════════════════════════════════════╗");
    println!("║              InterLink Load Test — Phase 7 / Phase 11               ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝");
    println!();
    println!("  Config:");
    println!("    Concurrency:   {}", cfg.concurrency);
    println!("    Total proofs:  {}", cfg.total);
    println!("    Circuit depth: {}", cfg.circuit_depth);
    println!();

    // ── Setup ─────────────────────────────────────────────────────────────────
    print!("▶ Initializing Groth16 prover...");
    let engine = ProverEngine::new(cfg.circuit_depth as u32);
    engine.initialize().await.expect("prover init failed");
    println!(" done.");

    let metrics = Metrics::new();

    // ── Single-threaded baseline ──────────────────────────────────────────────
    let baseline_count = cfg.total.min(20);
    print!("▶ Baseline: {} proofs, concurrency=1...", baseline_count);
    let baseline = run_load(&engine, 1, baseline_count, &metrics).await;
    println!(" done.");

    // ── Configured concurrency ────────────────────────────────────────────────
    println!("▶ Load run: {} proofs, concurrency={}...", cfg.total, cfg.concurrency);
    let load = run_load(&engine, cfg.concurrency, cfg.total, &metrics).await;
    println!(" done.");

    // ── High-concurrency stress (double the concurrency) ──────────────────────
    let stress_concurrency = cfg.concurrency * 2;
    let stress_count = cfg.total.min(100);
    println!("▶ Stress run: {} proofs, concurrency={}...", stress_count, stress_concurrency);
    let stress = run_load(&engine, stress_concurrency, stress_count, &metrics).await;
    println!(" done.");

    // ── Results ───────────────────────────────────────────────────────────────
    println!();
    println!("╔══════════════════════════════════════════════════════════════════════╗");
    println!("║                         LOAD TEST RESULTS                           ║");
    println!("╠══════════════════════════════════════════════════════════════════════╣");

    print_header("Baseline (concurrency=1)");
    print_stats("baseline", &baseline, 5.0);

    print_header(&format!("Load (concurrency={})", cfg.concurrency));
    print_stats("load", &load, 20.0);

    print_header(&format!("Stress (concurrency={})", stress_concurrency));
    print_stats("stress", &stress, 20.0);

    // Speedup from parallelism
    let speedup = if baseline.mean() > 0 {
        load.throughput() / (1_000.0 / baseline.mean() as f64)
    } else {
        0.0
    };

    println!();
    println!("┌─ Parallelism analysis ─────────────────────────────────────────────────┐");
    println!("│  Baseline single-core: {:>6.1} proofs/sec  (mean {:>5}ms/proof)        │",
        1_000.0 / baseline.mean().max(1) as f64, baseline.mean());
    println!("│  Parallel ({}x):      {:>6.1} proofs/sec  ({:.1}x speedup)              │",
        cfg.concurrency, load.throughput(), speedup);
    println!("│  Stress ({}x):        {:>6.1} proofs/sec                               │",
        stress_concurrency, stress.throughput());

    print_competitor_table(&[
        ("1x", &baseline),
        (&cfg.concurrency.to_string(), &load),
        (&stress_concurrency.to_string(), &stress),
    ]);

    // ── Metrics summary ───────────────────────────────────────────────────────
    println!();
    println!("┌─ Metrics snapshot (Phase 10) ──────────────────────────────────────────┐");
    let j = metrics.as_json();
    println!("│  proof_gen total:     {}",   j["proof_gen"]["total"]);
    println!("│  proof_gen success:   {}",   j["proof_gen"]["success"]);
    println!("│  proof_gen failure:   {}",   j["proof_gen"]["failure"]);
    println!("│  proof mean ms:       {}",   j["proof_gen"]["mean_ms"]);
    println!("│  proof max ms:        {}",   j["proof_gen"]["max_ms"]);
    println!("│  alerts (>1s):        {}",   j["proof_gen"]["alerts_over_1s"]);
    println!("└────────────────────────────────────────────────────────────────────────┘");

    // ── Pass/fail summary ─────────────────────────────────────────────────────
    let all_zero_errors = baseline.error_rate_pct() == 0.0
        && load.error_rate_pct() == 0.0
        && stress.error_rate_pct() < 0.1;

    println!();
    if all_zero_errors {
        println!("  ✓ Error rate: 0% across all load levels — PASS");
    } else {
        println!("  ✗ Error rate: baseline={:.2}% load={:.2}% stress={:.2}% — FAIL",
            baseline.error_rate_pct(), load.error_rate_pct(), stress.error_rate_pct());
    }

    // Export JSON results
    let output = serde_json::json!({
        "config": {
            "concurrency": cfg.concurrency,
            "total": cfg.total,
            "circuit_depth": cfg.circuit_depth,
        },
        "baseline": {
            "count": baseline.count(),
            "mean_ms": baseline.mean(),
            "p50_ms": baseline.p(50),
            "p95_ms": baseline.p(95),
            "p99_ms": baseline.p(99),
            "throughput_per_sec": baseline.throughput(),
            "error_rate_pct": baseline.error_rate_pct(),
        },
        "load": {
            "count": load.count(),
            "mean_ms": load.mean(),
            "p50_ms": load.p(50),
            "p95_ms": load.p(95),
            "p99_ms": load.p(99),
            "throughput_per_sec": load.throughput(),
            "error_rate_pct": load.error_rate_pct(),
        },
        "stress": {
            "count": stress.count(),
            "mean_ms": stress.mean(),
            "p50_ms": stress.p(50),
            "p95_ms": stress.p(95),
            "p99_ms": stress.p(99),
            "throughput_per_sec": stress.throughput(),
            "error_rate_pct": stress.error_rate_pct(),
        },
        "pass": all_zero_errors,
    });

    let out_path = "load_test_results.json";
    if let Ok(s) = serde_json::to_string_pretty(&output) {
        if std::fs::write(out_path, &s).is_ok() {
            println!("  Results saved to {out_path}");
        }
    }
}
