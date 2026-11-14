use cosmwasm_std::Addr;
use cw_storage_plus::{Item, Map};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Config {
    pub guardian: Addr,
    pub hub_chain_id: u16,
    pub fee_rate_bps: u16,
}

/// Contract configuration
pub const CONFIG: Item<Config> = Item::new("config");

/// Guardian nominated via `ProposeGuardian`, awaiting `AcceptGuardian`.
pub const PENDING_GUARDIAN: Item<Addr> = Item::new("pending_guardian");

/// Next sequence number to assign to an outbound message (0-based, matches EVM gateway).
pub const SEQUENCE: Item<u64> = Item::new("sequence");

/// Whether the contract is currently paused
pub const PAUSED: Item<bool> = Item::new("paused");

/// Executed inbound messages, keyed by (source_chain, nonce).
///
/// Nonces are only unique per source chain, so keying by nonce alone would let
/// message #5 from chain A block message #5 from chain B.
pub const EXECUTED_MESSAGES: Map<(u64, u64), bool> = Map::new("executed_v2");
