# InterLink Protocol

> **Trustless. Scalable. Unified.**

**Version:** 6.0 (Technical Compendium)  
**Status:** Research & Architecture  
**License:** MIT  

Read the Full Technical Whitepaper: [View as PDF](Interlink_Research.pdf) | [View LaTeX Source (.tex)](Interlink_Research.tex)

---

## The Vision

**InterLink** is a next-generation "Layer 0" meta-protocol designed to solve the **Interoperability Trilemma**. By leveraging **Recursive Zero-Knowledge Proofs (zk-SNARKs)** and a high-performance **Solana Execution Hub**, InterLink enables the atomic, trustless transfer of value and data across heterogeneous blockchains (EVM, SVM, Cosmos, Move).

We are moving beyond "bridges" – which are fragile and centralized – to build a **Unified Liquidity Hyper-Structure**.

### The Problem: Fragmentation in Multi-Chain Ecosystem

The rapid proliferation of blockchain protocols has catalyzed a decentralized revolution, enabling trustless value transfer and immutable computation. However, this cambrian explosion of Layer-1 (L1) and Layer-2 (L2) networks has resulted in severe fragmentation, isolating capital, users, and state within disparate execution environments. The prevailing "multi-chain" thesis has inadvertently created a "siloed-chain" reality, where liquidity is fractured, and user experience is marred by complex, high-latency bridging mechanisms.

#### Critical Issues with Current Solutions

**Security Vulnerabilities:** Conventional interoperability solutions, relying heavily on multisig federation or optimistic verification, introduce systemic latency and critical centralization vectors. History has shown these "trusted" bridges to be the weakest link in the crypto-economic stack, accounting for billions of dollars in lost value due to key compromises and smart contract vulnerabilities.

**Liquidity Fragmentation:** Each blockchain operates as a sovereign digital nation with its own rules, consensus mechanism, and state. They are largely unaware of each other. An asset on Ethereum cannot natively exist on Solana. To move value, users must rely on "bridges" that lock assets on one chain and mint a synthetic representation on another. This creates "siloed liquidity," where capital is trapped in isolated pools, reducing efficiency and increasing slippage for traders.

**User Experience Complexity:** The current landscape forces users to navigate multiple wallets, understand different gas tokens, and manage complex bridging processes with high latency and security risks. This complexity severely limits mass adoption.

### Our Solution: Verifiable Interoperability

InterLink presents a hyper-structure for cross-chain composability powered by **recursive zero-knowledge proofs (zk-SNARKs)**. By utilizing the **halo2** proving system, InterLink enables trustless, atomic state transitions between heterogeneous ledgers without relying on honest majority assumptions for verification. Our architecture introduces a decentralized, stake-weighted **Relayer Network** incentivized by a novel, deflationary token model, coupled with a high-performance **Solana Execution Hub** for global state management.

We rigorously define the cryptographic primitives, circuit constraints, and economic game theory underpinning the protocol. Specifically, we demonstrate how InterLink achieves O(1) verification cost for batched cross-chain intents while preserving complete sovereign security. This system does not merely act as a bridge; it functions as the foundational substrate for the Unified Liquidity Layer of the decentralized web, allowing developers to build "omnichain" applications that are agnostic to the underlying chain.

## Core Features

*   **Zero-Knowledge Security:** No multisigs. No optimistic delays. We use **halo2** circuits to mathematically prove state transitions.
*   **Solana Execution Hub:** A centralized-but-trustless coordination layer that aggregates liquidity and verifies proofs at 50,000+ TPS.
*   **Rust-Based Relayers:** A decentralized network of `tokio`-powered nodes that observe chains, generate proofs, and earn **$ILINK**.
*   **Hyper-Deflationary Tokenomics:** A "burn-on-transit" model where every cross-chain interaction permanently removes $ILINK from supply.

## Technical Architecture

### Hub-and-Spoke Network Topology

InterLink adopts a Hub-and-Spoke network topology to minimize complexity and maximize scalability:

*   **Hub:** The Solana Blockchain acts as the global clearinghouse for state. All cross-chain messages are routed through the Hub.
*   **Spokes:** Connected blockchains (Ethereum, Arbitrum, Optimism, Cosmos Hub, Sui).

This O(N) connection complexity is superior to the O(N²) complexity of pairwise bridges. Adding a new chain only requires establishing a connection to the Solana Hub.

