# contracts/solana

The Solana side of InterLink is the Anchor program in [`interlink-hub/`](interlink-hub/).

An older standalone program used to live directly in this directory (`src/lib.rs`).
It was removed because its "Groth16" check (`e(A,B)·e(C,−G2) = 1`) used no
verification key or public inputs and could be forged by anyone. See
[docs/SECURITY_REVIEW.md](../../docs/SECURITY_REVIEW.md) (finding C6).

## interlink-hub in brief

- `submit_proof`: a staked relayer attests a fully specified cross-chain message;
  the payload hash is recomputed on-chain and the Groth16 proof is checked.
- `execute_transfer` / `execute_swap`: permissionless execution of an attested
  message (mint wrapped tokens, or swap through the hub pool).
- `burn_for_withdrawal`: burn wrapped tokens and emit an outbound message that
  releases the original asset on the source chain.
- Staking with a 7-day unbonding period, timelocked verification-key rotation,
  admin-only pools and wrapped-asset registry.

```bash
cd interlink-hub
cargo test          # host tests, incl. a real Groth16 proof through the verifier
anchor test         # integration tests (requires Anchor CLI + local validator)
```
