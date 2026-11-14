//! Watches the Solana hub for `OutboundMessageEvent` (from
//! `burn_for_withdrawal`) and forwards messages bound for this relayer's EVM
//! gateway.
//!
//! Only `finalized` transactions are read, which is Solana's finality.
//! Anchor events are `Program data:` log lines, and *any* program can print
//! such a line, so events are accepted only while the hub program is the one
//! currently executing (tracked through the invoke/success log stack).

use crate::events::{CanonicalMessage, HUB_CHAIN_ID};
use crate::solana_tx::{self as stx, Pubkey};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{info, warn};

#[derive(Clone, Debug)]
pub struct HubOutboundConfig {
    pub rpc_url: String,
    pub program_id: Pubkey,
    /// Only messages to this EVM chain and gateway are forwarded.
    pub evm_chain_id: u64,
    pub evm_gateway: [u8; 20],
    pub poll_interval: Duration,
    /// Process signatures after this one on startup (None = only new ones).
    pub start_after_signature: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OutboundEvent {
    pub signature: String,
    pub message: CanonicalMessage,
}

/// Decoded `OutboundMessageEvent` fields (Borsh, after the 8-byte discriminator).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundMessageEvent {
    pub payload_hash: [u8; 32],
    pub dst_chain: u64,
    pub dst_gateway: [u8; 32],
    pub nonce: u64,
    pub action: u8,
    pub sender: [u8; 32],
    pub asset: [u8; 32],
    pub amount: u128,
    pub recipient: [u8; 32],
    pub extra_hash: [u8; 32],
}

const EVENT_LEN: usize = 32 + 8 + 32 + 8 + 1 + 32 + 32 + 16 + 32 + 32;

impl OutboundMessageEvent {
    pub fn decode(data: &[u8]) -> Option<Self> {
        let disc = stx::anchor_event_discriminator("OutboundMessageEvent");
        if data.len() != 8 + EVENT_LEN || data[..8] != disc {
            return None;
        }
        let mut p = 8usize;
        let mut take = |n: usize| {
            let s = &data[p..p + n];
            p += n;
            s
        };
        Some(Self {
            payload_hash: take(32).try_into().ok()?,
            dst_chain: u64::from_le_bytes(take(8).try_into().ok()?),
            dst_gateway: take(32).try_into().ok()?,
            nonce: u64::from_le_bytes(take(8).try_into().ok()?),
            action: take(1)[0],
            sender: take(32).try_into().ok()?,
            asset: take(32).try_into().ok()?,
            amount: u128::from_le_bytes(take(16).try_into().ok()?),
            recipient: take(32).try_into().ok()?,
            extra_hash: take(32).try_into().ok()?,
        })
    }

    /// Canonical message as the destination gateway will rebuild it. None if
    /// the emitted payload hash does not match the fields.
    pub fn to_message(&self, hub_program: &Pubkey) -> Option<CanonicalMessage> {
        let m = CanonicalMessage {
            src_chain: HUB_CHAIN_ID,
            src_gateway: *hub_program,
            dst_chain: self.dst_chain,
            dst_gateway: self.dst_gateway,
            nonce: self.nonce,
            action: self.action,
            sender: self.sender,
            asset: self.asset,
            amount: self.amount,
            recipient: self.recipient,
            extra_hash: self.extra_hash,
        };
        (m.hash() == self.payload_hash).then_some(m)
    }
}

/// Extract hub events from a transaction's log messages, accepting
/// `Program data:` only while `program_id` is at the top of the invoke stack.
pub fn parse_hub_events(logs: &[String], program_id: &str) -> Vec<OutboundMessageEvent> {
    let mut stack: Vec<&str> = Vec::new();
    let mut events = Vec::new();
    for line in logs {
        if let Some(b64) = line.strip_prefix("Program data: ") {
            if stack.last() == Some(&program_id) {
                if let Ok(bytes) =
                    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64.trim())
                {
                    if let Some(ev) = OutboundMessageEvent::decode(&bytes) {
                        events.push(ev);
                    }
                }
            }
        } else if let Some(rest) = line.strip_prefix("Program ") {
            let mut parts = rest.split_whitespace();
            let id = parts.next().unwrap_or_default();
            match parts.next() {
                Some("invoke") => stack.push(id),
                Some("success") | Some("failed:") | Some("failed") => {
                    stack.pop();
                }
                _ => {} // "consumed", "log:", "return:" etc.
            }
        }
    }
    events
}

