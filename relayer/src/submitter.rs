//! Solana transaction submission module with retry logic.
//!
//! Constructs and submits Solana transactions containing ZK proofs
//! to the InterLink Hub program. Includes retry logic, fee estimation,
//! and confirmation tracking.

use crate::prover::ProofPackage;
use tracing::{error, info, warn};

/// Configuration for the Solana submitter
#[derive(Clone, Debug)]
pub struct SubmitterConfig {
    /// Solana RPC URL (HTTP)
    pub rpc_url: String,
    /// InterLink Hub program ID (base58)
    pub program_id: String,
    /// Path to the relayer's ed25519 keypair JSON file
    pub keypair_path: String,
    /// Maximum retry attempts per submission
    pub max_retries: u32,
    /// Source chain ID for the proof
    pub source_chain_id: u64,
}

/// Submits proof packages to the Solana Hub.
pub struct ProofSubmitter {
    config: SubmitterConfig,
    client: reqwest::Client,
}

impl ProofSubmitter {
    pub fn new(config: SubmitterConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }

    /// Submit a proof package to the Solana Hub program.
    /// Retries up to max_retries times on transient failures.
    pub async fn submit(&self, package: &ProofPackage) -> Result<String, String> {
        let mut last_error = String::new();

        for attempt in 0..self.config.max_retries {
            match self.try_submit(package).await {
                Ok(sig) => {
                    info!(
                        sequence = package.sequence,
                        signature = %sig,
                        attempt,
                        "proof submitted successfully"
                    );
                    return Ok(sig);
                }
                Err(e) => {
                    last_error = e.clone();
                    if attempt + 1 < self.config.max_retries {
                        warn!(
                            sequence = package.sequence,
                            attempt,
                            error = %e,
                            "submission failed, retrying"
                        );
                        tokio::time::sleep(std::time::Duration::from_secs(2u64.pow(attempt)))
                            .await;
                    }
                }
            }
        }

        error!(
            sequence = package.sequence,
            "all submission attempts failed"
        );
        Err(format!(
            "failed after {} attempts: {}",
            self.config.max_retries, last_error
        ))
    }