### Component Architecture

#### Source Chain Gateway (The Spoke)

Deployed on every supported chain (e.g., Ethereum), the Gateway is a set of smart contracts responsible for:

*   **Asset Custody:** Holding user funds in a non-custodial vault.
*   **Event Emission:** Emitting canonical logs (e.g., `LogMessagePublished`) that Relayers can subscribe to.
*   **Message Ingestion:** Receiving verified instructions from the Hub to unlock assets.

**Safety Module:** The Gateway includes a "Guardian" role (controlled by the DAO) that can pause the contract in the event of a detected anomaly, providing an emergency circuit breaker.

#### The Relayer Network (The Transport)

A decentralized network of off-chain nodes. The Relayer utilizes the **Halo2** proving system, which is a high-performance zk-SNARK implementation that allows for the creation of efficient, recursive proofs.

*   **Listener:** Uses WebSocket connections to full nodes of connected chains to listen for Gateway events.
*   **Prover (The Engine):** The most computationally intensive component. It runs the Halo2 proving stack.
    *   **Input:** Block header, Merkle branch, Transaction content.
    *   **Process:** Generates a ZK-proof attesting to the validity of the transaction and its inclusion in the canonical chain.
    *   **Recursion:** Multiple transaction proofs are aggregated into a single "Batch Proof" to amortize gas costs.
*   **Submitter:** Monitors the Solana Hub. When a batch is ready, it submits the proof and the public inputs as a Solana transaction.

#### The Solana Execution Hub (The Core)

Implemented as an Anchor Program on Solana:

*   **Verifier Contract:** Contains the verification keys for the supported circuits. It executes the pairing checks (or validates the accumulation scheme).
*   **State Registry:** Maintains a mapping of `sequence_number` → `message_hash` to prevent replay attacks.
*   **Liquidity Pools:** Implements a concentrated liquidity AMM (CLMM) where cross-chain swaps are executed.
*   **Messenger:** Queues outbound messages to destination chains.

### Cross-Chain Message Passing Lifecycle

To fully understand the system, we must examine the lifecycle of a message as it traverses the stack:

1.  **Inbound Trigger (The User Action):** The process begins when a user interacts with the Source Chain Gateway. For example, a user on Arbitrum calls `InterLinkRouter.send(dest=Solana, payload=...)`. This action locks assets or queues a message and emits an on-chain event containing the `destination_chain`, `recipient_address`, and `payload_hash`. This event serves as the immutable "truth" that the Relayer Network must observe.

2.  **Block Finality (The Safety Buffer):** Relayers do not act immediately upon seeing an event. They must wait for the source chain's consensus to finalize the block containing the transaction. On Ethereum L1, this is the time until the "finalized" epoch (approx. 12-15 minutes). On L2s like Arbitrum, this involves waiting for the sequencer to post the batch to L1. This step prevents the bridge from acting on a block that could be reorganized (reorged) out of existence.

3.  **Witness Extraction (The Data Gathering):** Once finality is reached, the Relayer queries an Archive Node. It fetches the Merkle Proof (the "witness") that cryptographically links the specific transaction receipt to the block's `receiptsRoot`. This proof is the private input to the SNARK circuit.

4.  **Proof Composition (The Computation):** The Relayer runs the ZK-circuit.
    *   **Circuit 1 (Transaction):** Proves "I possess a Merkle Path showing transaction T is in Block B."
    *   **Circuit 2 (Consensus):** Proves "Block B was signed by >66% of the validator set."
    *   **Circuit 3 (Aggregation):** Compresses Circuit 1 and Circuit 2 into a single, succinct proof π.

5.  **Solana Submission (The Handover):** The final proof π is submitted to the InterLink Hub on Solana via a transaction. This transaction includes the `public_inputs` (the message hash and the block root) which are visible to everyone.

6.  **On-Chain Verification (The Check):** The Solana Runtime executes the `verify` instruction within the Anchor program. This involves performing elliptic curve pairing checks (for KZG) or inner product arguments (for IPA). If the mathematics hold, the program updates the state: `processed_seq_eth = 55`.

7.  **Execution (The Result):** Once the message is marked as "verified" in the Program Derived Address (PDA), an "Executor" bot (which can be the same entity as the Relayer) triggers the destination logic. This effectively mints the wrapped token or executes the swap on the Solana CLMM.

