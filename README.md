# InterLink Protocol

> **Trustless Multi-Chain Connectivity powered by Zero-Knowledge Math.**

InterLink is a high-performance, decentralized interoperability protocol designed to bridge the gap between fragmented blockchain ecosystems. By leveraging **zk-SNARKs (Groth16)** and a high-throughput **Solana Coordination Hub**, InterLink enables instant, trustless, and permissionless cross-chain message passing and asset transfers.

---

## 🚀 Recent Breakthroughs (v0.6.4)

We have recently achieved major milestones in our transition from a research prototype to an audit-ready production environment:

*   **Real Cryptographic Verification**: Replaced all simulated verification logic with production-grade BN254 pairing checks using Solana's `alt_bn128` syscalls and EVM precompiles (`0x08`).
*   **Deterministic PDA Architecture**: Refactored the Solana Gateway Hub to utilize a secure, derivation-based state registry (`seeds = [b"state"]`).
*   **Engineered Relayer Pipeline**:
    *   **SDK-Less Submission**: Developed a manual Solana transaction engine to eliminate dependency conflicts while maintaining 100% protocol fidelity.
    *   **Resilient Networking**: Implemented an exponential backoff reconnect strategy for WebSocket event monitoring.
*   **Premium Developer Portal**: Launched a visually stunning, documentation-first website built with React and Vite, featuring glassmorphism aesthetics and technical deep-dives.

---

## 🏛️ Architecture

InterLink follows a "Hub-and-Spoke" model where Solana serves as the central verification and coordination center.

*   **[`interlink-core`](./interlink-core/)**: The cryptographic engine containing the Halo2 circuits for Merkle inclusion proofs and state transition verification.
*   **[`contracts/solana`](./contracts/solana/)**: The Hub Gateway (Anchor). Performs O(1) verification of cross-chain proofs using native curve syscalls.
*   **[`contracts/evm`](./contracts/evm/)**: Spoke Gateways (Solidity). Handles asset custody and publishes events for the Relayer network.
*   **[`relayer`](./relayer/)**: The decentralized worker network. Monitors source chains, generates ZK-SNARKs, and submits them to the Hub.

---

## 🛠️ Developer Setup

### Prerequisites
*   Rust (Edition 2021)
*   Solana CLI & Anchor (0.32.1)
*   Node.js (for the website)

### Building the Project
```bash
# Build the core engine
cargo build --release

# Build the Solana Hub
cd contracts/solana/interlink-hub
anchor build
```

### Running the Relayer
The relayer is configured via environment variables for maximum flexibility in production:
```bash
EVM_RPC_URL="wss://..." \
SOLANA_RPC_URL="https://..." \
HUB_PROGRAM_ID="..." \
cargo run -p relayer
```

---

## 📊 Technical Stats
*   **Proving System**: Halo2 (Groth16)
*   **Curve**: BN254 (alt_bn128)
*   **Verification Complexity**: O(1) on-chain
*   **Hub Performance**: 1,000+ Cross-chain settlements per second (theoretical)

---

## 🌐 Community & Docs
*   **Website**: [interlink.protocol](https://meridianalgo.github.io/Interlink/)
*   **Paper**: [Technical Whitepaper (PDF)](./Interlink_Research.pdf)
*   **GitHub**: [MeridianAlgo/Interlink](https://github.com/MeridianAlgo/Interlink)

---

**“The future of Web3 is not fragmented. It is InterLinked.”**
