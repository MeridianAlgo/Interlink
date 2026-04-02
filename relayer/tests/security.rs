/// Security test suite for the InterLink relayer (Phase 11 / Testing Infrastructure)
///
/// Tests attack vectors that a cross-chain bridge must withstand:
///
/// 1. Sequence binding: different sequences yield different commitments
/// 2. Byzantine validator: forge threshold bundles with insufficient signers
/// 3. Oversized / malformed input: zero amounts, boundary values, max-u128 fee
/// 4. AMM manipulation: price impact guard rejects large manipulative swaps
/// 5. Governance attack: below-threshold proposals, duplicate votes, premature execute
/// 6. Rate-limit bypass: cannot exceed free-tier cap
/// 7. Webhook DoS: auto-disable after 10 consecutive failures via record_delivery
///
/// All tests run entirely in-process — no live chain needed.

// ─── Sequence / Replay ────────────────────────────────────────────────────────

mod sequence_binding {
    use relayer::multisig::compute_commitment;

    const PROGRAM_ID: [u8; 32] = [0xAB; 32];

    /// Two distinct commitments with identical payload but different sequence numbers
    /// must produce different commitment bytes (prevents sequence-number replay).
    #[test]
    fn different_sequences_produce_different_commitments() {
        let payload = b"transfer:1ETH:alice->bob";
        let c1 = compute_commitment(payload, 1, 1, &PROGRAM_ID);
        let c2 = compute_commitment(payload, 2, 1, &PROGRAM_ID);
        assert_ne!(
            c1, c2,
            "different sequences must yield different commitments"
        );
    }

    /// Same inputs must yield the same commitment (determinism required for all signers).
    #[test]
    fn same_input_is_deterministic() {
        let payload = b"transfer:1ETH:alice->bob";
        let c1 = compute_commitment(payload, 42, 1, &PROGRAM_ID);
        let c2 = compute_commitment(payload, 42, 1, &PROGRAM_ID);
        assert_eq!(c1, c2);
    }

    /// Different source chains produce different commitments (prevents cross-chain replay).
    #[test]
    fn different_source_chains_produce_different_commitments() {
        let payload = b"data";
        let c_eth = compute_commitment(payload, 1, 1, &PROGRAM_ID); // Ethereum
        let c_sol = compute_commitment(payload, 1, 900, &PROGRAM_ID); // Solana
        assert_ne!(c_eth, c_sol);
    }
}

// ─── Byzantine Validator ──────────────────────────────────────────────────────

mod byzantine {
    use relayer::multisig::{
        add_signature, create_bundle, verify_bundle, MultiSigError, ValidatorId, ValidatorSet,
    };

    const PROGRAM_ID: [u8; 32] = [0xAB; 32];

    fn make_validator_set(n: usize, threshold: usize) -> ValidatorSet {
        let validators: Vec<ValidatorId> = (0..n)
            .map(|i| {
                let mut key = [0u8; 32];
                key[0] = (i + 1) as u8;
                ValidatorId::new(key, i, format!("v{i}"))
            })
            .collect();
        ValidatorSet::new(validators, threshold).unwrap()
    }

    /// 3-of-5 threshold: 2 signatures must NOT pass verification.
    #[test]
    fn two_of_five_is_insufficient() {
        let vs = make_validator_set(5, 3);
        let payload = b"malicious_transfer";
        let mut bundle = create_bundle(payload, 1, 1, &PROGRAM_ID, &vs);

        // Only two validators sign (with dummy 64-byte signatures)
        add_signature(&mut bundle, 0, [1u8; 64], &vs, 0).unwrap();
        add_signature(&mut bundle, 1, [2u8; 64], &vs, 0).unwrap();

        let err = verify_bundle(&bundle, &vs).unwrap_err();
        assert_eq!(
            err,
            MultiSigError::InsufficientSignatures { got: 2, need: 3 },
            "2-of-5 must not reach quorum"
        );
    }

    /// A validator index outside the set must be rejected.
    #[test]
    fn out_of_range_validator_rejected() {
        let vs = make_validator_set(5, 3);
        let payload = b"data";
        let mut bundle = create_bundle(payload, 1, 1, &PROGRAM_ID, &vs);

        let err = add_signature(&mut bundle, 99, [0u8; 64], &vs, 0).unwrap_err();
        assert_eq!(
            err,
            MultiSigError::UnknownValidator { index: 99 },
            "outsider validator index must be rejected"
        );
    }

