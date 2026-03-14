//! Integration tests for the InterLink relayer pipeline.
//!
//! These tests exercise the full proof lifecycle:
//!   event creation → proof generation → proof verification → package integrity
//!
//! Phase 11 goal: 80%+ code coverage, 100% critical-path coverage.
//! All tests run without a live node — no external dependencies.

use relayer::batch::BatchCollector;
use relayer::events::{DepositEvent, GatewayEvent, NFTLockedEvent, SwapInitiatedEvent};
use relayer::fee::{self, FeeTier};
use relayer::gas;
use relayer::prover::ProverEngine;
use std::time::Duration;

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn deposit(seq: u64) -> GatewayEvent {
    GatewayEvent::Deposit(DepositEvent {
        block_number: 100 + seq,
        tx_hash: {
            let mut h = [0u8; 32];
            h[..8].copy_from_slice(&seq.to_le_bytes());
            h
        },
        sequence: seq,
        sender: [0xAA; 20],
        recipient: vec![0xBB; 32],
        amount: 1_000_000_000_000_000_000, // 1 ETH
        destination_chain: 2,
        payload_hash: {
            let mut h = [0xAB; 32];
            h[..8].copy_from_slice(&seq.to_le_bytes());
            h
        },
    })
}

fn swap_event(seq: u64) -> GatewayEvent {
    GatewayEvent::Swap(SwapInitiatedEvent {
        block_number: 200 + seq,
        tx_hash: [0x11; 32],
        sequence: seq,
        sender: [0xAA; 20],
        recipient: [0xBB; 20],
        amount_in: 5_000_000_000_000_000_000, // 5 ETH
        token_in: [0xCC; 20],
        token_out: [0xDD; 20],
        min_amount_out: 4_900_000_000_000_000_000,
        destination_chain: 2,
        swap_data: vec![0x01, 0x02, 0x03],
        payload_hash: [0xCD; 32],
    })
}

fn nft_event(seq: u64) -> GatewayEvent {
    GatewayEvent::NFTLock(NFTLockedEvent {
        block_number: 300 + seq,
        tx_hash: [0x22; 32],
        sequence: seq,
        sender: [0xAA; 20],
        nft_contract: [0xEE; 20],
        token_id: [0x01; 32],
        destination_chain: 2,
        destination_recipient: [0xFF; 32],
        nft_hash: [0x42; 32],
    })
}

// ─── Prover integration tests ─────────────────────────────────────────────────

#[tokio::test]
async fn test_full_deposit_proof_lifecycle() {
    let engine = ProverEngine::new(6);
    engine.initialize().await.expect("prover init failed");

    let event = deposit(42);
    let package = engine
        .generate_proof(&event)
        .await
        .expect("proof generation failed");

    // Proof format: A (64 bytes G1) + B (128 bytes G2) + C (64 bytes G1) = 256 bytes
    assert_eq!(package.proof_bytes.len(), 256, "proof must be exactly 256 bytes");
    assert_eq!(package.sequence, 42);
    // Commitment public input must be 32 bytes
    assert_eq!(package.public_inputs.len(), 32, "public input must be 32 bytes");
    // Payload hash must match what the deposit() helper encodes:
    // bytes [0..8] = seq(42) as little-endian, rest = 0xAB
    let mut expected_hash = [0xAB; 32];
    expected_hash[..8].copy_from_slice(&42u64.to_le_bytes());
    assert_eq!(package.payload_hash, expected_hash, "payload hash must match event");
}

#[tokio::test]
async fn test_proof_for_all_event_types() {
    let engine = ProverEngine::new(6);
    engine.initialize().await.expect("prover init failed");

    // All three event types must produce valid proofs
    for (name, event) in [
        ("deposit", deposit(1)),
        ("swap", swap_event(2)),
        ("nft", nft_event(3)),
    ] {
        let package = engine
            .generate_proof(&event)
            .await
            .unwrap_or_else(|e| panic!("{} proof failed: {}", name, e));

        assert_eq!(package.proof_bytes.len(), 256, "{}: proof must be 256 bytes", name);
        assert!(!package.proof_bytes.iter().all(|&b| b == 0), "{}: proof must not be all zeros", name);
    }
}

