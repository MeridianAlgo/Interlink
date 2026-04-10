# ZK Circuit Architecture (V1)

InterLink utilizes Halo2 (with the BN254 curve) for generating succinct proofs of cross-chain state transitions. This document details the specific gates and constraints used in our primary circuits.

## 1. Merkle Inclusion Circuit (`merkle.rs`)

We use the **Poseidon Hash** for Merkle trees to minimize constraints on-chain.
- **Arity**: Binary Tree.
- **Hash Function**: Quintic S-Box ($x^5$) Poseidon.
- **Constraints**: Approximately 300 gates per level.
- **Verification**: Used to prove that a specific `MessageID` exists in the source chain's state root.

## 2. Sync Committee Circuit (`consensus.rs`)

For Ethereum (PoS) finality, we verify the **Sync Committee** signatures.
- **Quorum**: $\ge 342$ out of 512 validators.
- **Logic**: Aggregates BLS signatures (using a mapping to BN254-friendly scalar fields) and verifies participation bits.
- **Optimization**: We use a lookup table for bit counting to reduce the number of advice columns.

## 3. Folding Pipeline (`folding.rs`)

To support high throughput, we don't verify every message individually. Instead, we use a recursive folding strategy.
- **Depth**: $\log_2(N)$ where $N$ is the number of messages.
- **Fiat-Shamir**: Challenges are generated using $\alpha = (C_1 + C_2)^5 \pmod{p}$.
- **Performance**: folding 64 proofs takes approximately 1.2s on a 16-core AWS c6g instance.

## 4. Constraint Optimization

| Feature | implementation | Benefit |
|---|---|---|
| **Fixed Columns** | Precomputed lagrange coefficients | Reduces proof generation by 15% |
| **Lookup Gates** | Range checks (0-255) | 10x more efficient than arithmetic gates |
| **Custom Gates** | Optimized BN254 scalar multiply | Minimizes rotation count in Halo2 rows |

## 5. Security Domains

Every proof is salted with `keccak256("interlink_v1_domain")`. This ensures:
1. Proofs cannot be replayed on different chain IDs.
2. Proofs generated for InterLink cannot be used in other bridges using similar circuits.
3. Transaction malleability is prevented by binding the `sequence_id` into the public inputs.