### Consensus Verification in Circuits

A unique feature of InterLink is "Light Client in a Circuit." Instead of just verifying a transaction signature, we verify the consensus of the source chain:

*   **Ethereum (Sync Committee):** The circuit verifies the BLS signatures of the current Sync Committee (512 validators) to confirm the block header validity.
*   **Cosmos (Tendermint):** The circuit verifies the Ed25519 signatures of the validator set (voting power > 2/3).

This means InterLink inherits the full security of the connected chain's consensus, rather than relying on a small multisig.

## Cryptographic Foundations

### Zero-Knowledge Proofs: Technical Deep Dive

InterLink utilizes **zk-SNARKs** (Zero-Knowledge Succinct Non-Interactive Arguments of Knowledge). Let us break down each term:

*   **Zero-Knowledge:** The verifier learns nothing about the private inputs (witness) other than the fact that the statement is true. In InterLink, this means the Solana blockchain learns that "Transaction X happened on Ethereum" without needing to see the entire Ethereum blockchain history.

*   **Succinct:** The proof size is incredibly small (constant or logarithmic size), and verification time is fast (milliseconds). For example, a proof attesting to the validity of 1,000 transactions might only be a few hundred bytes. This is critical for on-chain verification where storage and compute are expensive.

*   **Non-Interactive:** Unlike interactive protocols, a SNARK allows the prover to generate a proof once, publish it, and anyone can verify it at any time without further interaction. This is achieved via the Fiat-Shamir heuristic, which turns an interactive protocol into a non-interactive one by using a cryptographic hash function to simulate the verifier's randomness.

*   **Argument:** A slightly weaker notion than a "Proof." A proof holds against a computationally infinite adversary. An argument holds against a "computationally bounded" adversary (one who cannot break encryption in polynomial time). For all practical purposes in blockchain, this is sufficient.

*   **of Knowledge:** The proof guarantees that the prover actually *knows* the witness (the private data), not just that such a witness exists.

### Elliptic Curve Cryptography (ECC)

InterLink leverages specific elliptic curves to optimize for both Ethereum compatibility and recursive proof composition.

#### BN254 (Alt-Bn128)

For interactions with the Ethereum Virtual Machine (EVM), we utilize the BN254 curve. This is a pairing-friendly curve defined over a finite field F_q. It allows for efficient pairing operations e: G₁ × G₂ → G_T, which are essential for verifying KZG commitments on-chain.

```
y² = x³ + 3 mod q
```

Ethereum has precompiled contracts (0x06, 0x07, 0x08) specifically optimized for BN254 operations, enabling gas-efficient verification.

#### Pallas and Vesta (The Pasta Curves)

For the recursive proof generation inside the Relayer Network, we employ the Pallas and Vesta curves. These form a cycle of curves: the scalar field of Pallas is the base field of Vesta, and vice versa.

*   **Pallas:** y² = x³ + 5
*   **Vesta:** y² = x³ + 5 (over a different field)

This cycle property allows us to efficiently verify a proof generated over Pallas inside a circuit defined over Vesta, avoiding the massive overhead of non-native field arithmetic ("wrong-field" arithmetic). This is the cornerstone of our "Halo2" recursion strategy.

### Polynomial Commitment Schemes (PCS)

A PCS allows one to commit to a polynomial P(X) and later prove that P(z) = y.

#### KZG (Kate-Zaverucha-Goldberg)

Used for the final "outer" proof submitted to Ethereum.

*   **Pros:** Constant size proofs (48 bytes), constant time verification.
*   **Cons:** Requires a Trusted Setup (Structured Reference String).

#### IPA (Inner Product Argument)

Used for the internal "inner" proofs within the recursive tree.

*   **Pros:** Transparent (no trusted setup), relies only on discrete log hardness.
*   **Cons:** O(log n) verification time (slower than KZG).

InterLink uses a hybrid approach: IPA for recursion (accumulation) and a final wrap into KZG/Groth16 for cheap on-chain verification.

### Solana Runtime (Sealevel)

Solana differs fundamentally from EVM-based chains:

