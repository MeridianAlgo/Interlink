use relayer::events::GatewayEvent;
use relayer::finality::wait_for_finality;
use relayer::listener::{EventListener, ListenerConfig};
use relayer::prover::ProverEngine;
use relayer::submitter::{ProofSubmitter, SubmitterConfig};
use tokio::sync::mpsc;
use tracing::{error, info};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    // Load config from environment or use dev defaults.
    let chain_id: u64 = std::env::var("CHAIN_ID")
        .unwrap_or_else(|_| "1".to_string())
        .parse()
        .unwrap_or(1);
    let ws_rpc_url = std::env::var("EVM_RPC_URL")
        .unwrap_or_else(|_| "ws://localhost:8545".to_string());
    let solana_rpc_url = std::env::var("SOLANA_RPC_URL")
        .unwrap_or_else(|_| "https://api.devnet.solana.com".to_string());
    let gateway_address = std::env::var("GATEWAY_ADDRESS")
        .unwrap_or_else(|_| "0x0000000000000000000000000000000000000000".to_string());
    let program_id = std::env::var("HUB_PROGRAM_ID")
        .unwrap_or_else(|_| "AKzpc9tvxfhLjj5AantKizK2YZgSjoyhLmYqRZy6b8Lz".to_string());
    let keypair_path = std::env::var("KEYPAIR_PATH")
        .unwrap_or_else(|_| "~/.config/solana/id.json".to_string());

    // Initialize prover engine (generates VK/PK once at startup)
    let prover = ProverEngine::new(6);
    info!("initializing prover engine...");
    prover.initialize().await.map_err(|e| {
        error!(error = %e, "prover initialization failed");
        e
    })?;
    info!("prover engine ready");

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

    // Channel between listener and processing pipeline
    let (tx, mut rx) = mpsc::channel::<GatewayEvent>(1024);

    // Spawn listener task
    let mut listener = EventListener::new(listener_config, tx);
    let listener_handle = tokio::spawn(async move {
        if let Err(e) = listener.run().await {
            error!(error = %e, "listener exited with error");
        }
    });

    // Spawn processing pipeline
    let submitter = ProofSubmitter::new(submitter_config);
    let processing_handle = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            let sequence = event.sequence();
            info!(sequence, "processing gateway event");

            // Wait for source chain finality
            match wait_for_finality(chain_id, event.block_number(), &ws_rpc_url).await {
                Ok(()) => info!(sequence, "finality confirmed"),
                Err(e) => {
                    error!(sequence, error = %e, "finality check failed, skipping");
                    continue;
                }
            }

            // Generate ZK proof
            match prover.generate_proof(&event).await {
                Ok(package) => {
                    info!(
                        sequence,
                        proof_size = package.proof_bytes.len(),
                        "proof generated"
                    );

                    // Submit to Solana hub
                    match submitter.submit(&package).await {
                        Ok(sig) => info!(sequence, signature = %sig, "submitted to hub"),
                        Err(e) => error!(sequence, error = %e, "submission failed"),
                    }
                }
                Err(e) => {
                    error!(sequence, error = %e, "proof generation failed");
                }
            }
        }
    });

    info!("relayer started, listening for gateway events");

    // Wait for either task to complete (they should run forever)
    tokio::select! {
        _ = listener_handle => error!("listener task exited unexpectedly"),
        _ = processing_handle => error!("processing task exited unexpectedly"),
    }

    Ok(())
}