    /// Duplicate validator index in the same bundle must be rejected.
    #[test]
    fn duplicate_validator_rejected() {
        let vs = make_validator_set(5, 3);
        let payload = b"data";
        let mut bundle = create_bundle(payload, 1, 1, &PROGRAM_ID, &vs);

        add_signature(&mut bundle, 0, [1u8; 64], &vs, 0).unwrap();
        let err = add_signature(&mut bundle, 0, [2u8; 64], &vs, 0).unwrap_err();
        assert_eq!(err, MultiSigError::DuplicateValidator { index: 0 },);
    }

    /// 3-of-5 with exactly 3 signatures must verify successfully.
    #[test]
    fn exact_threshold_verifies() {
        let vs = make_validator_set(5, 3);
        let payload = b"valid_transfer";
        let mut bundle = create_bundle(payload, 10, 1, &PROGRAM_ID, &vs);

        add_signature(&mut bundle, 0, [1u8; 64], &vs, 0).unwrap();
        add_signature(&mut bundle, 2, [3u8; 64], &vs, 0).unwrap();
        add_signature(&mut bundle, 4, [5u8; 64], &vs, 0).unwrap();

        assert!(verify_bundle(&bundle, &vs).is_ok());
    }

    /// Threshold requiring more than validator count must be rejected at construction.
    #[test]
    fn threshold_exceeding_set_size_rejected() {
        let validators: Vec<ValidatorId> = (0..1)
            .map(|i| {
                let mut key = [0u8; 32];
                key[0] = 1;
                ValidatorId::new(key, i, "solo")
            })
            .collect();
        // 2-of-1 is impossible
        let err = ValidatorSet::new(validators, 2).unwrap_err();
        assert!(
            matches!(err, MultiSigError::InvalidThreshold { .. }),
            "threshold > set size must be rejected"
        );
    }

    /// Commitment binds to sequence: a bundle created for seq 100 will have a
    /// different commitment than one for seq 101.
    #[test]
    fn bundle_commitment_binds_to_sequence() {
        let vs = make_validator_set(3, 2);
        let payload = b"transfer";

        let b1 = create_bundle(payload, 100, 1, &PROGRAM_ID, &vs);
        let b2 = create_bundle(payload, 101, 1, &PROGRAM_ID, &vs);

        assert_ne!(
            b1.commitment, b2.commitment,
            "bundle commitments for different sequences must differ"
        );
    }
}

// ─── Oversized / Malformed Input ─────────────────────────────────────────────

mod malformed_input {
    use relayer::amm::{Pool, PoolId};
    use relayer::fee::FeeTier;

    fn make_pool() -> Pool {
        let id = PoolId {
            token_a: [1u8; 20],
            token_b: [2u8; 20],
            chain_a: 1,
            chain_b: 900,
        };
        let mut pool = Pool::new(id);
        pool.add_initial_liquidity(1_000_000, 1_000_000).unwrap();
        pool
    }

    /// AMM must reject a zero-amount swap without panic.
    #[test]
    fn amm_zero_swap_rejected() {
        let mut pool = make_pool();
        let err = pool.swap_a_for_b(0, 0);
        assert!(err.is_err(), "zero-amount swap must be rejected");
    }

    /// AMM must reject a swap that exceeds price impact guard.
    #[test]
    fn amm_oversized_swap_rejected() {
        let mut pool = make_pool();
        // Swap 10% of the reserve — well above 5% MAX_PRICE_IMPACT_BPS
        let err = pool.swap_a_for_b(100_001, 0);
        assert!(
            err.is_err(),
            "price-impact guard must reject oversized swap"
        );
    }

    /// Fee classifier must handle maximum value without panic.
    #[test]
    fn fee_tier_max_value() {
        let tier = FeeTier::from_usd_cents(u64::MAX);
        assert_eq!(tier, FeeTier::OTC, "u64::MAX should be OTC tier");
    }

    /// Zero-amount transfer must be free (Tier Zero).
    #[test]
    fn fee_tier_zero_amount() {
        let tier = FeeTier::from_usd_cents(0);
        assert_eq!(tier, FeeTier::Zero);
    }

    /// Initial liquidity with zero amount must be rejected.
    #[test]
    fn amm_initial_liquidity_zero_rejected() {
        let id = PoolId {
            token_a: [0u8; 20],
            token_b: [1u8; 20],
            chain_a: 1,
            chain_b: 2,
        };
        let mut pool = Pool::new(id);
        assert!(pool.add_initial_liquidity(0, 1000).is_err());
        assert!(pool.add_initial_liquidity(1000, 0).is_err());
    }
}

// ─── AMM Price Manipulation ────────────────────────────────────────────────────

