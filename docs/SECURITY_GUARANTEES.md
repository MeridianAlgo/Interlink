# Security Guarantees & Incident Procedures

> **Status: experimental, unaudited, not for real value.** See
> [SECURITY_REVIEW.md](SECURITY_REVIEW.md) for the full findings list.

## What the system guarantees today

- **Message binding.** Every gateway and the hub recompute the canonical payload hash
  on-chain (source/destination chain and gateway, nonce, action, sender, asset, amount,
  recipient, extra data). A relayer cannot change where funds go after a proof is made.
- **Replay protection** per canonical message hash on EVM and Solana, and per
  `(source_chain, nonce)` on Cosmos.
- **Custody limits (EVM).** Releases can never exceed what was locked for that asset, and
  verified generic calls cannot target the gateway or any custodied asset.
- **Timelocked trust changes.** Replacing the verification key or a remote gateway takes
  2 days on EVM and Solana. Guardian transfer is two-step. Emergency withdrawal requires
  the gateway to be paused.
- **Relayer stake** must be in the real ILINK mint and stays locked for 7 days after the
  relayer's last attestation.

## What it does NOT guarantee

- **Source-chain validity.** The Groth16 circuit proves knowledge of a preimage of the
  commitment, not that the message was emitted on the source chain. Anyone holding the
  proving key can attest any message. This is the main open issue (SECURITY_REVIEW C1).
- **Trusted setup.** Keys come from a local single-party setup. No ceremony has been run.
- **Audits / formal verification.** None has been performed. `relayer/src/formal_verification.rs`
  is a placeholder that returns `true`.
- **Slashing.** Slashing is an admin decision; there are no on-chain fraud proofs. Downtime
  slashing (`staking.rs`, `validator_rewards.rs`) lives in off-chain simulation modules that
  are not connected to the hub program.
- **Cosmos inbound.** Disabled: CosmWasm 1.5 has no BN254 pairing, so there is no verifier.

## Incident procedures

1. **Pause** the affected EVM gateway(s) and the Cosmos gateway (guardian `pause()`).
   The Solana hub has no pause yet; stop the relayers.
2. **Disable routes** with `disableRemoteGateway(chainId)` (immediate on EVM).
3. **Recover funds** only while paused, via `emergencyWithdraw`, to a guardian-controlled
   multisig.
4. **Rotate the verification key** with `proposeVerificationKey` / `apply…` (2-day delay
   by design). Anyone monitoring `VerificationKeyProposed` events sees it coming.

The circuit breaker and rate limiter in `relayer/src/circuitbreaker.rs` and `ratelimit.rs`
are library code; they are **not** wired into the relayer binary.
