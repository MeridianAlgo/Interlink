# contracts/solana/src

Anchor program source code for the Solana hub.

## Key concepts

- **StateRegistry**
  - Stores admin, fee rate, and the latest processed cross-chain sequence.
- **submit_proof**
  - Relayers submit proof bytes + public inputs, and the program records that a sequence is finalized.
- **buy_back_and_burn**
  - Burns a portion of the fee token supply according to the protocol design.

## Verification note

`verify_snark_commitment` is currently a lightweight commitment-consistency check meant to represent the intended verification path.
