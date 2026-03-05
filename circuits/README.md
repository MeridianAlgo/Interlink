# circuits

This crate contains the **zero-knowledge circuit code** used by InterLink.

## In plain terms

When InterLink says “we prove something happened on another chain using math”, this folder is where that “math” is implemented as circuits (using Halo2).

## What’s inside

- `src/`
  - Circuit implementations (currently focused on Merkle-path style proofs and hashing).
- `tests/`
  - Circuit tests (Halo2 `MockProver` style tests).

## How it relates to the rest of the repo

- Depends on `interlink-core/` for shared circuit primitives (e.g. hashing chips/config).
- Used by relayer/core logic to generate and/or verify proofs.
