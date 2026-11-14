use relayer::batch::BatchCollector;
use relayer::events::{GatewayEvent, ACTION_SWAP, ACTION_TRANSFER, HUB_CHAIN_ID};
use relayer::evm_submitter::{EvmSubmitter, EvmSubmitterConfig, ReleaseOutcome};
use relayer::finality::{wait_for_finalized_inclusion, InclusionError};
use relayer::http_api;
use relayer::hub_outbound::{HubOutboundConfig, HubOutboundListener, OutboundEvent};
use relayer::listener::{EventListener, ListenerConfig};
use relayer::metrics::Metrics;
use relayer::prover::ProverEngine;
use relayer::submitter::{HubOutcome, ProofSubmitter, SubmitterConfig};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Semaphore};
use tracing::{error, info, warn};

/// Flush a batch when this many events accumulate
const BATCH_MAX_SIZE: usize = 100;
/// Flush a batch after this many seconds regardless of size
const BATCH_FLUSH_SECS: u64 = 5;

/// Alert threshold: proof generation taking longer than this is suspicious
const PROOF_GEN_ALERT_MS: u128 = 1_000;
/// Alert threshold: total settlement time (Ethereum finality alone is ~13 min)
const SETTLEMENT_ALERT_MS: u128 = 30 * 60_000;

/// How often to poll for the finalized head, and how long to wait at most.
const FINALITY_POLL: Duration = Duration::from_secs(12);
const FINALITY_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// Attempts per event for transient failures (RPC, submission) before giving up.
const MAX_EVENT_ATTEMPTS: u32 = 8;

/// Log retention period in days
const LOG_RETENTION_DAYS: u64 = 30;

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Load the EVM signing key for hub → EVM delivery, from EVM_PRIVATE_KEY_FILE
/// (preferred) or EVM_PRIVATE_KEY. None disables that direction.
fn load_evm_key() -> Result<Option<k256::ecdsa::SigningKey>, String> {
    let hex = match (
        std::env::var("EVM_PRIVATE_KEY_FILE"),
        std::env::var("EVM_PRIVATE_KEY"),
    ) {
        (Ok(path), _) => std::fs::read_to_string(&path)
            .map_err(|e| format!("EVM_PRIVATE_KEY_FILE {path}: {e}"))?,
        (Err(_), Ok(hex)) => hex,
        _ => return Ok(None),
    };
    relayer::evm_tx::signing_key_from_hex(&hex).map(Some)
}

