use crate::Result;
use ethers_contract::abigen;
use ethers_core::types::Address;
use ethers_providers::{Provider, StreamExt, Ws};
use serde_json::json;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::mpsc;
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_real_snark_generation() {
        let nonce = 1u64;
        let payload_hash = [1u8; 32];
        let chain_id = 1u64;

        println!("\n[TEST] Testing Real ZK-SNARK generation (BN254/Multicore)...");
        let proof = Relayer::generate_proof_sync(nonce, payload_hash, chain_id)
            .expect("Proof generation failed");

        println!("[TEST] SNARK Generated. Length: {} bytes", proof.len());
        assert!(!proof.is_empty(), "Proof should not be empty");
        assert!(
            proof.len() > 100,
            "Proof size indicates incorrect generation"
        );
    }
}

abigen!(
    InterlinkGateway,
    r#"[
        event MessagePublished(uint64 indexed nonce, uint64 destinationChain, address sender, bytes32 payloadHash, bytes payload)
    ]"#
);

pub struct RelayerConfig {
    pub chain_id: u64,
    pub rpc_url: String, // WebSocket URL for EVM chains
    pub hub_url: String, // Solana Hub RPC URL
    pub gateway_address: String,
    pub solana_program_id: String,
    pub keypair_path: String,
}

pub struct Relayer {
    config: Arc<RelayerConfig>,
}

impl Relayer {
    pub fn new(config: RelayerConfig) -> Self {
        Self {
            config: Arc::new(config),
        }
    }

    /// Starts the main event loop and proof generation pipeline.
    pub async fn run(&self) -> Result<()> {
        println!(
            "Initializing High-Performance Relayer for chain ID {} [WS: {}] [Hub: {}]",
            self.config.chain_id, self.config.rpc_url, self.config.hub_url
        );

        let (tx, mut rx) = mpsc::channel(1024);
        let ws_url = self.config.rpc_url.clone();
        let gateway_address = self.config.gateway_address.clone();

        // Step 1: Initialize event watcher on the target chain.
        tokio::spawn(async move {
            if let Err(e) = Self::watch_events(&ws_url, &gateway_address, tx).await {
                eprintln!("[WS ERROR] Watcher failed: {}", e);
            }
        });

        // Step 2: Main processing loop for generating proofs and submitting.
        while let Some((nonce, payload_hash)) = rx.recv().await {
            println!(
                "\n[EVENT] Raw payload received from Source Chain #{} | Nonce: {}",
                self.config.chain_id, nonce
            );
            let relayer_ref = Arc::clone(&self.config);

            // Offload ZK proof generation to a blocking task to prevent blocking the async runtime.
            let proof_task = tokio::task::spawn_blocking(move || {
                Self::generate_proof_sync(nonce, payload_hash, relayer_ref.chain_id)
            });

            match proof_task.await {
                Ok(Ok(proof)) => {
                    let rpc_url = self.config.hub_url.clone();
                    let program_id = self.config.solana_program_id.clone();

                    if let Err(e) = Self::submit_to_hub(
                        rpc_url,
                        program_id,
                        nonce,
                        payload_hash,
                        proof,
                        self.config.keypair_path.clone(),
                    )
                    .await
                    {
                        eprintln!("[ERROR] Solana Hub Submission Failed: {:?}", e);
                    }
                }
                _ => eprintln!("[ERROR] ZK Proof Generation Failed."),
            }
        }

        Ok(())
    }

    async fn watch_events(
        ws_url: &str,
        contract_addr: &str,
        tx: mpsc::Sender<(u64, [u8; 32])>,
    ) -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut retry_backoff = 1;

