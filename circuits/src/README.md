# circuits/src

Rust source code for the `circuits` crate.

## What you’ll find here

- `lib.rs`
  - Crate entry point; exports the available circuit modules.
- `merkle.rs`
  - A Halo2 circuit that walks a Merkle path and exposes the resulting root as a public input.

## Typical usage

This code is primarily consumed by other crates (for example `interlink-core/` and tooling in `relayer/`) rather than being run as a standalone binary.