fn backoff_step(backoff: &mut Duration) {
    *backoff = (*backoff * 2).min(Duration::from_secs(300));
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // JSON structured logging. Set RUST_LOG=debug for verbose output.
    let filter = tracing_subscriber::EnvFilter::from_default_env()
        .add_directive("relayer=info".parse().unwrap());
    if env_or("LOG_FORMAT", "json") == "json" {
        tracing_subscriber::fmt()
            .json()
            .with_current_span(false)
            .with_env_filter(filter)
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }

    // EIP-155 chain ID of the EVM chain. Verified against the RPC by the listener.
    let chain_id: u64 = env_or("CHAIN_ID", "1")
        .parse()
        .map_err(|e| format!("CHAIN_ID: {e}"))?;
    let http_rpc_url = std::env::var("EVM_HTTP_RPC_URL")
        .or_else(|_| std::env::var("EVM_RPC_URL"))
        .unwrap_or_else(|_| "http://localhost:8545".to_string());
    let solana_rpc_url = env_or("SOLANA_RPC_URL", "https://api.devnet.solana.com");
    let gateway_address = env_or(
        "GATEWAY_ADDRESS",
        "0x0000000000000000000000000000000000000000",
    );
    let program_id = env_or(
        "HUB_PROGRAM_ID",
        "AKzpc9tvxfhLjj5AantKizK2YZgSjoyhLmYqRZy6b8Lz",
    );
    let keypair_path = env_or("KEYPAIR_PATH", "~/.config/solana/id.json");
    let proving_key_path = env_or("PROVING_KEY_PATH", "keys/interlink_pk.bin");
    let allow_dev_setup = env_or("ALLOW_DEV_SETUP", "false") == "true";
    let start_block = std::env::var("START_BLOCK")
        .ok()
        .and_then(|s| s.parse().ok());
    let api_addr = env_or("API_ADDR", "127.0.0.1:8080");

    let hub_program =
        relayer::solana_tx::pubkey(&program_id).map_err(|e| format!("HUB_PROGRAM_ID: {e}"))?;
    let gateway =
        relayer::evm::hex20(&gateway_address).map_err(|e| format!("GATEWAY_ADDRESS: {e}"))?;
    let evm_key = load_evm_key()?;

    let max_concurrent_provers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let metrics = Metrics::new();

    info!(
        chain_id,
        max_concurrent_provers,
        batch_max_size = BATCH_MAX_SIZE,
        batch_flush_secs = BATCH_FLUSH_SECS,
        log_retention_days = LOG_RETENTION_DAYS,
        api_addr,
        hub_to_evm = evm_key.is_some(),
        "interlink relayer starting"
    );

    // Load the persistent proving key. The VK registered on-chain must come
    // from this same file (`cargo run --bin export-vk`).
    let prover = ProverEngine::new(6);
    prover
        .load_or_create(std::path::Path::new(&proving_key_path), allow_dev_setup)
        .await
        .map_err(|e| {
            error!(error = %e, "prover initialization failed");
            e
        })?;
    info!("prover ready");

    let listener_config = ListenerConfig {
        rpc_url: http_rpc_url.clone(),
        gateway_address,
        chain_id,
        max_consecutive_errors: 30,
        start_block,
        poll_interval: Duration::from_secs(env_or("POLL_INTERVAL_SECS", "4").parse().unwrap_or(4)),
        max_block_range: env_or("MAX_BLOCK_RANGE", "2000").parse().unwrap_or(2000),
    };
    let submitter = ProofSubmitter::new(SubmitterConfig {
        rpc_url: solana_rpc_url.clone(),
        program_id,
        keypair_path,
        max_retries: 3,
        source_chain_id: chain_id,
    });

    // HTTP API (/health, /quote, /compare, /metrics). Binds to localhost by
    // default; set API_ADDR=0.0.0.0:8080 deliberately to expose it.
    let api_metrics = metrics.clone();
    let api_addr_clone = api_addr.clone();
    tokio::spawn(async move {
        http_api::serve(&api_addr_clone, api_metrics).await;
    });

    // Bounds concurrent *proving* only. Finality waits do not hold a permit.
    let prover_slots = Arc::new(Semaphore::new(max_concurrent_provers));

    // ── Hub → EVM (withdrawals), only if an EVM key is configured ─────────────
    if let Some(key) = evm_key {
        let evm = EvmSubmitter::new(
            EvmSubmitterConfig {
                rpc_url: http_rpc_url.clone(),
                chain_id,
                gateway,
            },
            key,
        );
        info!(address = %relayer::evm::to_hex(&evm.address()), "hub → EVM delivery enabled");
        let (out_tx, mut out_rx) = mpsc::channel::<OutboundEvent>(1024);
        let mut hub_listener = HubOutboundListener::new(
            HubOutboundConfig {
                rpc_url: solana_rpc_url.clone(),
                program_id: hub_program,
                evm_chain_id: chain_id,
                evm_gateway: gateway,
                poll_interval: Duration::from_secs(
                    env_or("HUB_POLL_INTERVAL_SECS", "5").parse().unwrap_or(5),
                ),
                start_after_signature: std::env::var("HUB_START_SIGNATURE").ok(),
            },
            out_tx,
        );
        tokio::spawn(async move {
            if let Err(e) = hub_listener.run().await {
                error!(error = %e, "hub outbound listener exited");
            }
        });
        let prover = prover.clone();
        let slots = prover_slots.clone();
        let m = metrics.clone();
        tokio::spawn(async move {
            while let Some(ev) = out_rx.recv().await {
                let (prover, evm, slots, m) =
                    (prover.clone(), evm.clone(), slots.clone(), m.clone());
                tokio::spawn(
                    async move { deliver_withdrawal(ev, &prover, &evm, &slots, &m).await },
                );
            }
        });
    } else {
        warn!("EVM_PRIVATE_KEY(_FILE) not set: hub → EVM withdrawals will not be delivered");
    }

    // ── EVM → hub (deposits) ──────────────────────────────────────────────────
    let (tx, mut rx) = mpsc::channel::<GatewayEvent>(1024);
    let mut listener = EventListener::new(listener_config, tx);
    let listener_handle = tokio::spawn(async move {
        if let Err(e) = listener.run().await {
            error!(error = %e, "listener exited with error");
        }
    });

    let pipeline_metrics = metrics.clone();
    let processing_handle = tokio::spawn(async move {
        let mut collector =
            BatchCollector::new(BATCH_MAX_SIZE, Duration::from_secs(BATCH_FLUSH_SECS));
        let mut flush_tick = tokio::time::interval(Duration::from_secs(BATCH_FLUSH_SECS));
        flush_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        flush_tick.tick().await;

        loop {
            let batch_opt = tokio::select! {
                event_opt = rx.recv() => match event_opt {
                    Some(event) => {
                        pipeline_metrics.set_queue_depth(collector.pending_count() + 1);
                        collector.push(event)
                    }
                    None => {
                        info!("event channel closed, flushing remaining events");
                        let _ = collector.flush_timer();
                        break;
                    }
                },
                _ = flush_tick.tick() => collector.flush_timer(),
            };

            let Some(batch) = batch_opt else { continue };
            let batch_id = batch.batch_id;
            info!(batch_id, batch_size = batch.len(), "dispatching batch");
            pipeline_metrics.record_batch_flushed(batch.len());
            pipeline_metrics.set_queue_depth(0);

            for event in batch.events {
                let prover = prover.clone();
                let submitter = submitter.clone();
                let http_url = http_rpc_url.clone();
                let slots = prover_slots.clone();
                let m = pipeline_metrics.clone();
                tokio::spawn(async move {
                    process_deposit(
                        batch_id,
                        event,
                        hub_program,
                        &http_url,
                        &prover,
                        &submitter,
                        &slots,
                        &m,
                    )
                    .await;
                });
            }
        }
        info!("processing pipeline stopped");
    });

    info!(chain_id, "relayer started");

    tokio::select! {
        _ = listener_handle => error!("listener task exited unexpectedly"),
        _ = processing_handle => error!("processing task exited unexpectedly"),
    }
    Ok(())
}