mod amm_manipulation {
    use relayer::amm::{Pool, PoolId, MAX_PRICE_IMPACT_BPS};

    fn pool_with_liquidity(ra: u128, rb: u128) -> Pool {
        let id = PoolId {
            token_a: [0xAAu8; 20],
            token_b: [0xBBu8; 20],
            chain_a: 1,
            chain_b: 137,
        };
        let mut pool = Pool::new(id);
        pool.add_initial_liquidity(ra, rb).unwrap();
        pool
    }

    /// A small swap well within limits must succeed and report valid price impact.
    #[test]
    fn small_swap_within_price_impact() {
        let mut pool = pool_with_liquidity(1_000_000, 1_000_000);
        let result = pool.swap_a_for_b(1_000, 0).unwrap();
        let impact_bps = result.price_impact_bps;
        assert!(
            impact_bps <= MAX_PRICE_IMPACT_BPS,
            "small swap price impact ({impact_bps} bps) must be within guard"
        );
    }

    /// Pool invariant k = ra × rb must be maintained (≥) after swap.
    #[test]
    fn constant_product_invariant_maintained() {
        let mut pool = pool_with_liquidity(100_000, 100_000);
        let k_before = pool.k();
        pool.swap_a_for_b(100, 0).unwrap(); // small swap
        let k_after = pool.k();
        assert!(
            k_after >= k_before,
            "k must not decrease after swap (LP fee stays in pool)"
        );
    }

    /// Slippage must increase monotonically with trade size.
    #[test]
    fn slippage_increases_with_trade_size() {
        let pool = pool_with_liquidity(1_000_000, 1_000_000);
        let slip_small = pool.slippage_bps(100);
        let slip_large = pool.slippage_bps(10_000);
        assert!(
            slip_large >= slip_small,
            "slippage must increase with trade size: {slip_small} -> {slip_large}"
        );
    }

    /// Quote and actual swap must return identical output (no front-running gap).
    #[test]
    fn quote_matches_actual_swap() {
        let mut pool = pool_with_liquidity(500_000, 500_000);
        let quoted = pool.quote_a_for_b(500);
        let result = pool.swap_a_for_b(500, 0).unwrap();
        assert_eq!(
            quoted, result.amount_out,
            "quote must exactly match actual swap output"
        );
    }
}

// ─── Governance Attack ────────────────────────────────────────────────────────

mod governance_attack {
    use relayer::governance::{
        Governance, GovernanceError, ProposalKind, VoteChoice, PROPOSAL_THRESHOLD,
        TIMELOCK_DELAY_SECS, VOTING_PERIOD_SECS,
    };

    fn gov() -> Governance {
        Governance::new()
    }

    /// An attacker with fewer than PROPOSAL_THRESHOLD tokens cannot create proposals.
    #[test]
    fn low_token_holder_cannot_propose() {
        let mut g = gov();
        let err = g
            .propose(
                ProposalKind::UpdateFees,
                "drain treasury",
                "send all funds to attacker",
                "attacker",
                PROPOSAL_THRESHOLD - 1,
                0,
            )
            .unwrap_err();
        assert_eq!(
            err,
            GovernanceError::BelowProposalThreshold {
                have: PROPOSAL_THRESHOLD - 1,
                need: PROPOSAL_THRESHOLD,
            }
        );
    }

    /// The same address cannot vote twice on the same proposal.
    #[test]
    fn vote_stuffing_prevented() {
        let mut g = gov();
        let id = g
            .propose(ProposalKind::Text, "t", "d", "x", 200_000, 0)
            .unwrap();
        g.vote(id, "attacker", 1_000_000, VoteChoice::For, 1)
            .unwrap();
        let err = g
            .vote(id, "attacker", 1_000_000, VoteChoice::For, 2)
            .unwrap_err();
        assert_eq!(
            err,
            GovernanceError::AlreadyVoted {
                voter: "attacker".into()
            }
        );
    }

    /// Cannot execute a proposal before the timelock expires.
    #[test]
    fn timelock_prevents_early_execution() {
        let mut g = gov();
        let now = 0u64;
        let id = g
            .propose(ProposalKind::UpdateFees, "t", "d", "x", 200_000, now)
            .unwrap();
        g.vote(id, "a", 25_000_000, VoteChoice::For, 1).unwrap();
        g.vote(id, "b", 20_000_000, VoteChoice::For, 2).unwrap();
        g.finalize(id, now + VOTING_PERIOD_SECS + 1).unwrap();
        g.queue(id).unwrap();

        // Try to execute 1 second before timelock expires
        let too_early = now + VOTING_PERIOD_SECS + TIMELOCK_DELAY_SECS - 1;
        let err = g.execute(id, too_early).unwrap_err();
        assert!(matches!(err, GovernanceError::TimelockNotExpired { .. }));
    }