*   **Stateless Execution:** Programs (smart contracts) are stateless; state is stored in separate "Accounts."
*   **Parallelism:** Transactions specify all accounts they will read or write upfront. This allows the runtime to schedule non-overlapping transactions in parallel.
*   **PDAs (Program Derived Addresses):** Addresses deterministically derived from a program ID and seeds (e.g., `hash("user_vault", user_pubkey)`). This allows us to map a user's Ethereum address to a specific Solana account without needing a private key for that account.

## Rust Implementation & Engineering

The engineering philosophy of InterLink prioritizes safety, performance, and maintainability. We leverage the Rust programming language across the entire stack---from the off-chain Relayers to the on-chain Solana programs---to ensure memory safety and type correctness.

### Circuit Engineering with Halo2

The heart of InterLink is the ZK-circuit. We use the **halo2-ce** (Community Edition) crate for its flexibility and support for the KZG commitment scheme.

#### Circuit Configuration

A Plonk-ish circuit is defined by a matrix of columns and rows. We define a custom `InterlinkConfig` struct to manage these resources.

```rust
#[derive(Clone, Debug)]
pub struct InterlinkConfig {
    // Advice columns (Witnesses): Private inputs like secret keys or Merkle paths
    pub advice: [Column<Advice>; 5],
    // Instance columns (Public Inputs): Root hash, Nullifier, etc.
    pub instance: Column<Instance>,
    // Fixed columns (Constants): Selector bits, lookup tables
    pub fixed: [Column<Fixed>; 2],
    // Selectors to toggle gates
    pub s_hash: Selector,
    pub s_verify: Selector,
}
```

#### The Merkle Inclusion Chip

We encapsulate logic into "Chips." The `MerkleChip` is responsible for verifying that a leaf exists in a tree. It takes a `Path` (witness) and a `Root` (public input) and enforces the hashing path.

*   **Input:** Leaf Node L, Path Elements P₀, ..., Pₙ, Path Indices I₀, ..., Iₙ (0 for left, 1 for right).
*   **Logic:** For each level, compute Hᵢ₊₁ = Hash(Lᵢ, Pᵢ) if Iᵢ=0, else Hᵢ₊₁ = Hash(Pᵢ, Lᵢ).
*   **Output:** The computed Root R_calc.
*   **Constraint:** R_calc == R_public.

### Relayer Infrastructure

The Relayer node is a high-performance Rust binary designed for 24/7 operation.

#### Concurrency Model

We utilize `tokio` for asynchronous runtime execution. The architecture follows a producer-consumer pattern:

1.  **Event Watcher (Producer):** Polls Ethereum logs via WebSocket (`ethers-rs`). Pushes events to a bounded `mpsc` channel.
2.  **Proof Generator (Consumer):** Pulls events, fetches witness data (Merkle proofs) from an archive node, and runs the CPU-intensive proving algorithm. This runs in a separate thread pool (`rayon`) to avoid blocking the async runtime.
3.  **Transaction Manager:** Queues the generated proof and manages the Solana transaction lifecycle (retries, priority fees).

```rust
pub async fn process_event(event: DepositEvent, params: &Params<Eq>) -> Result<Proof> {
    // 1. Fetch Witness Data (Network I/O)
    let merkle_path = eth_client.get_proof(event.address, event.slot).await?;
    
    // 2. Generate Proof (CPU Bound - Offload to thread)
    let proof = tokio::task::spawn_blocking(move || {
        let circuit = InterlinkCircuit::new(event, merkle_path);
        let mut transcript = Blake2bWrite::<_, _, Challenge255<_>>::init(vec![]);
        create_proof(params, &pk, &[circuit], &[&[&public_inputs]], &mut transcript)?;
        Ok(transcript.finalize())
    }).await??;

    Ok(proof)
}
```

### Performance Optimization

To achieve the sub-second latency required for a high-frequency bridge, we implement several optimization strategies.

#### Parallel Proof Generation

The process of generating a Zero-Knowledge Proof involves heavy mathematical operations, primarily Fast Fourier Transforms (FFTs) and Multi-Scalar Multiplications (MSMs). These operations are "embarrassingly parallel." We utilize the `rayon` crate in Rust to spawn a thread pool that matches the number of physical CPU cores.

```rust
// Pseudo-code for parallel MSM
let scalars = ...;
let points = ...;
let result = rayon::join(
    || msm(&scalars[0..n/2], &points[0..n/2]),
    || msm(&scalars[n/2..], &points[n/2..])
);
```

