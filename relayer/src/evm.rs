//! Minimal EVM helpers: keccak256, hex, JSON-RPC and log types.
//!
//! These replace ethers-rs, which is deprecated and pulled in
//! `jsonwebtoken → ring 0.16` (RUSTSEC-2025-0009/0010).

use serde::Deserialize;
use sha3::{Digest, Keccak256};

pub fn keccak256(data: &[u8]) -> [u8; 32] {
    Keccak256::digest(data).into()
}

/// `0x`-prefixed lowercase hex.
pub fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(2 + bytes.len() * 2);
    s.push_str("0x");
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// Decode hex with or without `0x`. Rejects odd lengths and non-hex characters.
pub fn from_hex(s: &str) -> Result<Vec<u8>, String> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    if !s.is_ascii() {
        return Err("non-ascii hex".into());
    }
    if !s.len().is_multiple_of(2) {
        return Err(format!("odd-length hex: {s}"));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| format!("bad hex: {e}")))
        .collect()
}

pub fn hex32(s: &str) -> Result<[u8; 32], String> {
    from_hex(s)?
        .try_into()
        .map_err(|v: Vec<u8>| format!("expected 32 bytes, got {}", v.len()))
}

pub fn hex20(s: &str) -> Result<[u8; 20], String> {
    from_hex(s)?
        .try_into()
        .map_err(|v: Vec<u8>| format!("expected 20 bytes, got {}", v.len()))
}

/// Parse a JSON-RPC quantity (`0x1a`).
pub fn quantity(s: &str) -> Result<u64, String> {
    u64::from_str_radix(s.strip_prefix("0x").unwrap_or(s), 16)
        .map_err(|e| format!("bad quantity {s}: {e}"))
}

/// Topic 0 of the gateway's `MessagePublished` event.
pub fn message_published_topic() -> [u8; 32] {
    keccak256(
        b"MessagePublished(bytes32,uint64,uint64,bytes32,uint8,bytes32,bytes32,uint256,bytes32,bytes32,bytes)",
    )
}

/// A log as returned by `eth_getLogs` / in receipts.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcLog {
    pub address: String,
    pub topics: Vec<String>,
    pub data: String,
    pub block_number: Option<String>,
    pub block_hash: Option<String>,
    pub transaction_hash: Option<String>,
    pub log_index: Option<String>,
    #[serde(default)]
    pub removed: bool,
}

/// Send a JSON-RPC request and return `result`, or an error string.
pub async fn rpc_call(
    client: &reqwest::Client,
    url: &str,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let body: serde_json::Value = client
        .post(url)
        .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}))
        .send()
        .await
        .map_err(|e| format!("{method}: {e}"))?
        .json()
        .await
        .map_err(|e| format!("{method} parse: {e}"))?;
    if let Some(err) = body.get("error") {
        return Err(format!("{method}: {err}"));
    }
    Ok(body["result"].clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keccak_known_vector() {
        // keccak256("") is a well-known constant.
        assert_eq!(
            to_hex(&keccak256(b"")),
            "0xc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
        );
    }

    #[test]
    fn hex_round_trip_and_errors() {
        assert_eq!(from_hex(&to_hex(&[0, 1, 0xab])).unwrap(), vec![0, 1, 0xab]);
        assert!(from_hex("0xabc").is_err());
        assert!(from_hex("0xzz").is_err());
        assert!(from_hex("0xé1").is_err());
        assert!(hex32("0x00").is_err());
        assert_eq!(quantity("0x1a").unwrap(), 26);
    }

    #[test]
    fn deserializes_rpc_log() {
        let log: RpcLog = serde_json::from_str(
            r#"{"address":"0xab","topics":["0x01"],"data":"0x","blockNumber":"0x10",
                "blockHash":"0x02","transactionHash":"0x03","logIndex":"0x0","removed":false}"#,
        )
        .unwrap();
        assert_eq!(quantity(log.block_number.as_deref().unwrap()).unwrap(), 16);
        assert!(!log.removed);
    }
}
