# contracts

This folder contains the **on-chain smart contract code** for InterLink, organized by ecosystem.

## In plain terms

InterLink is cross-chain, so it needs a “gateway” or “hub” program/contract on each chain family. This folder is where those chain-specific pieces live.

## Subfolders

- `evm/`
  - Solidity contracts for Ethereum and other EVM chains.
- `solana/`
  - Anchor program for the Solana “hub”/verification side.
- `cosmos/`
  - Rust crate placeholder for a Cosmos/wasm-style gateway (currently minimal scaffold).