This reduces the proving time T from O(N) to O(N/C) where C is the core count.

#### Batching

Verifying a single proof on-chain consumes a fixed amount of gas (e.g., 300k gas on EVM). By aggregating N transactions into a single batch proof, we amortize this cost.

```
Cost per Tx = Verification Cost / N + Marginal Data Cost
```

If we batch 100 transactions, the verification cost per user drops by 99%.

## Tokenomics: The ILINK Standard

The ILINK token is the economic fuel of the InterLink ecosystem. It is designed to capture value from cross-chain activity while securing the network through stake-weighted incentives.

### Supply Dynamics

*   **Total Supply:** 1,000,000,000 (1 Billion) ILINK.
*   **Emission Schedule:** Halving every 2 years, similar to Bitcoin, ensuring long-term scarcity.
*   **Distribution:**
    *   20% Team & Advisors (4-year vesting, 1-year cliff).
    *   15% Investors (3-year vesting).
    *   30% Ecosystem Growth (Grants, Liquidity Mining).
    *   35% Community Treasury (DAO controlled).

### Utility A: The Security Bond

Relayers must stake ILINK to participate in the network.

```
Stake_min = 100,000 ILINK
```

If a Relayer submits an invalid proof (which is caught by the Verifier), their stake is slashed.

```
SlashAmount = Stake × 50%
```

The slashed funds are burned, instantly reducing the total supply. This creates a strong economic disincentive for malicious behavior.

### Utility B: Gas Abstraction & The Buy-Back-and-Burn

Users do not need to hold ILINK to use the bridge. They pay gas in the source chain's native token (e.g., ETH).

1.  User pays 0.01 ETH fee.
2.  Protocol swaps 0.01 ETH for ILINK on a DEX (e.g., Uniswap, Orca).
3.  **40% of this ILINK is burned.**
4.  60% is distributed to Relayers as a reward for their work.

This mechanism ensures that **Increased Usage → Increased Buy Pressure → Decreased Supply**.

### Mathematical Model of Deflation

Let V be the daily volume of cross-chain transfers.
Let f be the fee rate (e.g., 0.1%).
Let P be the price of ILINK.
The daily amount of tokens burned, B, is:

```
B = (V × f × 0.40) / P
```

If volume V grows and price P remains constant, the burn rate accelerates. If P increases, the burn rate (in token terms) slows, stabilizing the economy. This is a self-regulating feedback loop.

### Governance

ILINK holders govern the protocol via the InterLink DAO.

*   **Parameter Tweaking:** Adjusting the fee rate f or the burn percentage.
*   **Chain Support:** Voting to deploy Gateways to new chains.
*   **Treasury Management:** Allocating funds for audits, bug bounties, and developer grants.

Governance uses a quadratic voting mechanism to prevent plutocracy (whale dominance), where the cost of votes is the square of the number of votes: `Cost = (Votes)²`.

## Security Analysis

Security is the paramount requirement for any interoperability protocol. InterLink approaches security through a layered model, combining cryptographic hardness with game-theoretic incentives.

### Game Theoretic Analysis

We analyze the protocol as a game between a Relayer R and the Protocol P.

*   **Strategy Space:** R can choose to be {Honest, Malicious}.
*   **Payoff Matrix:**

|                 | **Valid Proof** | **Invalid Proof** |
|-----------------|-----------------|------------------|
| **Accept**      | R: +Fee, P: +UserTrust | R: +Vault, P: -Collapse |
| **Reject**      | R: -Gas, P: -Liveness | R: -Stake, P: +Security |

Let S be the staked amount. Let G be the potential gain from a hack (Total Value Locked). Let P_verif be the probability the verifier catches a cheat. In a ZK system, P_verif = 1 (soundness).

Therefore, the expected payoff for malicious behavior is:

```
E[Malicious] = (1 - P_verif) × G - P_verif × S
```

Since P_verif ≈ 1, E[Malicious] ≈ -S. A rational actor will never attack the system as long as the cost of generating a false proof (breaking the cryptography) is sufficiently high (computationally infeasible).

### Specific Attack Vectors & Mitigations

#### Long-Range Attacks

