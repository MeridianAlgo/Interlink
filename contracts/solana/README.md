# contracts/solana

Solana program (Anchor) that acts as the InterLink hub/verification side.

## In plain terms

This program is the on-chain place where relayers submit proofs and where InterLink keeps track of which cross-chain message sequences have been processed.

## Layout

- `src/`
  - Anchor program entrypoints and account types.
- `Cargo.toml` / `Cargo.lock`
  - Rust/Anchor build configuration.

## What it does today

- Initializes a `StateRegistry` account (admin + fee settings + last processed sequence)
- Accepts `submit_proof` transactions and advances the processed sequence
- Includes a `buy_back_and_burn` instruction for fee burning mechanics
