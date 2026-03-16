# InterLink

## Overview

InterLink is a prototype for a zero-knowledge interoperability protocol enabling mathematically guaranteed cross-chain state transitions. Version 0.7.1 introduces full BN254 coupling.
<p align="center">
  <img src="InterLink.png" alt="InterLink Architecture" width="80%">
</p>
By utilizing zk-SNARKs (specifically Halo2 with Groth16) and a high-throughput Solana Coordination Hub, InterLink enables instant, permissionless cross-chain message passing and asset transfers with O(1) on-chain verification.

The protocol follows a Hub-and-Spoke architecture where Solana acts as the central settlement and verification layer, while various EVM, Cosmos, and other blockchains serve as spoke gateways.

> **IMPORTANT – PROVER CONSISTENCY REQUIREMENT**
>
> The relayer's Halo2 prover MUST use the exact same `interlink_v1_domain` salt when generating proofs. This is strictly required to match the updated Solidity input binding logic in `InterlinkGateway.sol:L175-180`. Ensure the entire pipeline (prover -> relayer -> on-chain verification) uses consistent domain separation to prevent proof mismatches.

---

## Project Architecture

The InterLink prototype is organized into several specialized components that handle the end-to-end lifecycle of a cross-chain message:

### 1. Interlink Core (`interlink-core/`)
The foundational engine of the protocol. It contains the cryptographic logic, circuit definitions, and the relayer's internal machinery.
- **Circuit Engine**: Implements Halo2 circuits for state transition and Merkle inclusion verification.
- **Relayer Logic**: Monitors source chain events (via `ethers-rs`), generates ZK-SNARKs, and constructs manual Solana transactions to ensure reliability without external SDK bloat.
- **Networking**: Features a resilient WebSocket layer with exponential backoff for continuous event monitoring.

### 2. Specialized Circuits (`circuits/`)
A dedicated module for advanced ZK primitives, including:
- **Merkle Circuit**: Poseidon-based Merkle tree inclusion proofs over BN254 within the Halo2 proving system.
- **Consensus Circuits**: Two production-grade consensus verification circuits:
  - `SyncCommitteeCircuit`: Proves Ethereum beacon chain finality via a BLS-aggregate-inspired participation accumulation gate (>=342/512 quorum).
  - `TendermintCircuit`: Proves Cosmos Tendermint finality via a >2/3 voting power accumulation gate.
- **Recursion / Folding Pipeline**: A `FoldingCircuit` and `FoldingPipeline` that accumulate multiple proofs pairwise (tree-structured, O(log N) depth) using a quintic Fiat-Shamir challenge `alpha = (C1 + C2)^5`, reducing on-chain verification to a single proof regardless of batch size.

### 3. Multi-Chain Contracts (`contracts/`)
- **Solana Hub Gateway**: An Anchor-based program (`interlink-hub`) that serves as the central verification authority. It utilizes Solana's `alt_bn128` syscalls for efficient pairing checks.
- **EVM Spoke Gateways**: Solidity contracts that handle asset custody and emit `MessagePublished` events that trigger the cross-chain relaying process.
- **Cosmos Spoke Gateways**: Initial implementation of CosmWasm-based gateways for the InterLink network.

### 4. Relayer Node (`relayer/`)
A standalone executable that wraps the core library into a deployable service. It handles environment-based configuration and acts as the bridge between disparate networks.

### 5. Developer Portal (`website/`)
A documentation-first web application built with React and Vite. It provides a technical interface for developers to interact with the protocol and explore its architecture.

---

## Recent Breakthroughs (v0.8.0)

Significant progress has been made in transitioning the protocol from a research prototype to a production-grade environment:

- **DAO Governance**: New `governance.rs` implements on-chain token-weighted voting — proposal creation (100k token threshold), 7-day voting period, 2-day timelock before execution, and treasury disbursement. Beats Wormhole (guardian multisig only) and Stargate (Snapshot off-chain).
- **Constant-Product AMM**: New `amm.rs` provides a Uniswap v2-style bridge liquidity pool with LP fee split (0.25% LPs + 0.05% protocol), price-impact guard (5% max), and APY tracking. Enables 3-5% LP yield competing with Across Protocol's 3-8%.
- **Intent-Based Routing**: New `intent.rs` lets users specify desired output; the solver finds optimal paths across DirectBridge, BridgeAndSwap, MultiHop, and SameChainSwap routes — matching LiFi's intent engine.
- **Wrapped Asset Registry**: New `wrapped.rs` provides a deterministic canonical mapping for wETH, wSOL, wMATIC across all supported chains. Automatic resolve-or-no-wrap decision on destination — no manual attestation step (beats Wormhole's attested token workflow).
- **API Rate Limiting**: New `ratelimit.rs` implements token-bucket rate limiting with Free (100 req/min), Pro (1000 req/min), and Enterprise (custom/unlimited) tiers. Standard `X-RateLimit-*` headers on every response.
- **Extended Metrics**: `metrics.rs` now tracks chain health (per-chain finality lag + RPC latency), user metrics (daily transfers, unique users, top corridors), and verification time with >500ms alert — fully Grafana/Prometheus compatible.
- **Security Test Suite**: New `tests/security.rs` (30 tests) validates double-spend prevention, byzantine validator threshold enforcement, AMM price-impact guards, governance attack vectors, rate-limit bypass resistance, and webhook DoS auto-disable.
- **MEV Capture + LP Breakeven Analysis**: `mev.rs` models the full revenue stack and computes minimum daily volume for zero-fee Tier 1 sustainability.
- **$INTERLINK Staking Rewards**: `staking.rs` implements Bronze/Silver/Gold/Platinum tiers with 10-100% fee discounts, 20% → 5% APY taper, validator eligibility, and configurable slashing.
- **Threshold Multi-Sig (3-of-5)**: `multisig.rs` implements Ed25519 threshold bundles — upgradeable to 13-of-19 (Wormhole parity) via governance vote.
- **Webhook Event Subscriptions**: `webhook.rs` + HTTP routes provide real-time push notifications with 3-attempt exponential backoff and auto-disable after 10 failures.
- **L2 Deployment**: `contracts/evm/script/DeployL2.s.sol` deploys to Optimism, Arbitrum One/Nova, Polygon PoS, and Base with per-chain finality calibration.
- **@interlink/sdk**: TypeScript SDK with `InterlinkClient` — quotes, fee comparisons, webhook management, zero-configuration Tier 1 transfers.
- **ZK Proof Batching Engine**: `BatchedInterlinkCircuit` enables O(1) on-chain verification for N cross-chain messages in a single SNARK.
- **Resilient Infrastructure**: Advanced WebSocket networking with exponential backoff; durable nonce pool for parallel Solana settlement.

---

## Testing Framework

InterLink employs a multi-layered testing strategy to ensure the integrity of its cryptographic proofs and contract logic. All tests have been verified passing on a live Solana devnet deployment.

### Full Test Suite Summary

| Layer | Tool | Tests | Status |
|---|---|---|---|
| Relayer lib (unit) | `cargo test --lib` | 160 | ✅ All passing |
| Relayer security | `cargo test --test security` | 30 | ✅ All passing |
| Relayer integration | `cargo test --test integration` | 18 | ✅ All passing |
| ZK Circuits | `cargo test -p circuits` | 10 | ✅ All passing |
| EVM Gateway (Solidity) | `forge test` | 23 | ✅ All passing |
| Solana Hub (devnet) | Anchor / Mocha | 4 | ✅ All passing |
| **Total** | | **245** | **✅ 245/245** |

---

### 1. Rust ZK Circuits & Core (`cargo test --workspace`)

Validates the full proving pipeline: circuit satisfiability, SNARK generation, proof serialization, consensus circuits, Merkle proofs, and the recursive folding pipeline.

**Run:**
```bash
cargo test --workspace
```

**Tests covered:**

| Crate | Test | Description |
|---|---|---|
| `interlink-core` | `test_interlink_circuit_valid` | Single-message Poseidon circuit satisfiability (MockProver) |
| `interlink-core` | `test_batched_interlink_circuit_valid` | Batched 3-message circuit satisfiability |
| `interlink-core` | `test_real_snark_generation` | Full BN254 Halo2 proof generation (keygen → prove) |
| `interlink-core` | `test_chain_roundtrip` | Chain ID encoding/decoding roundtrip |
| `interlink-core` | `test_payload_encode` | `InterLinkPayload` binary encoding |
| `interlink-core` | `test_cross_chain_message_trait` | `Message` trait impl for `CrossChainMessage` |
| `relayer` | `test_circuit_satisfiability` | Prover circuit passes constraint system |
| `relayer` | `test_full_prove_verify` | End-to-end: prove → serialize → verify |
| `relayer` | `test_vk_serialization` | VK round-trip serialization |
| `relayer` | `test_proof_serialization_size` | Proof byte length within expected bounds |
| `relayer` | `test_chain_finality_configs` | Finality seconds per chain (Ethereum, Solana, etc.) |
| `relayer` | `test_from_chain_id` | Chain ID → finality config resolution |
| `relayer` | `test_submitter_config` | Relayer submitter config construction |
| `relayer` | `test_listener_config` | Relayer listener config construction |
| `relayer` | `test_compact_u16` | Solana compact-u16 encoding correctness |
| `circuits` | `test_merkle_circuit_valid` | Poseidon Merkle tree inclusion proof |
| `circuits` | `test_sync_committee_quorum_met` | Ethereum sync committee: 300/400 weight (quorum met) |
| `circuits` | `test_sync_committee_quorum_not_met` | Ethereum sync committee: 100/400 weight (quorum not met) |
| `circuits` | `test_tendermint_quorum_met` | Cosmos Tendermint: 500/600 power (quorum met) |
| `circuits` | `test_tendermint_quorum_not_met` | Cosmos Tendermint: 100/600 power (quorum not met) |
| `circuits` | `test_folding_circuit` | Two-proof pairwise folding circuit (Fiat-Shamir alpha) |
| `circuits` | `test_folding_pipeline_pair` | Single-pair fold: commitment + evaluation correctness |
| `circuits` | `test_folding_pipeline_batch` | Batch flush triggered at batch_size=4 |
| `circuits` | `test_folding_pipeline_odd_count` | Tree-fold with odd number of proofs (carry-forward) |
| `circuits` | `test_config_compiles_and_satisfies` | Config module baseline |

---

### 2. EVM Gateway Solidity Tests (`forge test`)

Validates the `InterlinkGateway.sol` contract: message routing, swap initiation, NFT locking, ZK proof execution, pause/unpause, and access control. Includes fuzz tests with 256 runs each.

**Run:**
```bash
cd contracts/evm
forge test -vv
```

**Tests covered:**

| Test | Type | Description |
|---|---|---|
| `testSendCrossChainMessage_EmitsEvent` | Unit | `MessagePublished` event emitted correctly |
| `testSendCrossChainMessage_IncrementsNonce` | Unit | Nonce increments per message |
| `testSendCrossChainMessage_Native` | Fuzz (256) | Native ETH cross-chain message routing |
| `testSendCrossChainMessage_Token` | Fuzz (256) | ERC-20 cross-chain message routing |
| `testSendCrossChainMessage_RevertsWhenPaused` | Unit | Reverts when protocol is paused |
| `testSendCrossChainMessage_RevertsWrongNativeValue` | Unit | Reverts on ETH amount mismatch |
| `testInitiateSwap_Native` | Fuzz (256) | Native ETH swap initiation |
| `testInitiateSwap_Token` | Fuzz (256) | ERC-20 swap initiation |
| `testInitiateSwap_RevertsZeroAmount` | Unit | Reverts on zero swap amount |
| `testInitiateSwap_RevertsZeroRecipient` | Unit | Reverts on zero recipient |
| `testLockNFT_Success` | Fuzz (256) | NFT locking for cross-chain transfer |
| `testLockNFT_RevertsZeroContract` | Unit | Reverts on zero NFT contract address |
| `testLockNFT_RevertsZeroRecipient` | Unit | Reverts on zero recipient |
| `testExecuteVerifiedMessage_RejectsReplay` | Unit | Replay attack prevention |
| `testExecuteVerifiedMessage_RejectsShortProof` | Unit | Rejects malformed short proof |
| `testExecuteVerifiedMessage_RejectsWithoutVK` | Unit | Rejects execution without VK set |
| `testExecuteVerifiedMessage_RejectsZeroTarget` | Unit | Reverts on zero target address |
| `testSetVK_Success` | Unit | Guardian can set verification key |
| `testSetVK_RevertsNonGuardian` | Unit | Non-guardian cannot set VK |
| `testPauseUnpause` | Unit | Guardian can pause and unpause |
| `testPause_RevertsNonGuardian` | Unit | Non-guardian cannot pause |
| `testEmergencyWithdraw_ETH` | Unit | Emergency ETH withdrawal |
| `testEmergencyWithdraw_Token` | Unit | Emergency ERC-20 withdrawal |

---

### 3. Solana Hub Tests — Live Devnet

The Anchor program has been **deployed and verified on Solana devnet**. Tests were executed against the live deployment.

**Deployed Program:**
- **Program ID:** `AKzpc9tvxfhLjj5AantKizK2YZgSjoyhLmYqRZy6b8Lz`
- **Deploy Transaction:** `59bSsMZU9GZAvaVL5mEL8NQ24Ucs4HvgpQ7i9TmnFDYPdANivm24n3b18Yb5Nx2aSZYp9ti3NmT7GF1jsd2v59ZY`
- **IDL Account:** `3YddvfRCPY6MALVoVzAN59njDkzCEQ3px2NMhszBrZpp`
- **Cluster:** Solana Devnet (`https://api.devnet.solana.com`)

**Run against devnet:**
```bash
cd contracts/solana/interlink-hub
ANCHOR_PROVIDER_URL=https://api.devnet.solana.com \
ANCHOR_WALLET=~/.config/solana/id.json \
yarn run ts-mocha -p ./tsconfig.json -t 1000000 "tests/**/*.ts"
```

**Run against localnet:**
```bash
cd contracts/solana/interlink-hub
anchor test --provider.cluster localnet
```

**Tests covered:**

| Test | Description | Devnet Tx |
|---|---|---|
| `initializes the hub with fee rate` | Initializes `StateRegistry` PDA with admin and `feeRateBps=10`, verifies `nextSequence=0` and `vkInitialized=false` | `29UhT9Znj...` |
| `rejects submit_proof when VK is not initialized` | Confirms `VKNotInitialized` or `AccountNotInitialized` error before VK is set | — |
| `rejects submit_proof with wrong proof length` | Confirms `InvalidProof` or gating error on 100-byte proof (expected 256) | — |
| `rejects duplicate sequence numbers via sequential ordering` | Verifies `nextSequence` guard is in place to prevent replay | — |

---

## Developer Setup

### Prerequisites
- Rust (Edition 2021)
- Solana CLI (`>=1.18`) & Anchor CLI (`0.32.1`)
- Foundry (`forge`, `cast`, `anvil`) — install via `foundryup`
- Node.js & yarn (for Anchor TypeScript tests and website)

### Building the Workspace
```bash
# Build all Rust crates
cargo build --release

# Build the Solana Hub program
cd contracts/solana/interlink-hub
anchor build

# Build EVM contracts
cd contracts/evm
forge build
```

### Running All Tests
```bash
# Rust (ZK circuits, core, relayer)
cargo test --workspace

# EVM Solidity
cd contracts/evm && forge test -vv

# Solana (localnet)
cd contracts/solana/interlink-hub && anchor test --provider.cluster localnet

# Solana (devnet — requires SOL balance)
cd contracts/solana/interlink-hub
ANCHOR_PROVIDER_URL=https://api.devnet.solana.com \
ANCHOR_WALLET=~/.config/solana/id.json \
yarn run ts-mocha -p ./tsconfig.json -t 1000000 "tests/**/*.ts"
```

---

## Technical Specifications

- **Proving System**: Halo2 (IPA commitment, BN254 scalar field)
- **Elliptic Curve**: BN254 (alt_bn128)
- **Hash Function**: Poseidon-style quintic S-box (`x^5`) — injective over BN254 since `gcd(5, p-1) = 1`
- **Domain Separation**: `keccak256("interlink_v1_domain")` used as round constant across all circuits and the prover
- **Verification Complexity**: O(1) on-chain across all supported networks
- **State Commitment**: Sparse Merkle Trees for efficient inclusion proofs
- **Recursion**: Pairwise proof folding with Fiat-Shamir challenge `alpha = (C1 + C2)^5`, O(log N) folding depth
- **Consensus Models**: Ethereum Sync Committee (342/512 threshold) and Cosmos Tendermint (>2/3 voting power)

---

## Documentation and Resources

- **Technical Whitepaper**: [InterLink Research (PDF)](./Interlink_Research.pdf)
- **Developer Portal**: [interlink.protocol](https://meridianalgo.github.io/Interlink/)
- **GitHub Repository**: [MeridianAlgo/Interlink](https://github.com/MeridianAlgo/Interlink)