pub struct HubOutboundListener {
    config: HubOutboundConfig,
    tx: mpsc::Sender<OutboundEvent>,
    client: reqwest::Client,
    cursor: Option<String>,
    seen: std::collections::HashSet<[u8; 32]>,
}

impl HubOutboundListener {
    pub fn new(config: HubOutboundConfig, tx: mpsc::Sender<OutboundEvent>) -> Self {
        let cursor = config.start_after_signature.clone();
        Self {
            config,
            tx,
            client: reqwest::Client::new(),
            cursor,
            seen: Default::default(),
        }
    }

    async fn rpc(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        crate::evm::rpc_call(&self.client, &self.config.rpc_url, method, params).await
    }

    pub async fn run(&mut self) -> Result<(), String> {
        let program = bs58::encode(self.config.program_id).into_string();
        if self.cursor.is_none() {
            // Start at the current tip: only withdrawals from now on.
            let latest = self
                .rpc(
                    "getSignaturesForAddress",
                    serde_json::json!([program, {"limit": 1, "commitment": "finalized"}]),
                )
                .await?;
            self.cursor = latest[0]["signature"].as_str().map(str::to_string);
        }
        info!(cursor = ?self.cursor, "hub outbound listener started");
        loop {
            if let Err(e) = self.poll(&program).await {
                if e == "channel closed" {
                    return Ok(());
                }
                warn!(error = %e, "hub outbound poll failed");
            }
            tokio::time::sleep(self.config.poll_interval).await;
        }
    }

    /// Fetch finalized signatures newer than the cursor (oldest first).
    async fn new_signatures(&self, program: &str) -> Result<Vec<serde_json::Value>, String> {
        let mut all = Vec::new();
        let mut before: Option<String> = None;
        loop {
            let mut opts = serde_json::json!({"limit": 1000, "commitment": "finalized"});
            if let Some(c) = &self.cursor {
                opts["until"] = c.clone().into();
            }
            if let Some(b) = &before {
                opts["before"] = b.clone().into();
            }
            let page = self
                .rpc(
                    "getSignaturesForAddress",
                    serde_json::json!([program, opts]),
                )
                .await?;
            let page = page.as_array().cloned().unwrap_or_default();
            let full = page.len() == 1000;
            before = page
                .last()
                .and_then(|s| s["signature"].as_str())
                .map(str::to_string);
            all.extend(page);
            if !full {
                break;
            }
        }
        all.reverse();
        Ok(all)
    }

    async fn poll(&mut self, program: &str) -> Result<(), String> {
        for sig_info in self.new_signatures(program).await? {
            let Some(sig) = sig_info["signature"].as_str().map(str::to_string) else {
                continue;
            };
            if sig_info["err"].is_null() {
                let tx = self
                    .rpc(
                        "getTransaction",
                        serde_json::json!([sig, {"encoding": "json", "commitment": "finalized", "maxSupportedTransactionVersion": 0}]),
                    )
                    .await?;
                if tx["meta"]["err"].is_null() {
                    let logs: Vec<String> =
                        serde_json::from_value(tx["meta"]["logMessages"].clone())
                            .unwrap_or_default();
                    for ev in parse_hub_events(&logs, program) {
                        self.forward(&sig, ev).await?;
                    }
                }
            }
            self.cursor = Some(sig);
        }
        Ok(())
    }

