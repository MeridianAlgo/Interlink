# InterLink Protocol

## Overview

InterLink is a zero-knowledge interoperability protocol enabling mathematically guaranteed cross-chain state transitions. Version 0.7.1 introduces full BN254 coupling.
<p align="center">
  <img src="InterLink.png" alt="InterLink Architecture" width="80%">
</p>
By utilizing zk-SNARKs (specifically Halo2 with Groth16) and a high-throughput Solana Coordination Hub, InterLink enables instant, permissionless cross-chain message passing and asset transfers with O(1) on-chain verification.

The protocol follows a Hub-and-Spoke architecture where Solana acts as the central settlement and verification layer, while various EVM, Cosmos, and other blockchains serve as spoke gateways.

> **🚨 IMPORTANT – PROVER CONSISTENCY REQUIREMENT 🚨**
> 
> The relayer's Halo2 prover MUST use the exact same `interlink_v1_domain` salt when generating proofs. This is strictly required to match the updated Solidity input binding logic in `InterlinkGateway.sol:L175-180`. Ensure the entire pipeline (prover -> relayer -> on-chain verification) uses consistent domain separation to prevent proof mismatches.

---

## Project Architecture

The InterLink repository is organized into several specialized components that handle the end-to-end lifecycle of a cross-chain message:

### 1. Interlink Core (`interlink-core/`)
The foundational engine of the protocol. It contains the cryptographic logic, circuit definitions, and the relayer's internal machinery.
- **Circuit Engine**: Implements Halo2 circuits for state transition and Merkle inclusion verification.
- **Relayer Logic**: Monitors source chain events (via `ethers-rs`), generates ZK-SNARKs, and constructs manual Solana transactions to ensure reliability without external SDK bloat.
- **Networking**: Features a resilient WebSocket layer with exponential backoff for continuous event monitoring.

### 2. Specialized Circuits (`circuits/`)
A dedicated module for advanced ZK primitives, including a production-ready Merkle tree implementation designed for efficiency within the Halo2 proving system.

### 3. Multi-Chain Contracts (`contracts/`)
- **Solana Hub Gateway**: An Anchor-based program that serves as the central verification authority. It utilizes Solana's `alt_bn128` syscalls for efficient pairing checks.
- **EVM Spoke Gateways**: Solidity contracts that handle asset custody and emit events that trigger the cross-chain relaying process.
- **Cosmos Spoke Gateways**: Initial implementation of CosmWasm-based gateways for the InterLink network.

### 4. Relayer Node (`relayer/`)
A standalone executable that wraps the core library into a deployable service. It handles environment-based configuration and acts as the bridge between disparate networks.

### 5. Developer Portal (`website/`)
A documentation-first web application built with React and Vite. It provides a technical interface for developers to interact with the protocol and explore its architecture.

---

## Recent Breakthroughs (v0.7.1)

Significant progress has been made in transitioning the protocol from a research prototype to a production-grade environment:

- **ZK Proof Batching Engine**: Replaced single-payload proofs with `BatchedInterlinkCircuit`, enabling O(1) bulk verification of $N$ cross-chain state updates in a single SNARK, drastically reducing gas limits on EVM and compute limits on Solana.
- **Robust Ed25519 PDA Derivation**: Refactored the relayer's transaction builder to use rigorous `ed25519-dalek` off-curve validation, mirroring Solana's native `find_program_address` 1-for-1 and eliminating 50% arbitrary relayer crash rates.
- **Solana Dependency Convergence (1.18.26)**: Downgraded Anchor lang packages to `0.30.1` and precisely aligned crate locks to safely orchestrate alongside Solana Mainnet v1.18.26.
- **Cryptographic Maturity**: Replaced all simulated verification with real BN254 pairing checks using native Solana syscalls and EVM precompiles.
- **Resilient Infrastructure**: Implemented an advanced networking strategy for the relayer, ensuring high availability even during RPC instability.

---

## Testing Framework

InterLink employs a multi-layered testing strategy to ensure the integrity of its cryptographic proofs and contract logic.

### 1. Cryptographic and Core Logic Tests
These tests validate the correctness of the ZK-SNARK generation and the relayer's internal state transitions.
- **Tooling**: Rust native test runner.
- **Key Test**: `test_real_snark_generation` in `interlink-core` validates the end-to-end BN254 proving pipeline.
- **Execution**:
  ```bash
  cargo test -p interlink-core
  ```

### 2. Solana Hub Contract Tests
Validates the Anchor program logic, including proof verification, PDA derivation, and state updates.
- **Tooling**: Anchor Framework (TypeScript/Mocha).
- **Execution**:
  ```bash
  cd contracts/solana/interlink-hub
  anchor test
  ```

### 3. Merkle Circuit Tests
Focuses on the correctness of the Merkle inclusion proofs used within the Halo2 circuits.
- **Execution**:
  ```bash
  cargo test -p circuits
  ```

### 4. Integration and Manual Testing
The relayer can be tested in a staging environment by providing RPC endpoints for both the source (EVM) and destination (Solana) chains.
- **Execution**:
  ```bash
  EVM_RPC_URL="<EVM_WS_URL>" \
  SOLANA_RPC_URL="<SOLANA_HTTP_URL>" \
  HUB_PROGRAM_ID="<PROGRAM_ID>" \
  cargo run -p relayer
  ```

---

## Developer Setup

### Prerequisites
- Rust (Edition 2021)
- Solana CLI & Anchor (0.32.1)
- Node.js & npm/yarn (for website development)

### Building the Workspace
To build all core components:
```bash
cargo build --release
```

To build the Solana Hub specifically:
```bash
cd contracts/solana/interlink-hub
anchor build
```

---

## Technical Specifications

- **Proving System**: Halo2 (Groth16 backend)
- **Elliptic Curve**: BN254 (alt_bn128)
- **Verification Complexity**: O(1) on-chain across all supported networks.
- **State Commitment**: Sparse Merkle Trees for efficient inclusion proofs.

---

## Documentation and Resources

- **Technical Whitepaper**: [InterLink Research (PDF)](./Interlink_Research.pdf)
- **Developer Portal**: [interlink.protocol](https://meridianalgo.github.io/Interlink/)
- **GitHub Repository**: [MeridianAlgo/Interlink](https://github.com/MeridianAlgo/Interlink)
