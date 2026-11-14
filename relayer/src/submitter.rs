//! Solana hub submission: attest proofs (`submit_proof`) and execute attested
//! messages (`execute_transfer` / `execute_swap`), with confirmation tracking.
//!
//! A signature from `sendTransaction` only means the RPC accepted the
//! transaction, so every send waits for `confirmed` status.

use crate::events::{CanonicalMessage, ACTION_SWAP, ACTION_TRANSFER};
use crate::prover::ProofPackage;
use crate::solana_tx::{self as stx, AccountMeta, Instruction, Pubkey};
use tracing::{error, info, warn};

/// Compute units requested for `submit_proof` (pairing + keccak + account init).
const PROOF_COMPUTE_UNITS: u32 = 400_000;
/// Compute units for execution (CPI to token / ATA programs).
const EXECUTE_COMPUTE_UNITS: u32 = 200_000;
/// How long to wait for a submitted transaction to confirm.
const CONFIRM_TIMEOUT_SECS: u64 = 60;

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

/// Outcome of a hub transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HubOutcome {
    /// Landed and confirmed with this signature.
    Confirmed(String),
    /// Nothing to do: someone already attested / executed this message.
    AlreadyDone,
}

/// Submits proof packages to the Solana Hub. Cheaply cloneable.
#[derive(Clone)]
pub struct ProofSubmitter {
    config: SubmitterConfig,
    client: reqwest::Client,
    /// Cached raw keypair bytes (64 bytes: secret || public) loaded at startup.
    keypair: Vec<u8>,
}

impl ProofSubmitter {
    /// Load the keypair from disk once at construction. Fails fast if the
    /// path is invalid so the relayer surfaces the misconfiguration at startup.
    pub fn new(config: SubmitterConfig) -> Self {
        let raw = std::fs::read(&config.keypair_path).unwrap_or_else(|e| {
            panic!("failed to load keypair from {}: {}", config.keypair_path, e)
        });
        let keypair: Vec<u8> = serde_json::from_slice(&raw)
            .unwrap_or_else(|e| panic!("invalid keypair JSON at {}: {}", config.keypair_path, e));
        assert!(keypair.len() >= 64, "keypair must be at least 64 bytes");
        Self {
            config,
            client: reqwest::Client::new(),
            keypair,
        }
    }

    fn relayer(&self) -> Pubkey {
        self.keypair[32..64].try_into().unwrap()
    }

    fn program_id(&self) -> Result<Pubkey, String> {
        stx::pubkey(&self.config.program_id)
    }

    /// Attest a proof on the hub (`submit_proof`). Idempotent.
    pub async fn submit(&self, package: &ProofPackage) -> Result<HubOutcome, String> {
        let message = package
            .message
            .as_ref()
            .ok_or("legacy event format cannot be submitted to the hub")?;
        let ix = submit_proof_ix(&self.program_id()?, &self.relayer(), package, message)?;
        self.send_with_retries(
            package.sequence,
            vec![stx::set_compute_unit_limit(PROOF_COMPUTE_UNITS), ix],
        )
        .await
    }

    /// Execute an attested message (mint wrapped tokens or swap). Permissionless
    /// on-chain; the relayer does it so transfers complete without user action.
    pub async fn execute(
        &self,
        message: &CanonicalMessage,
        data: &[u8],
    ) -> Result<HubOutcome, String> {
        let program = self.program_id()?;
        let wrapped = self.wrapped_asset(&program, message).await?;
        let ix = match message.action {
            ACTION_TRANSFER => execute_transfer_ix(&program, &self.relayer(), message, &wrapped)?,
            ACTION_SWAP => {
                let swap = SwapParams::decode(data)?;
                execute_swap_ix(&program, &self.relayer(), message, &wrapped, &swap)?
            }
            other => return Err(format!("action {other} has no hub executor")),
        };
        self.send_with_retries(
            message.nonce,
            vec![stx::set_compute_unit_limit(EXECUTE_COMPUTE_UNITS), ix],
        )
        .await
    }

