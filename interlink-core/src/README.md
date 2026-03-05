# interlink-core/src

> **🚨 IMPORTANT – PROVER CONSISTENCY REQUIREMENT 🚨**
> 
> The relayer's Halo2 prover MUST use the exact same `interlink_v1_domain` salt when generating proofs. This is strictly required to match the updated Solidity input binding logic in `InterlinkGateway.sol:L175-180`. Ensure the entire pipeline (prover -> relayer -> on-chain verification) uses consistent domain separation to prevent proof mismatches.

Rust source code for the `interlink-core` crate.

## In plain terms

This is the shared “core logic” for InterLink: common types, networking/protocol glue, and relayer/proof plumbing that other crates build on.

## Key files

- `lib.rs`
  - Public module exports and core traits/types (e.g., `Message`) and `InterlinkError`.
- `circuit.rs`
  - Circuit-related primitives used by higher-level circuits (for example Poseidon hashing chips/config).
- `relayer.rs`
  - Core relayer logic and configuration types.
- `network.rs`
  - Networking primitives used by the system.
- `main.rs`
  - A small runnable entry point wiring up a relayer config (useful for local/dev testing).
