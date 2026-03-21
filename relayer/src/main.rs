use relayer::batch::BatchCollector;
use relayer::events::GatewayEvent;
use relayer::fee;
use relayer::finality::{wait_for_finality, wait_for_finality_ws};
use relayer::http_api;
use relayer::listener::{EventListener, ListenerConfig};
use relayer::metrics::Metrics;
use relayer::prover::ProverEngine;
use relayer::submitter::{ProofSubmitter, SubmitterConfig};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Semaphore};
use tracing::{error, info, warn};

/// Flush a batch when this many events accumulate (target: beat Wormhole's 1-20 per VAA)
const BATCH_MAX_SIZE: usize = 100;
/// Flush a batch after this many seconds regardless of size
const BATCH_FLUSH_SECS: u64 = 5;

/// Alert threshold: proof generation taking longer than this is suspicious
const PROOF_GEN_ALERT_MS: u128 = 1_000;
/// Alert threshold: total settlement finality time
const SETTLEMENT_ALERT_MS: u128 = 60_000;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // JSON structured logging for Datadog/Splunk compatibility (Phase 10)
    // Set RUST_LOG=debug for verbose output, default to info
    let log_format = std::env::var("LOG_FORMAT").unwrap_or_else(|_| "json".to_string());
    if log_format == "json" {
        tracing_subscriber::fmt()
            .json()
            .with_current_span(false)
            .with_env_filter(
                tracing_subscriber::EnvFilter::from_default_env()
                    .add_directive("relayer=info".parse().unwrap()),
            )
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::from_default_env()
                    .add_directive("relayer=info".parse().unwrap()),
            )
            .init();
    }

    // Load config from environment or use dev defaults.
    let chain_id: u64 = std::env::var("CHAIN_ID")
        .unwrap_or_else(|_| "1".to_string())
        .parse()
        .unwrap_or(1);
    let ws_rpc_url = std::env::var("EVM_WS_RPC_URL")
        .or_else(|_| std::env::var("EVM_RPC_URL"))
        .unwrap_or_else(|_| "ws://localhost:8545".to_string());
    let http_rpc_url =
        std::env::var("EVM_HTTP_RPC_URL").unwrap_or_else(|_| "http://localhost:8545".to_string());
    let solana_rpc_url = std::env::var("SOLANA_RPC_URL")
        .unwrap_or_else(|_| "https://api.devnet.solana.com".to_string());
    let gateway_address = std::env::var("GATEWAY_ADDRESS")
        .unwrap_or_else(|_| "0x0000000000000000000000000000000000000000".to_string());
    let program_id = std::env::var("HUB_PROGRAM_ID")
        .unwrap_or_else(|_| "AKzpc9tvxfhLjj5AantKizK2YZgSjoyhLmYqRZy6b8Lz".to_string());
    let keypair_path =
        std::env::var("KEYPAIR_PATH").unwrap_or_else(|_| "~/.config/solana/id.json".to_string());

    // Limit concurrent proof generation to CPU count
    let max_concurrent_provers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    // HTTP API server address (Phase 5 / Phase 10)
    let api_addr = std::env::var("API_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());

    // Shared metrics (Phase 10)
    let metrics = Metrics::new();

    info!(
        chain_id,
        max_concurrent_provers,
        batch_max_size = BATCH_MAX_SIZE,
        batch_flush_secs = BATCH_FLUSH_SECS,
        api_addr,
        "interlink relayer starting"
    );

    // Initialize prover engine (Groth16 trusted setup — once at startup)
    let prover = ProverEngine::new(6);
    info!("initializing groth16 prover (trusted setup)...");
    prover.initialize().await.map_err(|e| {
        error!(error = %e, "prover initialization failed");
        e
    })?;
    info!("prover ready");

    // Configure listener
    let listener_config = ListenerConfig {
        ws_rpc_url: ws_rpc_url.clone(),
        gateway_address,
        chain_id,
        max_reconnect_attempts: 10,
    };

    // Configure submitter
    let submitter_config = SubmitterConfig {
        rpc_url: solana_rpc_url.clone(),
        program_id,
        keypair_path,
        max_retries: 3,
        source_chain_id: chain_id,
    };

    // Spawn HTTP API server (Phase 5 / Phase 10): /health, /quote, /compare, /metrics
    let api_metrics = metrics.clone();
    let api_addr_clone = api_addr.clone();
    tokio::spawn(async move {
        http_api::serve(&api_addr_clone, api_metrics).await;
    });

    // Channel: listener → batch pipeline. Buffer 1024 so listener never blocks.
    let (tx, mut rx) = mpsc::channel::<GatewayEvent>(1024);

    // Spawn listener
    let mut listener = EventListener::new(listener_config, tx);
    let listener_handle = tokio::spawn(async move {
        if let Err(e) = listener.run().await {
            error!(error = %e, "listener exited with error");
        }
    });

    let submitter = ProofSubmitter::new(submitter_config);
    let semaphore = Arc::new(Semaphore::new(max_concurrent_provers));

    // Batch pipeline: collect events into time-bounded batches, then process concurrently.
    //
    // Old serial pipeline: 1 event → finality → proof → submit → next event
    // New batch pipeline:  N events collected over 5s → all finality waits in parallel
    //                      → all proofs in parallel (bounded) → submit all
    //
    // This directly beats Wormhole (1-20 per VAA) and Stargate (sequential settlement).
    let pipeline_metrics = metrics.clone();
    let processing_handle = tokio::spawn(async move {
        let mut collector =
            BatchCollector::new(BATCH_MAX_SIZE, Duration::from_secs(BATCH_FLUSH_SECS));
        let mut flush_tick = tokio::time::interval(Duration::from_secs(BATCH_FLUSH_SECS));
        flush_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Consume the immediate first tick so we don't flush an empty batch at t=0
        flush_tick.tick().await;

        loop {
            let batch_opt = tokio::select! {
                event_opt = rx.recv() => {
                    match event_opt {
                        Some(event) => {
                            let seq = event.sequence();
                            info!(sequence = seq, "event received, buffering in batch");
                            // Track queue depth for Phase 10 alerting
                            pipeline_metrics.set_queue_depth(collector.pending_count() + 1);
                            collector.push(event)
                        }
                        None => {
                            // Channel closed — flush remaining and exit
                            info!("event channel closed, flushing remaining events");
                            let _ = collector.flush_timer();
                            break;
                        }
                    }
                }
                _ = flush_tick.tick() => {
                    collector.flush_timer()
                }
            };

            if let Some(batch) = batch_opt {
                let batch_id = batch.batch_id;
                let batch_size = batch.len();
                info!(
                    batch_id,
                    batch_size, "dispatching batch to concurrent pipeline"
                );

                // Record batch metrics (Phase 10)
                pipeline_metrics.record_batch_flushed(batch_size);
                pipeline_metrics.set_queue_depth(0);

                // Spawn a task per event in the batch — all run concurrently.
                // Semaphore ensures we don't over-saturate CPU with provers.
                for event in batch.events {
                    let permit = semaphore
                        .clone()
                        .acquire_owned()
                        .await
                        .expect("semaphore closed");
                    let prover = prover.clone();
                    let submitter = submitter.clone();
                    let ws_url = ws_rpc_url.clone();
                    let http_url = http_rpc_url.clone();
                    let event_metrics = pipeline_metrics.clone();

                    tokio::spawn(async move {
                        let _permit = permit;
                        let sequence = event.sequence();
                        let settlement_start = std::time::Instant::now();

                        // Log fee tier for this transfer
                        let fee_desc = fee::FeeTier::from_usd_cents(
                            // Without oracle we can't know USD value, log at Standard tier boundary
                            100_000, // placeholder: treat as $1k for logging
                        )
                        .describe();
                        info!(batch_id, sequence, fee_tier = fee_desc, "processing event");

                        // Phase 1: Wait for finality via WebSocket (beats Wormhole 2-15min)
                        let finality_result =
                            if ws_url.starts_with("ws://") || ws_url.starts_with("wss://") {
                                wait_for_finality_ws(chain_id, event.block_number(), &ws_url).await
                            } else {
                                wait_for_finality(chain_id, event.block_number(), &http_url).await
                            };

                        match finality_result {
                            Ok(()) => info!(batch_id, sequence, "finality confirmed"),
                            Err(e) => {
                                error!(batch_id, sequence, error = %e, "finality failed, skipping");
                                event_metrics.record_settlement_failure();
                                return;
                            }
                        }

                        event_metrics.record_settlement_start();

                        // Phase 2: Generate ZK proof
                        let proof_start = std::time::Instant::now();
                        event_metrics.record_proof_start();
                        match prover.generate_proof(&event).await {
                            Ok(package) => {
                                let proof_ms = proof_start.elapsed().as_millis();
                                event_metrics.record_proof_success(proof_ms as u64);

                                // Alerting threshold: proof gen > 1s is a warning (Phase 10)
                                if proof_ms > PROOF_GEN_ALERT_MS {
                                    warn!(
                                        batch_id,
                                        sequence,
                                        proof_ms,
                                        threshold_ms = PROOF_GEN_ALERT_MS,
                                        "ALERT: proof generation exceeded threshold"
                                    );
                                } else {
                                    info!(
                                        batch_id,
                                        sequence,
                                        proof_ms,
                                        proof_size = package.proof_bytes.len(),
                                        "proof generated"
                                    );
                                }

                                // Phase 3: Submit to Solana hub
                                match submitter.submit(&package).await {
                                    Ok(sig) => {
                                        let settlement_ms = settlement_start.elapsed().as_millis();
                                        event_metrics
                                            .record_settlement_success(settlement_ms as u64);

                                        // Alerting threshold: total settlement > 60s (Phase 10)
                                        if settlement_ms > SETTLEMENT_ALERT_MS {
                                            warn!(
                                                batch_id,
                                                sequence,
                                                settlement_ms,
                                                threshold_ms = SETTLEMENT_ALERT_MS,
                                                "ALERT: settlement time exceeded SLA threshold"
                                            );
                                        } else {
                                            info!(
                                                batch_id,
                                                sequence,
                                                settlement_ms,
                                                signature = %sig,
                                                "settlement complete"
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        error!(batch_id, sequence, error = %e, "submission failed");
                                        event_metrics.record_settlement_failure();
                                    }
                                }
                            }
                            Err(e) => {
                                error!(batch_id, sequence, error = %e, "proof generation failed");
                                event_metrics.record_proof_failure();
                                event_metrics.record_settlement_failure();
                            }
                        }
                    });
                }
            }
        }

        info!("processing pipeline stopped");
    });

    info!(
        chain_id,
        max_concurrent_provers,
        batch_max_size = BATCH_MAX_SIZE,
        batch_flush_secs = BATCH_FLUSH_SECS,
        "relayer started — batch pipeline active"
    );

    tokio::select! {
        _ = listener_handle => error!("listener task exited unexpectedly"),
        _ = processing_handle => error!("processing task exited unexpectedly"),
    }

    Ok(())
}
