# InterLink Protocol

**Made by MeridianAlgo**

[Full Technical Whitepaper (RESEARCH.tex)](RESEARCH.tex)

License: MIT
Rust: 1.75+
Solana: Anchor
ZK-SNARKs: halo2

**InterLink** is a next-generation, zero-knowledge atomic cross-chain interoperability protocol designed to unify fragmented blockchain ecosystems without the security risks of traditional wrapping or centralized bridges.

---

## The Vision

In the 2026 blockchain landscape, assets remain trapped in "siloed islands." Moving value between chains often involves high fees, long wait times, and the systemic risk of wrapped assets. InterLink solves this by enabling **true native-asset atomic transfers** powered by ZK-SNARK verification and a decentralized, incentive-aligned relay network.

## Core Innovations

### 1. ZK-Powered Atomic Transfers
Unlike legacy bridges that rely on multisig or optimistic models, InterLink utilizes **halo2-based zero-knowledge proofs**. This ensures that an asset is proven to be locked on the source chain before it is ever released on the destination, with zero counterparty risk and no need for "wrapped" synthetics.

### 2. Unified Liquidity Aggregation
InterLink doesn't just move messages; it aggregates depth. By linking liquidity pools across arbitrary chains through a central Solana-based hub, InterLink provides users with the best execution prices and minimal slippage for cross-chain swaps. The protocol enforces a **Global Liquidity Invariant**:
$$\sum_{i=1}^n L_{i,t} = \text{Total Hub Collateral} + \text{In-Transit Volume}$$

### 3. Decentralized Relayer Network
A permissionless network of relayers, built in high-performance **Rust**, facilitates the flow of proofs. Security is enforced through economic incentives: relayers must stake **ILINK** tokens, which are automatically slashed by smart contracts if invalid proofs are submitted.

### 4. Native Fee Abstraction
Pay for gas on **any** chain using ILINK. The protocol abstracts the underlying gas complexities, offering massive discounts to ILINK holders while maintaining a deflationary pressure through a built-in fee-burn mechanism.

---

## Technical Architecture

InterLink consists of four primary layers working in concert:

| Component | Technology | Role |
| :--- | :--- | :--- |
| **Source Interface** | Smart Contracts | Locks native assets and triggers ZK-proof generation. |
| **Relay Network** | Rust / Tokio | Asynchronously forwards ZK-proofs to the InterLink Hub. |
| **InterLink Hub** | Solana / Anchor | Verifies proofs, manages liquidity, and executes atomic unlocks. |
| **Destination Release** | Smart Contracts | Receives the hub's signal to release native assets to the user. |

### Formal Verification
The protocol's state transition function is defined as:
$$\delta(S_{hub}, \pi, \alpha) \to S_{hub}', \beta_{dest}, \Phi_{burn}$$
Where $\pi$ is the verified halo2 proof and $\alpha$ is the unique source lock commitment. The hub ensures single-consume semantics for all transfer records.

---

## ILINK Tokenomics

The **ILINK** token is the lifeblood of the InterLink ecosystem, designed with a closed-loop economy that rewards participation and penalizes bad actors.

- **Total Supply:** 1,000,000,000 ILINK (Fixed)
- **Deflationary Mechanism:** A protocol-defined fraction of all cross-chain fees is automatically burned from the supply.
- **Utility:**
    - **Staking:** Required for Relayer Nodes to earn rewards and participate in the security game.
    - **Governance:** Vote on protocol upgrades, verification keys, and DAO treasury allocations.
    - **Gas Discounts:** Pay for cross-chain moves at 50%+ discounts vs. native gas.
    - **LP Yield:** Earn a share of protocol fees by providing liquidity to the Hub.

---

## Security Model

InterLink is built on the principle of **Trust Minimization**:

1. **Mathematical Certainty:** Proofs are generated using the halo2 system, ensuring the validity of source-chain state with $k=18+$ security parameters ($\epsilon \approx 2^{-128}$).
2. **Economic Security:** The relay network operates in an adversarial equilibrium where the cost of an attack exceeds any possible profit from state manipulation.
3. **No Central Point of Failure:** The relay network is decentralized, and the Hub is governed by an on-chain DAO.

---

## Getting Started (For Developers)

### Prerequisites
- Rust (latest stable)
- Solana CLI
- Anchor Framework

### Repository Structure
```text
.
├── src/
│   ├── relayer/          # Rust-based relayer implementation
│   ├── circuits/         # halo2 ZK-circuits for state verification
│   └── programs/         # Solana/Anchor smart contracts (The Hub)
├── RESEARCH.tex          # Full technical whitepaper (LaTeX)
└── README.md             # This file
```

---

*“Making blockchains feel like one unified network.”*