    /// Read the registered `WrappedAsset` (mint) for a message's source asset.
    async fn wrapped_asset(
        &self,
        program: &Pubkey,
        m: &CanonicalMessage,
    ) -> Result<WrappedAsset, String> {
        let address = stx::pda(&[b"wrapped", &m.src_chain.to_le_bytes(), &m.asset], program)?;
        let body = self
            .rpc(
                "getAccountInfo",
                serde_json::json!([bs58::encode(address).into_string(), {"encoding": "base64", "commitment": "confirmed"}]),
            )
            .await?;
        let value = &body["result"]["value"];
        if value.is_null() {
            return Err(format!(
                "asset {} from chain {} is not registered on the hub",
                crate::evm::to_hex(&m.asset),
                m.src_chain
            ));
        }
        let b64 = value["data"][0].as_str().ok_or("account data missing")?;
        let data = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64)
            .map_err(|e| format!("account data: {e}"))?;
        WrappedAsset::decode(&data)
    }

    async fn send_with_retries(
        &self,
        sequence: u64,
        ixs: Vec<Instruction>,
    ) -> Result<HubOutcome, String> {
        let mut last_error = String::new();
        for attempt in 0..self.config.max_retries {
            match self.send(&ixs).await {
                Ok(outcome) => {
                    info!(sequence, ?outcome, attempt, "hub transaction done");
                    return Ok(outcome);
                }
                Err(e) => {
                    last_error = e.clone();
                    if attempt + 1 < self.config.max_retries {
                        warn!(sequence, attempt, error = %e, "hub transaction failed, retrying");
                        tokio::time::sleep(std::time::Duration::from_secs(2u64.pow(attempt))).await;
                    }
                }
            }
        }
        error!(sequence, "all hub transaction attempts failed");
        Err(format!(
            "failed after {} attempts: {}",
            self.config.max_retries, last_error
        ))
    }

    async fn send(&self, ixs: &[Instruction]) -> Result<HubOutcome, String> {
        let blockhash = self.get_recent_blockhash().await?;
        let msg = stx::compile_message(&self.relayer(), ixs, &blockhash)?;
        let tx = stx::sign_transaction(&msg, &self.keypair)?;
        let tx_base64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &tx);

        let body = self
            .rpc(
                "sendTransaction",
                serde_json::json!([tx_base64, { "encoding": "base64", "skipPreflight": false }]),
            )
            .await?;
        if let Some(error) = body.get("error") {
            let text = error.to_string();
            if is_already_done(&text) {
                return Ok(HubOutcome::AlreadyDone);
            }
            return Err(format!("rpc error: {}", text));
        }
        let sig = body["result"]
            .as_str()
            .ok_or("no transaction signature in response")?
            .to_string();
        self.wait_for_confirmation(&sig).await?;
        Ok(HubOutcome::Confirmed(sig))
    }

    async fn rpc(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        self.client
            .post(&self.config.rpc_url)
            .json(
                &serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}),
            )
            .send()
            .await
            .map_err(|e| format!("{} failed: {}", method, e))?
            .json()
            .await
            .map_err(|e| format!("{} parse failed: {}", method, e))
    }

    async fn wait_for_confirmation(&self, sig: &str) -> Result<(), String> {
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(CONFIRM_TIMEOUT_SECS);
        while std::time::Instant::now() < deadline {
            let body = self
                .rpc("getSignatureStatuses", serde_json::json!([[sig]]))
                .await?;
            let status = &body["result"]["value"][0];
            if !status.is_null() {
                if !status["err"].is_null() {
                    return Err(format!("transaction {} failed: {}", sig, status["err"]));
                }
                if matches!(
                    status["confirmationStatus"].as_str(),
                    Some("confirmed" | "finalized")
                ) {
                    return Ok(());
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        Err(format!(
            "transaction {} not confirmed within {}s",
            sig, CONFIRM_TIMEOUT_SECS
        ))
    }

    async fn get_recent_blockhash(&self) -> Result<[u8; 32], String> {
        let body = self
            .rpc(
                "getLatestBlockhash",
                serde_json::json!([{ "commitment": "finalized" }]),
            )
            .await?;
        let hash_str = body["result"]["value"]["blockhash"]
            .as_str()
            .ok_or("missing blockhash in response")?;
        stx::pubkey(hash_str).map_err(|e| format!("blockhash: {e}"))
    }
}

/// Preflight failures that mean the work was already done by someone else.
fn is_already_done(rpc_error: &str) -> bool {
    // `init` of an existing message record, or the hub's own guard.
    rpc_error.contains("already in use") || rpc_error.contains("MessageAlreadyExecuted")
}

/// Borsh encoding of the hub's `InboundMessage` (destination fields are implied).
pub fn encode_inbound(m: &CanonicalMessage) -> Vec<u8> {
    let mut out = Vec::with_capacity(192);
    out.extend_from_slice(&m.src_chain.to_le_bytes());
    out.extend_from_slice(&m.src_gateway);
    out.extend_from_slice(&m.nonce.to_le_bytes());
    out.extend_from_slice(&m.sender);
    out.extend_from_slice(&m.asset);
    out.extend_from_slice(&m.amount.to_le_bytes());
    out.extend_from_slice(&m.recipient);
    out.extend_from_slice(&m.extra_hash);
    out
}

/// `submit_proof(payload_hash, action, msg, proof_data)`. Account order
/// follows the Anchor `SubmitProof` struct.
pub fn submit_proof_ix(
    program: &Pubkey,
    relayer: &Pubkey,
    package: &ProofPackage,
    m: &CanonicalMessage,
) -> Result<Instruction, String> {
    let mut data = stx::anchor_ix_discriminator("submit_proof").to_vec();
    data.extend_from_slice(&package.payload_hash);
    data.push(m.action);
    data.extend_from_slice(&encode_inbound(m));
    data.extend_from_slice(&(package.proof_bytes.len() as u32).to_le_bytes());
    data.extend_from_slice(&package.proof_bytes);
    Ok(Instruction {
        program_id: *program,
        accounts: vec![
            AccountMeta::readonly(stx::pda(&[b"state"], program)?),
            AccountMeta::writable(stx::pda(&[b"stake", relayer], program)?),
            AccountMeta::readonly(stx::pda(&[b"vk"], program)?),
            AccountMeta::writable(stx::pda(&[b"message", &package.payload_hash], program)?),
            AccountMeta::signer(*relayer),
            AccountMeta::readonly(stx::SYSTEM_PROGRAM),
        ],
        data,
    })
}

/// The hub's `WrappedAsset` account: 8-byte discriminator, src_chain (u64),
/// asset ([u8;32]), mint (Pubkey), decimals_shift (u8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrappedAsset {
    pub mint: Pubkey,
    pub decimals_shift: u8,
}

impl WrappedAsset {
    pub fn decode(data: &[u8]) -> Result<Self, String> {
        if data.len() < 81 {
            return Err(format!(
                "WrappedAsset account too short: {} bytes",
                data.len()
            ));
        }
        Ok(Self {
            mint: data[48..80].try_into().unwrap(),
            decimals_shift: data[80],
        })
    }
}

/// `execute_transfer(payload_hash, msg)`; account order follows `ExecuteTransfer`.
pub fn execute_transfer_ix(
    program: &Pubkey,
    payer: &Pubkey,
    m: &CanonicalMessage,
    wrapped: &WrappedAsset,
) -> Result<Instruction, String> {
    let payload_hash = m.hash();
    let mut data = stx::anchor_ix_discriminator("execute_transfer").to_vec();
    data.extend_from_slice(&payload_hash);
    data.extend_from_slice(&encode_inbound(m));
    Ok(Instruction {
        program_id: *program,
        accounts: vec![
            AccountMeta::signer(*payer),
            AccountMeta::writable(stx::pda(&[b"message", &payload_hash], program)?),
            AccountMeta::readonly(stx::pda(
                &[b"wrapped", &m.src_chain.to_le_bytes(), &m.asset],
                program,
            )?),
            AccountMeta::writable(wrapped.mint),
            AccountMeta::readonly(m.recipient),
            AccountMeta::writable(stx::associated_token_address(&m.recipient, &wrapped.mint)?),
            AccountMeta::readonly(stx::pda(&[b"mint_authority"], program)?),
            AccountMeta::readonly(stx::token_program()),
            AccountMeta::readonly(stx::associated_token_program()),
            AccountMeta::readonly(stx::SYSTEM_PROGRAM),
        ],
        data,
    })
}

/// Swap parameters carried in the gateway event's `data`:
/// `abi.encode(bytes32 tokenOut, uint256 minAmountOut, bytes swapData)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwapParams {
    pub token_out: Pubkey,
    pub min_amount_out: u64,
    pub swap_data_hash: [u8; 32],
}