    /// Cannot vote after the voting period has ended.
    #[test]
    fn vote_after_period_rejected() {
        let mut g = gov();
        let id = g
            .propose(ProposalKind::Text, "t", "d", "x", 200_000, 0)
            .unwrap();
        let err = g
            .vote(
                id,
                "late_voter",
                1_000_000,
                VoteChoice::For,
                VOTING_PERIOD_SECS + 1,
            )
            .unwrap_err();
        assert_eq!(err, GovernanceError::VotingNotActive { id });
    }

    /// Treasury cannot be over-disbursed.
    #[test]
    fn treasury_over_disburse_rejected() {
        let mut g = gov();
        let initial = g.treasury.balance_tokens;
        let err = g
            .treasury
            .disburse("thief", initial + 1, "steal")
            .unwrap_err();
        assert_eq!(
            err,
            GovernanceError::InsufficientTreasuryBalance {
                requested: initial + 1,
                available: initial,
            }
        );
    }
}

// ─── Rate Limit ──────────────────────────────────────────────────────────────

mod rate_limit {
    use relayer::ratelimit::{RateLimiter, Tier, FREE_RPM, PRO_RPM};

    /// Exhausting the free tier and retrying must always return an error —
    /// no way to bypass the bucket by rapid-fire requests.
    #[test]
    fn cannot_exceed_free_tier() {
        let mut rl = RateLimiter::new();
        rl.register("attacker", Tier::Free);

        let mut ok = 0u32;
        let mut rejected = 0u32;
        for _ in 0..(FREE_RPM * 2) {
            match rl.check("attacker") {
                Ok(_) => ok += 1,
                Err(_) => rejected += 1,
            }
        }
        assert_eq!(ok, FREE_RPM, "exactly FREE_RPM requests should succeed");
        assert_eq!(rejected, FREE_RPM, "the rest must be rejected");
    }

    /// Pro tier allows PRO_RPM requests (10× more than free).
    #[test]
    fn pro_tier_allows_more_than_free() {
        let mut rl = RateLimiter::new();
        rl.register("pro", Tier::Pro);

        let mut ok = 0u32;
        for _ in 0..PRO_RPM {
            if rl.check("pro").is_ok() {
                ok += 1;
            }
        }
        assert_eq!(ok, PRO_RPM);
        assert!(rl.check("pro").is_err(), "PRO_RPM+1 must be rejected");
    }

    /// An unknown API key is always rejected.
    #[test]
    fn unknown_key_always_rejected() {
        let mut rl = RateLimiter::new();
        for _ in 0..10 {
            assert!(rl.check("ghost_key").is_err());
        }
    }

    /// Enterprise (unlimited) tier never rejects.
    #[test]
    fn enterprise_unlimited_never_rejected() {
        let mut rl = RateLimiter::new();
        rl.register("enterprise", Tier::Enterprise(0));
        for _ in 0..(PRO_RPM * 10) {
            assert!(rl.check("enterprise").is_ok());
        }
    }
}

// ─── Validator stress: 10 000 pending transactions ───────────────────────────
//
// Testing-infrastructure checklist: "stress test validator with 10k pending txs".
// Verifies that the batch pipeline and multisig bundle layer remain correct and
// non-panicking under a 10 000-event flood.

mod validator_stress {
    use relayer::batch::BatchCollector;
    use relayer::events::{DepositEvent, GatewayEvent};
    use relayer::multisig::{add_signature, create_bundle, verify_bundle, ValidatorId, ValidatorSet};
    use std::time::Duration;

    const TOTAL_EVENTS: u64 = 10_000;
    const BATCH_SIZE: usize = 100;

    fn flood_event(seq: u64) -> GatewayEvent {
        GatewayEvent::Deposit(DepositEvent {
            block_number: seq,
            tx_hash: {
                let mut h = [0u8; 32];
                h[..8].copy_from_slice(&seq.to_le_bytes());
                h
            },
            sequence: seq,
            sender: [0xCC; 20],
            recipient: vec![0xDD; 32],
            amount: 1,
            destination_chain: 2,
            payload_hash: {
                let mut h = [0xEE; 32];
                h[..8].copy_from_slice(&seq.to_le_bytes());
                h
            },
        })
    }

