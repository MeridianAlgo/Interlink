# relayer

This crate builds the **relayer node** for InterLink.

## In plain terms

Relayers are off-chain workers that:

- Watch a source chain (e.g. an EVM chain) for gateway events
- Build the required proof(s)
- Submit proofs and message data to the hub chain (Solana in this repo)

## Layout

- `src/`
  - The relayer binary entry point.
- `Cargo.toml`
  - Dependency config. This crate depends on `interlink-core/`.