/// EVM → hub: wait for finality, prove, attest, then execute the message.
#[allow(clippy::too_many_arguments)]
async fn process_deposit(
    batch_id: u64,
    event: GatewayEvent,
    hub_program: [u8; 32],
    http_url: &str,
    prover: &ProverEngine,
    submitter: &ProofSubmitter,
    prover_slots: &Semaphore,
    metrics: &Metrics,
) {
    let sequence = event.sequence();

    let (Some(message), GatewayEvent::Message(raw)) = (event.verified_message(), &event) else {
        error!(
            batch_id,
            sequence, "event fields do not reproduce its payload hash; refusing to relay"
        );
        metrics.record_settlement_failure();
        return;
    };
    if message.dst_chain != HUB_CHAIN_ID || message.dst_gateway != hub_program {
        info!(
            batch_id,
            sequence,
            dst_chain = message.dst_chain,
            "message not addressed to this hub; skipping"
        );
        return;
    }

    let started = std::time::Instant::now();
    let mut backoff = Duration::from_secs(5);

    for attempt in 1..=MAX_EVENT_ATTEMPTS {
        // 1. Finality + canonical inclusion (no prover permit held).
        match wait_for_finalized_inclusion(http_url, raw, FINALITY_POLL, FINALITY_TIMEOUT).await {
            Ok(()) => {}
            Err(InclusionError::NotCanonical(reason)) => {
                error!(batch_id, sequence, %reason, "event is not canonical; dropping (never retried)");
                metrics.record_settlement_failure();
                return;
            }
            Err(InclusionError::Transient(reason)) => {
                warn!(batch_id, sequence, attempt, %reason, "finality check failed; will retry");
                tokio::time::sleep(backoff).await;
                backoff_step(&mut backoff);
                continue;
            }
        }
        metrics.record_settlement_start();

        // 2. Prove (bounded concurrency).
        let package = {
            let _permit = prover_slots.acquire().await.expect("semaphore closed");
            let t = std::time::Instant::now();
            metrics.record_proof_start();
            match prover.generate_proof(&event).await {
                Ok(p) => {
                    let ms = t.elapsed().as_millis();
                    metrics.record_proof_success(ms as u64);
                    if ms > PROOF_GEN_ALERT_MS {
                        warn!(
                            batch_id,
                            sequence,
                            proof_ms = ms,
                            "ALERT: proof generation exceeded threshold"
                        );
                    }
                    p
                }
                Err(e) => {
                    // Deterministic (bad witness/hash): retrying will not help.
                    error!(batch_id, sequence, error = %e, "proof generation failed");
                    metrics.record_proof_failure();
                    metrics.record_settlement_failure();
                    return;
                }
            }
        };

        // 3. Attest on the hub.
        if let Err(e) = submitter.submit(&package).await {
            warn!(batch_id, sequence, attempt, error = %e, "attestation failed; will retry");
            tokio::time::sleep(backoff).await;
            backoff_step(&mut backoff);
            continue;
        }

        // 4. Execute (mint / swap). Permissionless on-chain, so a failure here
        //    (e.g. slippage, unregistered asset) can be retried by anyone later.
        if matches!(message.action, ACTION_TRANSFER | ACTION_SWAP) {
            match submitter.execute(message, &raw.data).await {
                Ok(outcome) => {
                    let ms = started.elapsed().as_millis();
                    metrics.record_settlement_success(ms as u64);
                    if ms > SETTLEMENT_ALERT_MS {
                        warn!(
                            batch_id,
                            sequence,
                            settlement_ms = ms,
                            "ALERT: settlement exceeded threshold"
                        );
                    }
                    let sig = match outcome {
                        HubOutcome::Confirmed(sig) => sig,
                        HubOutcome::AlreadyDone => "already-executed".into(),
                    };
                    info!(batch_id, sequence, settlement_ms = ms, signature = %sig, "settlement complete");
                }
                Err(e) => {
                    warn!(batch_id, sequence, error = %e, "attested but execution failed; anyone can retry execute_*");
                    metrics.record_settlement_failure();
                }
            }
        } else {
            info!(
                batch_id,
                sequence,
                action = message.action,
                "attested (no hub executor for this action)"
            );
        }
        return;
    }

    error!(
        batch_id,
        sequence,
        attempts = MAX_EVENT_ATTEMPTS,
        "ALERT: giving up on event; manual replay required (set START_BLOCK to re-scan)"
    );
    metrics.record_settlement_failure();
}