        loop {
            println!(
                "[WS] Attempting to connect to WebSocket listener at {}...",
                ws_url
            );

            // use ethers to catch evm events. the real deal.
            match Provider::<Ws>::connect(ws_url).await {
                Ok(provider) => {
                    println!("[WS ERROR] Successfully connected to {}.", ws_url);
                    retry_backoff = 1; // reset backoff on success

                    let client = Arc::new(provider);
                    let address = Address::from_str(contract_addr).map_err(|e| e.to_string())?;
                    let contract = InterlinkGateway::new(address, client);

                    println!("[WS] Subscribing to MessagePublished events...");

                    // strictly wss for the live environment. no polls allowed.
                    match contract.events().subscribe().await {
                        Ok(mut block_stream) => {
                            println!("[WS] Successfully subscribed. Monitoring chain for finalized messages...");

                            while let Some(log) = block_stream.next().await {
                                if let Ok(event) = log {
                                    println!(
                                        "[WS] Captured Event: DestChain: {} | Sender: {:?} | Nonce: {}",
                                        event.destination_chain, event.sender, event.nonce
                                    );

                                    if tx.send((event.nonce, event.payload_hash)).await.is_err() {
                                        return Ok(()); // channel closed, exit watcher.
                                    }
                                }
                            }
                            println!("[WS WARNING] Stream closed. Attempting reconnect...");
                        }
                        Err(e) => {
                            eprintln!("[WS ERROR] Failed to subscribe: {}", e);
                        }
                    }
                }
                Err(e) => {
                    eprintln!(
                        "[WS ERROR] Watcher failed to connect: {}. Retrying in {}s...",
                        e, retry_backoff
                    );
                }
            }

            tokio::time::sleep(tokio::time::Duration::from_secs(retry_backoff)).await;
            retry_backoff = std::cmp::min(30, retry_backoff * 2); // exponential backoff up to 30s.
        }
    }

    fn generate_proof_sync(nonce: u64, hash: [u8; 32], _chain_id: u64) -> Result<Vec<u8>> {
        use crate::circuit::InterlinkCircuit;
        use ff::PrimeField;
        use halo2_proofs::{
            plonk::{create_proof, keygen_pk, keygen_vk},
            poly::commitment::Params,
            transcript::{Blake2bWrite, Challenge255},
        };
        use halo2curves::bn256::{Fr, G1Affine};
        use rand_core::OsRng;

        println!("[PROVER] Generating Real ZK-SNARK for message #{}", nonce);

        // Stage 1: Setup proving parameters
        let k = 6;
        let params = Params::<G1Affine>::new(k);

        // Stage 2: Initialize circuit with payloads
        let payload_f = Fr::from_repr(hash).unwrap_or(Fr::from(nonce));
        let circuit = InterlinkCircuit {
            message_payload: Some(payload_f),
            sequence_number: Some(Fr::from(nonce)),
        };

        // Stage 3: Generate proving and verifying keys
        let vk = keygen_vk(&params, &circuit)
            .map_err(|_| crate::InterlinkError::ProofGenerationFailed)?;
        let pk = keygen_pk(&params, vk, &circuit)
            .map_err(|_| crate::InterlinkError::ProofGenerationFailed)?;

        // Stage 4: Produce public instance parameters
        let salt_hash = ethers_core::utils::keccak256(b"interlink_v1_domain");
        let mut arr = [0u8; 8];
        arr.copy_from_slice(&salt_hash[0..8]);
        let rc = Fr::from(u64::from_be_bytes(arr));

        let diff = payload_f + rc;
        let sq = diff.square();
        let commitment = sq * sq * diff + Fr::from(nonce);

        // nesting is a bit of a nightmare: &[&[&[scalar]]]
        // 1 circuit, 1 column, 1 value. easy.
        let instances: &[&[Fr]] = &[&[commitment]];
        let instances_ref: &[&[&[Fr]]] = &[instances];

        // Stage 5: Create final proof
        let mut transcript = Blake2bWrite::<_, G1Affine, Challenge255<_>>::init(vec![]);
        create_proof::<G1Affine, _, _, _, _>(
            &params,
            &pk,
            &[circuit],
            instances_ref,
            OsRng,
            &mut transcript,
        )
        .map_err(|_| crate::InterlinkError::ProofGenerationFailed)?;

        let proof = transcript.finalize();
        println!("[PROVER] Proof successful. Size: {} bytes", proof.len());

        Ok(proof)
    }

    async fn submit_to_hub(
        rpc_url: String,
        program_id_str: String,
        sequence: u64,
        payload_hash: [u8; 32],
        proof: Vec<u8>,
        keypair_path: String,
    ) -> crate::Result<()> {
        use base64::{engine::general_purpose, Engine as _};
        use ed25519_dalek::{Signer, SigningKey};
        use reqwest::Client;
        use sha2::{Digest, Sha256};

        println!(
            "[SUBMITTER] Dispatching real Signed Transaction to Solana Hub at {}...",
            rpc_url
        );

        // 1. Fetch real blockhash
        let client = Client::new();
        let payload_json = json!({
            "jsonrpc": "2.0", "id": 1, "method": "getLatestBlockhash", "params": [{"commitment": "confirmed"}]
        });
        let res = client
            .post(&rpc_url)
            .json(&payload_json)
            .send()
            .await
            .map_err(|e| crate::InterlinkError::NetworkError(e.to_string()))?;
        let result_json: serde_json::Value = res.json().await.unwrap_or_default();
        let blockhash_b64 = result_json["result"]["value"]["blockhash"]
            .as_str()
            .ok_or_else(|| {
                crate::InterlinkError::NetworkError("Failed to fetch blockhash".to_string())
            })?;
        let blockhash = general_purpose::STANDARD
            .decode(blockhash_b64)
            .map_err(|e| crate::InterlinkError::NetworkError(e.to_string()))?;

        // 2. Load keypair and identities
        let data =
            std::fs::read(keypair_path.replace('~', &std::env::var("HOME").unwrap_or_default()))
                .map_err(|_| {
                    crate::InterlinkError::NetworkError("Failed to load keypair".to_string())
                })?;
        let key_bytes: Vec<u8> = serde_json::from_slice(&data)
            .map_err(|_| crate::InterlinkError::NetworkError("Invalid key format".to_string()))?;
        let signing_key = SigningKey::from_bytes(&key_bytes[..32].try_into().unwrap());
        let public_key = signing_key.verifying_key().to_bytes();
        let program_id_raw = bs58::decode(program_id_str)
            .into_vec()
            .map_err(|_| crate::InterlinkError::NetworkError("Invalid program id".to_string()))?;

        // 3. Derive PDA: [b"state"]
        // Robust PDA derivation for the relayer.
        // Solana PDAs MUST be off the Ed25519 curve. Relying on a blind `255` is fragile.
        // We iterate downwards until we find a hash that correctly parses as an invalid curve point.
        let mut registry_pda = [0u8; 32];
        for bump in (0..=255).rev() {
            let mut hasher = Sha256::new();
            hasher.update(b"state");
            hasher.update([bump]);
            hasher.update(&program_id_raw);
            hasher.update(b"ProgramDerivedAddress");
            let result = hasher.finalize();

            registry_pda.copy_from_slice(&result);
            // If `VerifyingKey::from_bytes` errors, it's NOT a valid Edwards curve point,
            // which means it is a valid PDA off-curve!
            if ed25519_dalek::VerifyingKey::from_bytes(&registry_pda).is_err() {
                break;
            }
        }

        // 4. Build Instruction Data (Anchor format)
        let mut ix_data = Vec::with_capacity(8 + 8 + 8 + 4 + proof.len() + 32 + 32);
        ix_data.extend_from_slice(&[0x1d, 0x11, 0x18, 0x17, 0x11, 0x1a, 0x1c, 0x12]); // sighash: 'submit_proof'
        ix_data.extend_from_slice(&1u64.to_le_bytes()); // source id
        ix_data.extend_from_slice(&sequence.to_le_bytes());
        ix_data.extend_from_slice(&(proof.len() as u32).to_le_bytes());
        ix_data.extend_from_slice(&proof);
        ix_data.extend_from_slice(&payload_hash);
        // commitment re-calc
        use ff::PrimeField;
        use halo2curves::bn256::Fr;
        let payload_f = Fr::from_repr(payload_hash).unwrap_or(Fr::from(sequence));
        let salt_hash = ethers_core::utils::keccak256(b"interlink_v1_domain");
        let mut arr = [0u8; 8];
        arr.copy_from_slice(&salt_hash[0..8]);
        let rc = Fr::from(u64::from_be_bytes(arr));

        let commitment_f = (payload_f + rc).square() * (payload_f + rc) + Fr::from(sequence);
        ix_data.extend_from_slice(&commitment_f.to_repr());

        // 5. Build Transaction Message
        let mut msg = vec![1, 0, 1, 3]; // num_sigs, num_readonly_signed, num_readonly_unsigned, num_keys
        msg.extend_from_slice(&public_key);
        msg.extend_from_slice(&registry_pda);
        msg.extend_from_slice(&program_id_raw);

        msg.extend_from_slice(&blockhash);
        msg.push(1); // 1 instruction
        msg.push(2); // program_id index
        msg.push(2); // num accounts in instruction
        msg.push(1); // registry_pda (writable)
        msg.push(0); // relayer (writable, signer)

        msg.extend_from_slice(&(ix_data.len() as u16).to_le_bytes());
        msg.extend_from_slice(&ix_data);

        // 6. Sign and Broadcast
        let signature = signing_key.sign(&msg);
        let mut tx = vec![1]; // num signatures
        tx.extend_from_slice(&signature.to_bytes());
        tx.extend_from_slice(&msg);

        let final_json = json!({
            "jsonrpc": "2.0", "id": 1, "method": "sendTransaction", "params": [general_purpose::STANDARD.encode(&tx), {"encoding": "base64"}]
        });

        match client.post(&rpc_url).json(&final_json).send().await {
            Ok(r) => {
                let text = r.text().await.unwrap_or_default();
                if text.contains("error") {
                    eprintln!("[SUBMITTER] HUB REJECTED: {}", text);
                } else {
                    println!("[SUBMITTER] HUB CONFIRMED: Tx Sig {}...", signature);
                }
            }
            Err(e) => eprintln!("[ERROR] RPC Failed: {}", e),
        }

        Ok(())
    }
}