**Threat:** An attacker creates a private fork of the source chain starting from a point far in the past. In Proof-of-Stake systems, validators who have since unstaked (and can no longer be slashed) could sell their old private keys to an attacker. The attacker then builds a valid chain from that past point, eventually overtaking the canonical chain with a different state. If the bridge light client naively follows the "heaviest chain," it might accept this forged history.

**Mitigation:** InterLink implements **Checkpointing** (or "Weak Subjectivity"). The Solana Hub stores a history of finalized block headers. Once a header is finalized on Solana, it is considered immutable by the bridge logic. Even if the source chain undergoes a deep reorg that presents a heavier chain, the bridge will reject any path that contradicts its own finalized history.

```rust
// Pseudo-code for Checkpoint Verification
if incoming_header.height < last_finalized_height {
    if incoming_header.hash != stored_history[incoming_header.height] {
        return Error("Long Range Attack Detected");
    }
}
```

#### Data Availability (DA) Attacks

**Threat:** A malicious Relayer provides a valid ZK-proof that a state transition occurred (e.g., "I moved 100 USDC to Alice"), but they withhold the data required to reconstruct the new state (e.g., they don't reveal that the 100 USDC is now in Alice's balance). The proof validates the math, but without the data, the system enters an unrecoverable state where funds are locked because no one knows who owns them.

**Mitigation:** InterLink requires a **DA Attestation**. The ZK-circuit has an additional public input: a commitment (hash) to the data blob. The verification logic on Solana checks that this data blob has been posted to a robust Data Availability Layer (like Ethereum CallData, Celestia, or EigenDA). If the DA Layer does not confirm receipt of the data, the bridge rejects the proof, even if the ZK math is correct.

#### Censorship

**Threat:** The set of Relayers colludes to ignore transactions from specific users or addresses (e.g., OFAC sanctioned addresses, or competitors). While this doesn't steal funds, it breaks the "permissionless" guarantee of the blockchain.

**Mitigation:** We employ an **Open Relayer Set**. Unlike permissioned bridges where only a whitelist can relay, InterLink allows anyone to become a Relayer by staking ILINK. If the major Relayers censor a user, a new, independent Relayer can spin up, process that specific censored transaction (collecting the fee), and then shut down. This "Free Market" approach guarantees liveness as long as there is at least one honest rational actor in the world.

### Auditing & Formal Verification

To mitigate Smart Contract Risk, InterLink undergoes a rigorous pipeline:

1.  **Static Analysis:** Automated scanning with tools like Slither (for Solidity) and Soteria (for Solana) to catch common vulnerabilities like reentrancy or integer overflows.
2.  **Fuzzing:** Property-based testing using `proptest` and `trident` to feed millions of random inputs to the contracts and circuits, checking for invariants (e.g., "Total supply of wrapped tokens must always equal total assets locked").
3.  **Formal Verification:** We are working with firms like *Veridise* to mathematically prove that our ZK circuits exactly match the intended logic (constraint satisfaction). This proves that there are no "under-constrained" circuits where a hacker could create a valid proof for an invalid state.

## Competitive Analysis

InterLink competes in a crowded market. However, its architecture offers distinct advantages over incumbents.

| **Protocol** | **Trust Model** | **Latency** | **Gas Cost** | **Finality** | **Setup** | **Extensibility** |
|--------------|------------------|-------------|--------------|--------------|-----------|-------------------|
| **InterLink** | **ZK-Light Client** | **Medium** | **Low (O(1))** | **Src Finality** | **Trustless** | **High** |
| LayerZero v2 | DVNs (Multi-sig) | Low | Low | Instant | Permissioned | High |
| Axelar | PoS Validators | Medium | Medium | 1-2 mins | Trusted | Medium |
| Wormhole | Guardian Set (19/19) | Low | Low | Instant | Trusted | High |
| Cosmos IBC | Light Client | Low | Low | Instant | Trustless | Low |

### InterLink vs. LayerZero

LayerZero uses Ultra Light Nodes (ULNs). In v1, security relied on the independence of the Oracle and Relayer. In v2, it relies on Decentralized Verifier Networks (DVNs). While flexible, it ultimately trusts a set of external parties (like Google Cloud or Polyhedra). InterLink replaces these parties with a ZK proof. You don't trust the Relayer; you verify their work.

### InterLink vs. Axelar