/// Hub → EVM: prove a finalized withdrawal and release it on the gateway.
async fn deliver_withdrawal(
    ev: OutboundEvent,
    prover: &ProverEngine,
    evm: &EvmSubmitter,
    prover_slots: &Semaphore,
    metrics: &Metrics,
) {
    let nonce = ev.message.nonce;
    let package = {
        let _permit = prover_slots.acquire().await.expect("semaphore closed");
        match prover.prove_message(&ev.message).await {
            Ok(p) => p,
            Err(e) => {
                error!(nonce, sig = %ev.signature, error = %e, "withdrawal proof failed");
                metrics.record_proof_failure();
                return;
            }
        }
    };

    let mut backoff = Duration::from_secs(5);
    for attempt in 1..=MAX_EVENT_ATTEMPTS {
        match evm.release(&ev.message, &package.proof_bytes).await {
            Ok(ReleaseOutcome::Confirmed(tx)) => {
                info!(nonce, sig = %ev.signature, tx = %tx, "withdrawal released on EVM");
                return;
            }
            Ok(ReleaseOutcome::AlreadyExecuted) => {
                info!(nonce, sig = %ev.signature, "withdrawal already released");
                return;
            }
            Err(e) => {
                warn!(nonce, attempt, error = %e, "releaseTokens failed; will retry");
                tokio::time::sleep(backoff).await;
                backoff_step(&mut backoff);
            }
        }
    }
    error!(nonce, sig = %ev.signature, "ALERT: giving up on withdrawal; manual release required");
}