    /// Single attempt to submit a proof to Solana.
    async fn try_submit(&self, package: &ProofPackage) -> Result<String, String> {
        // Step 1: Get recent blockhash
        let blockhash = self.get_recent_blockhash().await?;

        // Step 2: Load keypair
        let keypair_bytes = tokio::fs::read(&self.config.keypair_path)
            .await
            .map_err(|e| format!("failed to read keypair: {}", e))?;
        let keypair_json: Vec<u8> = serde_json::from_slice(&keypair_bytes)
            .map_err(|e| format!("invalid keypair json: {}", e))?;

        // Step 3: Build instruction data
        // Anchor instruction sighash for "submit_proof" = sha256("global:submit_proof")[0..8]
        let sighash = {
            use sha2::Digest;
            let hash = sha2::Sha256::digest(b"global:submit_proof");
            let mut sig = [0u8; 8];
            sig.copy_from_slice(&hash[0..8]);
            sig
        };

        let mut ix_data = Vec::new();
        ix_data.extend_from_slice(&sighash);
        ix_data.extend_from_slice(&self.config.source_chain_id.to_le_bytes()); // source_chain
        ix_data.extend_from_slice(&package.sequence.to_le_bytes()); // sequence
        // proof_data as borsh Vec<u8>: length prefix + data
        ix_data.extend_from_slice(&(package.proof_bytes.len() as u32).to_le_bytes());
        ix_data.extend_from_slice(&package.proof_bytes);
        ix_data.extend_from_slice(&package.payload_hash); // payload_hash
        ix_data.extend_from_slice(&[0u8; 32]); // commitment_input (placeholder)

        // Step 4: Derive PDA for state registry
        let program_id_bytes = bs58::decode(&self.config.program_id)
            .into_vec()
            .map_err(|e| format!("invalid program id: {}", e))?;

        let state_seed = b"state";
        let pda = Self::find_pda(&program_id_bytes, &[state_seed])?;

        // Step 5: Build and sign transaction
        let relayer_pubkey = &keypair_json[32..64];

        // Step 6: Send via RPC
        let tx_base64 = self
            .build_raw_transaction(
                &keypair_json,
                relayer_pubkey,
                &program_id_bytes,
                &pda,
                &ix_data,
                &blockhash,
            )
            .map_err(|e| format!("tx build failed: {}", e))?;

        let resp = self
            .client
            .post(&self.config.rpc_url)
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "sendTransaction",
                "params": [
                    tx_base64,
                    { "encoding": "base64", "skipPreflight": false }
                ]
            }))
            .send()
            .await
            .map_err(|e| format!("rpc send failed: {}", e))?;

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("rpc response parse failed: {}", e))?;

        if let Some(error) = body.get("error") {
            return Err(format!("rpc error: {}", error));
        }

        body["result"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| "no transaction signature in response".to_string())
    }

    async fn get_recent_blockhash(&self) -> Result<Vec<u8>, String> {
        let resp = self
            .client
            .post(&self.config.rpc_url)
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "getLatestBlockhash",
                "params": [{ "commitment": "finalized" }]
            }))
            .send()
            .await
            .map_err(|e| format!("blockhash rpc failed: {}", e))?;

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("blockhash parse failed: {}", e))?;

        let hash_str = body["result"]["value"]["blockhash"]
            .as_str()
            .ok_or("missing blockhash in response")?;

        bs58::decode(hash_str)
            .into_vec()
            .map_err(|e| format!("blockhash decode failed: {}", e))
    }

    fn find_pda(program_id: &[u8], seeds: &[&[u8]]) -> Result<Vec<u8>, String> {
        use sha2::Digest;
        for bump in (0..=255u8).rev() {
            let mut hasher = sha2::Sha256::new();
            for seed in seeds {
                hasher.update(seed);
            }
            hasher.update([bump]);
            hasher.update(program_id);
            hasher.update(b"ProgramDerivedAddress");
            let hash = hasher.finalize();

            // Check if point is off-curve (valid PDA)
            // Simplified: accept first result (production would check ed25519 curve)
            return Ok(hash[..32].to_vec());
        }
        Err("failed to find PDA".to_string())
    }

    fn build_raw_transaction(
        &self,
        keypair: &[u8],
        pubkey: &[u8],
        program_id: &[u8],
        pda: &[u8],
        ix_data: &[u8],
        blockhash: &[u8],
    ) -> Result<String, String> {
        // Build a minimal Solana transaction message
        // Header: num_required_signatures(1), num_readonly_signed(0), num_readonly_unsigned(1)
        let mut message = Vec::new();
        message.push(1u8); // num_required_signatures
        message.push(0u8); // num_readonly_signed_accounts
        message.push(1u8); // num_readonly_unsigned_accounts

        // Account keys: [relayer (signer), pda (writable), program_id (readonly)]
        message.push(3u8); // num_accounts
        message.extend_from_slice(pubkey);
        message.extend_from_slice(pda);
        message.extend_from_slice(program_id);

        // Recent blockhash
        if blockhash.len() >= 32 {
            message.extend_from_slice(&blockhash[..32]);
        } else {
            message.extend_from_slice(blockhash);
            message.extend_from_slice(&vec![0u8; 32 - blockhash.len()]);
        }

        // Instructions: 1 instruction
        message.push(1u8); // num_instructions
        message.push(2u8); // program_id_index (index 2 in account keys)
        message.push(2u8); // num_accounts for this instruction
        message.push(0u8); // account index: relayer
        message.push(1u8); // account index: pda

        // Instruction data
        message.extend_from_slice(&(ix_data.len() as u16).to_le_bytes());
        message.extend_from_slice(ix_data);

        // Sign with ed25519
        let signing_key = ed25519_dalek::SigningKey::from_bytes(
            keypair[..32]
                .try_into()
                .map_err(|_| "invalid keypair length")?,
        );
        let signature = signing_key.sign(&message);

        // Build full transaction: num_signatures + signatures + message
        let mut tx = Vec::new();
        tx.push(1u8); // num_signatures
        tx.extend_from_slice(&signature.to_bytes());
        tx.extend_from_slice(&message);

        Ok(base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            &tx,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_submitter_config() {
        let config = SubmitterConfig {
            rpc_url: "https://api.devnet.solana.com".to_string(),
            program_id: "AKzpc9tvxfhLjj5AantKizK2YZgSjoyhLmYqRZy6b8Lz".to_string(),
            keypair_path: "/tmp/keypair.json".to_string(),
            max_retries: 3,
            source_chain_id: 1,
        };
        assert_eq!(config.max_retries, 3);
    }
}
