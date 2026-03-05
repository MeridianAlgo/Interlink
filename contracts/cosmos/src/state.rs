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

/// Global sequence counter for cross-chain messages
pub const SEQUENCE: Item<u64> = Item::new("sequence");

/// Whether the contract is currently paused
pub const PAUSED: Item<bool> = Item::new("paused");

/// Map of executed message nonces (nonce -> true) for replay protection
pub const EXECUTED_MESSAGES: Map<u64, bool> = Map::new("executed");