    async fn forward(&mut self, sig: &str, ev: OutboundMessageEvent) -> Result<(), String> {
        let Some(message) = ev.to_message(&self.config.program_id) else {
            warn!(
                sig,
                "outbound event hash does not match its fields; ignoring"
            );
            return Ok(());
        };
        let mut gw = [0u8; 32];
        gw[12..].copy_from_slice(&self.config.evm_gateway);
        if message.dst_chain != self.config.evm_chain_id || message.dst_gateway != gw {
            return Ok(()); // addressed to another chain or gateway
        }
        if !self.seen.insert(ev.payload_hash) {
            return Ok(());
        }
        info!(sig, nonce = message.nonce, "hub withdrawal detected");
        self.tx
            .send(OutboundEvent {
                signature: sig.to_string(),
                message,
            })
            .await
            .map_err(|_| "channel closed".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HUB: &str = "AKzpc9tvxfhLjj5AantKizK2YZgSjoyhLmYqRZy6b8Lz";

    fn sample_event() -> (OutboundMessageEvent, Pubkey) {
        let program = stx::pubkey(HUB).unwrap();
        let mut ev = OutboundMessageEvent {
            payload_hash: [0; 32],
            dst_chain: 1,
            dst_gateway: [0x0B; 32],
            nonce: 4,
            action: 1,
            sender: [0x22; 32],
            asset: [0x33; 32],
            amount: 5_000,
            recipient: [0x44; 32],
            extra_hash: crate::evm::keccak256(b""),
        };
        let m = CanonicalMessage {
            src_chain: HUB_CHAIN_ID,
            src_gateway: program,
            dst_chain: ev.dst_chain,
            dst_gateway: ev.dst_gateway,
            nonce: ev.nonce,
            action: ev.action,
            sender: ev.sender,
            asset: ev.asset,
            amount: ev.amount,
            recipient: ev.recipient,
            extra_hash: ev.extra_hash,
        };
        ev.payload_hash = m.hash();
        (ev, program)
    }

    fn encode(ev: &OutboundMessageEvent) -> String {
        let mut b = stx::anchor_event_discriminator("OutboundMessageEvent").to_vec();
        b.extend_from_slice(&ev.payload_hash);
        b.extend_from_slice(&ev.dst_chain.to_le_bytes());
        b.extend_from_slice(&ev.dst_gateway);
        b.extend_from_slice(&ev.nonce.to_le_bytes());
        b.push(ev.action);
        b.extend_from_slice(&ev.sender);
        b.extend_from_slice(&ev.asset);
        b.extend_from_slice(&ev.amount.to_le_bytes());
        b.extend_from_slice(&ev.recipient);
        b.extend_from_slice(&ev.extra_hash);
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b)
    }

    #[test]
    fn accepts_event_emitted_by_hub() {
        let (ev, program) = sample_event();
        let logs = vec![
            format!("Program {HUB} invoke [1]"),
            "Program log: Instruction: BurnForWithdrawal".to_string(),
            "Program TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA invoke [2]".to_string(),
            "Program TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA success".to_string(),
            format!("Program data: {}", encode(&ev)),
            format!("Program {HUB} consumed 30000 of 200000 compute units"),
            format!("Program {HUB} success"),
        ];
        let parsed = parse_hub_events(&logs, HUB);
        assert_eq!(parsed, vec![ev.clone()]);
        assert!(parsed[0].to_message(&program).is_some());
    }

    /// An attacker program logging identical bytes must not be accepted,
    /// whether invoked at top level or via CPI from the hub's transaction.
    #[test]
    fn rejects_lookalike_event_from_other_program() {
        let (ev, _) = sample_event();
        let fake = "Fake111111111111111111111111111111111111111";
        let logs = vec![
            format!("Program {fake} invoke [1]"),
            format!("Program data: {}", encode(&ev)),
            format!("Program {fake} success"),
            format!("Program {HUB} invoke [1]"),
            format!("Program {fake} invoke [2]"),
            format!("Program data: {}", encode(&ev)),
            format!("Program {fake} success"),
            format!("Program {HUB} success"),
        ];
        assert!(parse_hub_events(&logs, HUB).is_empty());
    }

    #[test]
    fn rejects_hash_mismatch() {
        let (mut ev, program) = sample_event();
        ev.amount += 1;
        assert!(ev.to_message(&program).is_none());
    }
}