impl SwapParams {
    pub fn decode(data: &[u8]) -> Result<Self, String> {
        let word = |i: usize| -> Result<&[u8], String> {
            data.get(i * 32..(i + 1) * 32)
                .ok_or_else(|| "swap data too short".to_string())
        };
        let token_out: Pubkey = word(0)?.try_into().unwrap();
        let min_word = word(1)?;
        if min_word[..24].iter().any(|b| *b != 0) {
            return Err("minAmountOut exceeds u64".into());
        }
        if word(2)?[..31].iter().any(|b| *b != 0) || word(2)?[31] != 0x60 {
            return Err("non-canonical swapData offset".into());
        }
        let len_word = word(3)?;
        if len_word[..24].iter().any(|b| *b != 0) {
            return Err("swapData length too large".into());
        }
        let len = u64::from_be_bytes(len_word[24..].try_into().unwrap()) as usize;
        let swap_data = data.get(128..128 + len).ok_or("swapData truncated")?;
        Ok(Self {
            token_out,
            min_amount_out: u64::from_be_bytes(min_word[24..].try_into().unwrap()),
            swap_data_hash: crate::evm::keccak256(swap_data),
        })
    }
}

/// `execute_swap(payload_hash, msg, token_out, min_amount_out, swap_data_hash)`;
/// account order follows `ExecuteSwap`.
pub fn execute_swap_ix(
    program: &Pubkey,
    payer: &Pubkey,
    m: &CanonicalMessage,
    wrapped: &WrappedAsset,
    swap: &SwapParams,
) -> Result<Instruction, String> {
    let payload_hash = m.hash();
    let pool = stx::pda(
        &[b"liquidity_pool", &wrapped.mint, &swap.token_out],
        program,
    )?;
    let mut data = stx::anchor_ix_discriminator("execute_swap").to_vec();
    data.extend_from_slice(&payload_hash);
    data.extend_from_slice(&encode_inbound(m));
    data.extend_from_slice(&swap.token_out);
    data.extend_from_slice(&swap.min_amount_out.to_le_bytes());
    data.extend_from_slice(&swap.swap_data_hash);
    Ok(Instruction {
        program_id: *program,
        accounts: vec![
            AccountMeta::signer(*payer),
            AccountMeta::readonly(stx::pda(&[b"state"], program)?),
            AccountMeta::writable(stx::pda(&[b"message", &payload_hash], program)?),
            AccountMeta::readonly(stx::pda(
                &[b"wrapped", &m.src_chain.to_le_bytes(), &m.asset],
                program,
            )?),
            AccountMeta::writable(pool),
            AccountMeta::writable(stx::pda(&[b"pool_vault", &pool, &swap.token_out], program)?),
            AccountMeta::readonly(swap.token_out),
            AccountMeta::readonly(m.recipient),
            AccountMeta::writable(stx::associated_token_address(
                &m.recipient,
                &swap.token_out,
            )?),
            AccountMeta::readonly(stx::token_program()),
            AccountMeta::readonly(stx::associated_token_program()),
            AccountMeta::readonly(stx::SYSTEM_PROGRAM),
        ],
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::HUB_CHAIN_ID;

    fn sample_message(action: u8) -> CanonicalMessage {
        CanonicalMessage {
            src_chain: 1,
            src_gateway: [1; 32],
            dst_chain: HUB_CHAIN_ID,
            dst_gateway: [2; 32],
            nonce: 9,
            action,
            sender: [3; 32],
            asset: [0; 32],
            amount: 1_000,
            recipient: [4; 32],
            extra_hash: [5; 32],
        }
    }

    fn sample_package() -> ProofPackage {
        let message = sample_message(ACTION_TRANSFER);
        ProofPackage {
            proof_bytes: vec![7; 256],
            public_inputs: vec![],
            sequence: 9,
            payload_hash: message.hash(),
            message: Some(message),
        }
    }

    #[test]
    fn submit_proof_layout_and_size() {
        let p = sample_package();
        let ix = submit_proof_ix(&[8; 32], &[6; 32], &p, p.message.as_ref().unwrap()).unwrap();
        // 8 discriminator + 32 hash + 1 action + 192 message + 4 len + 256 proof
        assert_eq!(ix.data.len(), 8 + 32 + 1 + 192 + 4 + 256);
        assert_eq!(&ix.data[8..40], &p.payload_hash);
        assert_eq!(ix.data[40], ACTION_TRANSFER);
        assert_eq!(&ix.data[41..49], &1u64.to_le_bytes());

        let ixs = [stx::set_compute_unit_limit(PROOF_COMPUTE_UNITS), ix];
        let msg = stx::compile_message(&[6; 32], &ixs, &[9; 32]).unwrap();
        assert!(
            1 + 64 + msg.len() <= 1232,
            "tx is {} bytes",
            1 + 64 + msg.len()
        );
    }

    #[test]
    fn execute_transfer_fits_and_binds_recipient_ata() {
        let m = sample_message(ACTION_TRANSFER);
        let wrapped = WrappedAsset {
            mint: [0x33; 32],
            decimals_shift: 10,
        };
        let ix = execute_transfer_ix(&[8; 32], &[6; 32], &m, &wrapped).unwrap();
        assert_eq!(ix.accounts.len(), 10);
        assert_eq!(
            ix.accounts[5].pubkey,
            stx::associated_token_address(&m.recipient, &wrapped.mint).unwrap()
        );
        let ixs = [stx::set_compute_unit_limit(EXECUTE_COMPUTE_UNITS), ix];
        let msg = stx::compile_message(&[6; 32], &ixs, &[9; 32]).unwrap();
        assert!(1 + 64 + msg.len() <= 1232);
    }

    #[test]
    fn swap_params_decode_strictly() {
        let mut data = vec![0u8; 128];
        data[..32].copy_from_slice(&[0x55; 32]);
        data[56..64].copy_from_slice(&3000u64.to_be_bytes());
        data[95] = 0x60;
        data[127] = 3;
        data.extend_from_slice(&[1, 2, 3]);
        data.resize(160, 0);
        let p = SwapParams::decode(&data).unwrap();
        assert_eq!(p.token_out, [0x55; 32]);
        assert_eq!(p.min_amount_out, 3000);
        assert_eq!(p.swap_data_hash, crate::evm::keccak256(&[1, 2, 3]));

        let mut too_big = data.clone();
        too_big[40] = 1; // minAmountOut above u64
        assert!(SwapParams::decode(&too_big).is_err());
        assert!(SwapParams::decode(&data[..100]).is_err());
    }

    #[test]
    fn wrapped_asset_decode() {
        let mut data = vec![0u8; 81];
        data[48..80].copy_from_slice(&[0x77; 32]);
        data[80] = 10;
        let w = WrappedAsset::decode(&data).unwrap();
        assert_eq!(w.mint, [0x77; 32]);
        assert_eq!(w.decimals_shift, 10);
        assert!(WrappedAsset::decode(&data[..50]).is_err());
    }

    #[test]
    fn already_done_detection() {
        assert!(is_already_done(
            "Allocate: account Address { .. } already in use"
        ));
        assert!(is_already_done("Error Code: MessageAlreadyExecuted"));
        assert!(!is_already_done("InvalidProof"));
    }
}