Axelar is a blockchain itself. It uses a PoS validator set to vote on state changes. This is secure but adds an intermediary consensus layer. If the Axelar chain halts or is taken over, the bridge stops. InterLink is not a chain; it is a protocol. Its security inherits directly from the source and destination chains.

### InterLink vs. Wormhole

Wormhole relies on a Guardian Set of 19 reputable entities. While they have a strong track record, it is a Proof-of-Authority system. It is not censorship-resistant by design. InterLink's open relayer network is permissionless.

### InterLink vs. Cosmos IBC

IBC is the gold standard for trustless interoperability. However, it is expensive to verify on non-Cosmos chains (like Ethereum) due to the cost of checking Ed25519 signatures in EVM. InterLink is effectively IBC with ZK. We use ZK proofs to compress the consensus verification, making it affordable to run an IBC-style light client on any chain.

## Conclusion

InterLink represents the maturity of the interoperability landscape. We have moved from the "Wild West" of multisig bridges, plagued by hacks and centralization, to the era of **Verifiable Interoperability**.

By combining the cryptographic guarantees of **halo2 zk-SNARKs** with the high-performance execution environment of **Solana**, InterLink solves the Trilemma. It offers the trustlessness of a light client with the cost-effectiveness of a multisig. The result is a Unified Liquidity Layer that empowers developers to build applications that transcend chain boundaries.

The future of blockchain is not about which chain wins. It is about how they all work together. InterLink is the common language that allows them to speak. We envision a future where users are unaware of which chain they are on; they simply interact with applications, and InterLink handles the complex routing of value and data in the background. This "Chain Abstraction" is the final frontier for mass adoption.

As we move forward, our development focus remains on three core pillars: cryptographic efficiency, economic sustainability, and developer experience. We are not just building a bridge; we are building the internet's value layer. By making cross-chain interaction as simple as a local function call, we unlock a new class of decentralized applications that were previously impossible.

### Future Work

Our roadmap includes several key research directions:

*   **Privacy-Preserving Bridging:** integrating Aztec-style privacy proofs to allow users to bridge assets without revealing the amount or the recipient.
*   **Hardware Acceleration:** Developing FPGA and ASIC designs specifically optimized for the FFT (Fast Fourier Transforms) and MSM (Multi-Scalar Multiplications) operations in the Halo2 prover, aiming to reduce proof generation time to sub-second latency.
*   **L3 Fractal Scaling:** Allowing app-chains to settle directly on the InterLink Hub, using it as a DA and Settlement layer.

We invite the global research community, developers, and users to contribute to this open-source effort. Whether through formal security audits, circuit optimizations, or the creation of new cross-chain primitives, your participation is vital to realizing the vision of a unified, trustless digital economy. Together, we can turn the fragmented multi-chain reality into a cohesive, interoperable ecosystem where innovation knows no borders.

## Repository Structure

This repository contains the core research, documentation, and initial scaffold for the InterLink protocol.

```text
C:\Users\Ishaan\OneDrive\Desktop\Cobalt\
├── contracts/          # Smart Contracts (Source/Dest Chains)
│   ├── solana/         # Anchor Programs (The Hub)
│   ├── evm/            # Solidity Vaults (Ethereum, Arb, Op)
│   └── cosmos/         # CosmWasm Contracts
├── circuits/           # Halo2 ZK-Circuits
│   ├── src/
│   └── tests/
├── relayer/            # Rust Relayer Node Implementation
├── docs/               # Additional Technical Documentation
├── interlink-zk-interoperability-whitepaper.tex  # The Whitepaper (LaTeX)
└── README.md           # This file
```

## Technology Stack

*   **Language:** Rust (Relayers, Circuits, Solana Programs)
*   **ZK System:** Halo2 (PLONKish Arithmetization + KZG/IPA)
*   **Solana Framework:** Anchor
*   **Hashing:** Poseidon (ZK-friendly hash)

## Getting Started

### Prerequisites
*   Rust (latest stable)
*   Solana Tool Suite
*   Node.js & Yarn
*   LaTeX (to compile the whitepaper)

### Building the Whitepaper
To generate the PDF from the LaTeX source:
```bash
pdflatex interlink-zk-interoperability-whitepaper.tex
```

---

*“The future is not multi-chain; it is cross-chain native.”* — **MeridianAlgo Research**
