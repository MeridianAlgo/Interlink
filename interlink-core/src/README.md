# interlink-core/src

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
