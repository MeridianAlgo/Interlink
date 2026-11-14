//! InterLink relayer.
//!
//! Only the modules in the first group are used by the `relayer` binary.
//! The second group is standalone library / simulation code (fee models,
//! staking and governance simulations, research prototypes). It is tested but
//! **not** wired into the running relayer and provides no runtime guarantees.

// ─── Production pipeline (used by the relayer binary) ───────────────────────
pub mod batch;
pub mod events;
pub mod evm;
pub mod evm_submitter;
pub mod evm_tx;
pub mod fee;
pub mod finality;
pub mod gas;
pub mod http_api;
pub mod hub_outbound;
pub mod listener;
pub mod metrics;
pub mod prover;
pub mod solana_tx;
pub mod submitter;
pub mod webhook;

// ─── Experimental / simulation (not used by the relayer binary) ─────────────
pub mod amm;
pub mod atomic;
pub mod audit_trail;
pub mod benchmarks;
pub mod bitcoin_spv;
pub mod blob_space;
pub mod bounty;
pub mod byzantine_bridge;
pub mod circuitbreaker;
pub mod cosmos;
pub mod defi_integration;
pub mod enterprise;
pub mod explorer;
pub mod governance;
pub mod intent;
pub mod kyc;
pub mod lending;
pub mod liquidity_mining;
pub mod mev;
pub mod multisig;
pub mod network_opt;
pub mod nft_bridge;
pub mod nonce;
pub mod polygon;
pub mod privacy;
pub mod proof_batching;
pub mod proof_binding;
pub mod proof_perf;
pub mod ratelimit;
pub mod retry;
pub mod sdk_experience;
pub mod simulator;
pub mod sla;
pub mod solana_dex;
pub mod staking;
pub mod starknet;
pub mod swap_routing;
pub mod validator_rewards;
pub mod verkle;
pub mod vesting;
pub mod wallet;
pub mod wrapped;
pub mod xchain_messaging;
pub mod zk_research;
