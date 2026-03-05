# InterLink Core

This is the core Rust library for the InterLink Protocol, a zero-knowledge atomic cross-chain interoperability framework.

## Status

**🚧 UNDER ACTIVE DEVELOPMENT 🚧**

This codebase is a work-in-progress implementation of the architecture described in the [InterLink Research Paper](../RESEARCH.tex).

## Modules

- `circuit`: Halo2 circuit definitions for verifying cross-chain state.
- `relayer`: The off-chain agent responsible for listening to events and generating proofs.
- `network`: P2P and RPC networking primitives.

## Usage

Currently, this library serves as the foundational scaffolding. 

```rust
// Example usage (future)
use interlink_core::relayer::Relayer;

#[tokio::main]
async fn main() {
    let relayer = Relayer::new(config);
    relayer.run().await.unwrap();
}
```