#[tokio::test]
async fn test_proofs_are_deterministic_for_same_event() {
    let engine = ProverEngine::new(6);
    engine.initialize().await.expect("prover init failed");

    let event = deposit(99);

    // Generate two proofs for the same event — the PUBLIC INPUTS must match
    // (proofs themselves may differ due to randomised Groth16, but commitments are deterministic)
    let p1 = engine.generate_proof(&event).await.expect("proof 1 failed");
    let p2 = engine.generate_proof(&event).await.expect("proof 2 failed");

    assert_eq!(p1.public_inputs, p2.public_inputs, "commitment must be deterministic");
    assert_eq!(p1.payload_hash, p2.payload_hash);
    assert_eq!(p1.sequence, p2.sequence);
}

#[tokio::test]
async fn test_different_events_produce_different_commitments() {
    let engine = ProverEngine::new(6);
    engine.initialize().await.expect("prover init failed");

    let p1 = engine.generate_proof(&deposit(1)).await.expect("proof 1 failed");
    let p2 = engine.generate_proof(&deposit(2)).await.expect("proof 2 failed");

    // Different sequence numbers must produce different commitments
    assert_ne!(
        p1.public_inputs, p2.public_inputs,
        "different events must produce different public inputs"
    );
}

#[tokio::test]
async fn test_concurrent_proof_generation() {
    let engine = ProverEngine::new(6);
    engine.initialize().await.expect("prover init failed");

    // Spawn 10 concurrent proof generation tasks
    let mut handles = Vec::new();
    for i in 0..10 {
        let e = engine.clone();
        let event = deposit(i as u64 + 100);
        handles.push(tokio::spawn(async move {
            e.generate_proof(&event).await
        }));
    }

    let results: Vec<_> = futures::future::join_all(handles).await;
    for (i, result) in results.into_iter().enumerate() {
        let package = result
            .expect("task panicked")
            .unwrap_or_else(|e| panic!("proof {} failed: {}", i, e));
        assert_eq!(package.proof_bytes.len(), 256, "proof {} must be 256 bytes", i);
    }
}

// ─── Batch collector integration tests ───────────────────────────────────────

#[test]
fn test_batch_collects_and_flushes_on_size() {
    let mut collector = BatchCollector::new(5, Duration::from_secs(60));

    for i in 0..4 {
        assert!(collector.push(deposit(i)).is_none(), "batch should not flush before size");
    }

    // 5th event triggers flush
    let batch = collector.push(deposit(4)).expect("batch should flush at max_size");
    assert_eq!(batch.len(), 5);
    assert_eq!(batch.batch_id, 0);
    assert_eq!(collector.pending_count(), 0);
}

#[test]
fn test_batch_carries_block_range() {
    let mut collector = BatchCollector::new(10, Duration::from_millis(1));
    collector.push(deposit(1)); // block 101
    collector.push(deposit(5)); // block 105
    collector.push(deposit(3)); // block 103

    std::thread::sleep(Duration::from_millis(5));
    let batch = collector.flush_timer().unwrap();

    // min/max block should span the range of event blocks
    assert!(batch.min_block() <= batch.max_block());
}

#[test]
fn test_multiple_batches_have_sequential_ids() {
    let mut collector = BatchCollector::new(2, Duration::from_secs(60));

    // max_size=2: first push doesn't flush, second does
    assert!(collector.push(deposit(0)).is_none(), "1 event < max_size, no flush");
    let b0 = collector.push(deposit(1)).expect("2nd event should flush batch 0");
    // New batch: same pattern
    assert!(collector.push(deposit(2)).is_none(), "1 event < max_size, no flush");
    let b1 = collector.push(deposit(3)).expect("2nd event should flush batch 1");

    assert_eq!(b0.batch_id, 0);
    assert_eq!(b1.batch_id, 1);
}

// ─── Fee module integration tests ────────────────────────────────────────────

#[test]
fn test_fee_tiers_cover_all_ranges() {
    let cases = [
        (0, FeeTier::Zero),
        (9_999, FeeTier::Zero),          // $99.99
        (100_000, FeeTier::Standard),    // $1,000.00
        (9_999_999, FeeTier::Standard),  // $99,999.99
        (10_000_000, FeeTier::Institutional), // $100,000
        (999_999_999, FeeTier::Institutional), // $9,999,999.99
        (1_000_000_000, FeeTier::OTC),   // $10M
        (u64::MAX, FeeTier::OTC),
    ];
    for (cents, expected) in cases {
        assert_eq!(
            FeeTier::from_usd_cents(cents), expected,
            "cents={} should be {:?}", cents, expected
        );
    }
}