    fn make_validator_set() -> ValidatorSet {
        ValidatorSet::new(
            vec![
                ValidatorId::new(*b"validator_one___________________", 0, "v1"),
                ValidatorId::new(*b"validator_two___________________", 1, "v2"),
                ValidatorId::new(*b"validator_three_________________", 2, "v3"),
            ],
            2,
        )
        .expect("valid set")
    }

    /// Feed 10 000 events through the BatchCollector; every full batch must
    /// contain exactly BATCH_SIZE events and batches must have sequential IDs.
    #[test]
    fn batch_pipeline_handles_10k_events_correctly() {
        let mut collector = BatchCollector::new(BATCH_SIZE, Duration::from_secs(3600));
        let mut flushed_batches = 0usize;
        let mut total_events_in_batches = 0usize;

        for seq in 0..TOTAL_EVENTS {
            if let Some(batch) = collector.push(flood_event(seq)) {
                assert_eq!(
                    batch.len(),
                    BATCH_SIZE,
                    "every size-triggered batch must be exactly BATCH_SIZE"
                );
                assert_eq!(
                    batch.batch_id as usize,
                    flushed_batches,
                    "batches must have sequential IDs"
                );
                total_events_in_batches += batch.len();
                flushed_batches += 1;
            }
        }

        let expected_batches = TOTAL_EVENTS as usize / BATCH_SIZE;
        assert_eq!(
            flushed_batches, expected_batches,
            "expected {expected_batches} full batches from {TOTAL_EVENTS} events"
        );
        assert_eq!(
            total_events_in_batches,
            expected_batches * BATCH_SIZE,
            "total events in batches must match"
        );

        // Remaining events stay in the pending buffer
        let remainder = TOTAL_EVENTS as usize % BATCH_SIZE;
        assert_eq!(
            collector.pending_count(),
            remainder,
            "leftover events must stay in buffer"
        );
    }

    /// With 10 000 pending events the multisig bundle layer must still produce
    /// and verify bundles correctly — no panic, no silent failure.
    #[test]
    fn multisig_bundle_stable_under_flood() {
        let vset = make_validator_set();

        // Create and sign 1 000 bundles (representative sample of the 10k flood)
        let mut success = 0usize;
        for seq in 0u64..1_000 {
            let payload = format!("flood_payload_{seq}");
            let mut bundle = create_bundle(payload.as_bytes(), seq, 1, &[0xAA; 32], &vset);

            add_signature(&mut bundle, 0, [0x01u8; 64], &vset, seq).unwrap();
            add_signature(&mut bundle, 1, [0x02u8; 64], &vset, seq).unwrap();

            if verify_bundle(&bundle, &vset).is_ok() {
                success += 1;
            }
        }

        assert_eq!(
            success, 1_000,
            "all 1 000 representative bundles must verify (success={success})"
        );
    }
}

// ─── Webhook DoS ──────────────────────────────────────────────────────────────

mod webhook_dos {
    use relayer::webhook::{EventType, WebhookRegistry};

    /// After 10 consecutive delivery failures the webhook is auto-disabled,
    /// so it never appears in active subscribers.
    #[test]
    fn auto_disabled_after_ten_failures() {
        let reg = WebhookRegistry::new();
        let registration = reg.register(
            "https://attacker.example.com/hook".to_string(),
            vec![EventType::All],
        );

        // Simulate 10 consecutive failures via the public record_delivery API
        for _ in 0..10 {
            reg.record_delivery(&registration.id, false);
        }

        let updated = reg.get(&registration.id).unwrap();
        assert!(
            !updated.active,
            "webhook must be disabled after 10 consecutive failures"
        );
        assert_eq!(reg.active_count(), 0);
    }

    /// A single success resets the consecutive failure counter.
    #[test]
    fn success_resets_failure_counter() {
        let reg = WebhookRegistry::new();
        let registration =
            reg.register("https://example.com/hook".to_string(), vec![EventType::All]);

        // 9 failures (not yet disabled)
        for _ in 0..9 {
            reg.record_delivery(&registration.id, false);
        }
        // 1 success — should reset
        reg.record_delivery(&registration.id, true);

        let updated = reg.get(&registration.id).unwrap();
        assert!(updated.active, "9 failures + 1 success must remain active");
        assert_eq!(updated.consecutive_failures, 0);
    }

    /// Deregistered webhooks must never appear in subscribers.
    #[test]
    fn deregistered_webhook_not_dispatched() {
        let reg = WebhookRegistry::new();
        let registration =
            reg.register("https://example.com/hook".to_string(), vec![EventType::All]);
        reg.deregister(&registration.id);
        let subs = reg.subscribers_for(&EventType::All);
        assert!(subs.is_empty(), "deregistered webhook must not be returned");
    }
}