#[test]
fn test_fee_zero_for_tier1_regardless_of_amount() {
    // No matter how large the token amount, Tier 1 is always free
    for amount in [1u128, 1_000_000, u64::MAX as u128, u128::MAX / 10_000] {
        assert_eq!(fee::calculate_fee(amount, 99_999), 0, "Tier 1 must be free, amount={}", amount);
    }
}

#[test]
fn test_fee_standard_tier_precision() {
    // 1 ETH (1e18 wei) at $3,000 → Standard tier (5 bps) → 0.05%
    // fee = 1e18 × 5 / 10_000 = 5e14 wei
    let amount = 1_000_000_000_000_000_000u128;
    let usd_cents = 300_000; // $3,000
    let fee = fee::calculate_fee(amount, usd_cents);
    assert_eq!(fee, 500_000_000_000_000, "0.05% of 1e18 = 5e14 wei");
}

#[test]
fn test_amount_after_fee_never_overflows() {
    // Should never return more than the input amount
    for amount in [0u128, 1, 1_000_000, u128::MAX / 2] {
        let after = fee::amount_after_fee(amount, 100_000);
        assert!(after <= amount, "recipient can't receive more than was sent");
    }
}

// ─── Gas estimation integration tests ────────────────────────────────────────

#[test]
fn test_gas_estimate_interlink_cheapest_vs_percentage_bridges() {
    // InterLink wins vs Stargate (0.5-5%) and Across (0.25-1%) at every transfer size.
    // Wormhole uses a flat $1 fee which beats us at large institutional sizes —
    // but their 15-min settlement and 19-guardian trust model are different trade-offs.
    for usd_cents in [1_000u64, 100_000, 1_000_000, 10_000_000] {
        let cmp = gas::compare(usd_cents);
        let interlink = cmp.interlink.fee_usd_cents;

        let stargate = cmp.competitors.iter().find(|c| c.name == "Stargate v2").unwrap();
        let across = cmp.competitors.iter().find(|c| c.name == "Across").unwrap();

        assert!(
            interlink <= stargate.fee_usd_cents,
            "InterLink must beat Stargate at ${:.2}",
            usd_cents as f64 / 100.0
        );
        assert!(
            interlink <= across.fee_usd_cents,
            "InterLink must beat Across at ${:.2}",
            usd_cents as f64 / 100.0
        );
    }
}

#[test]
fn test_gas_estimate_interlink_always_fastest() {
    for usd_cents in [1_000u64, 100_000, 1_000_000] {
        let cmp = gas::compare(usd_cents);
        assert!(
            cmp.interlink_wins_on_speed(),
            "InterLink 30s must beat all competitors at ${:.2}",
            usd_cents as f64 / 100.0
        );
    }
}

#[test]
fn test_savings_positive_for_small_transfers() {
    // Small transfers: InterLink is free, Wormhole charges $1+
    let cmp = gas::compare(10_000); // $100
    let savings = cmp.savings_vs_cheapest_cents();
    assert!(
        savings > 0,
        "InterLink should save money vs cheapest competitor for $100 transfer, but savings={}",
        savings
    );
}

#[test]
fn test_batch_amortises_proof_cost() {
    let single_batch = gas::estimate(1_000_000u128, 100_000, 30, 1, 3_000);
    let large_batch = gas::estimate(1_000_000u128, 100_000, 30, 100, 3_000);

    // Amortised proof cost at batch=100 should be exactly 1/100th of single
    assert_eq!(
        large_batch.proof_cost_amortised_wei * 100,
        single_batch.proof_cost_amortised_wei,
        "proof cost must amortise linearly with batch size"
    );
}

// ─── Event encoding tests ─────────────────────────────────────────────────────

#[test]
fn test_event_accessors_all_types() {
    let d = deposit(42);
    assert_eq!(d.sequence(), 42);
    assert_eq!(d.block_number(), 142);

    let s = swap_event(7);
    assert_eq!(s.sequence(), 7);
    assert_eq!(s.block_number(), 207);

    let n = nft_event(3);
    assert_eq!(n.sequence(), 3);
    assert_eq!(n.block_number(), 303);
}

#[test]
fn test_payload_hash_differs_per_event_type() {
    let d = deposit(1);
    let n = nft_event(1);
    // Different event types with same sequence should have different payload hashes
    assert_ne!(d.payload_hash(), n.payload_hash());
}
