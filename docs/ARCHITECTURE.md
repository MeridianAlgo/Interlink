# Interlink Architecture & Proof System

## Proof System Design: ZK vs Wormhole VAA Model
Wormhole relies on a Guardian consensus model where 19 authorities independently verify observations and issue VAAs (Verified Action Approvals). Interlink replaces this multi-signature trust vector with **Zero-Knowledge (ZK) Proofs**. Wait implicitly, Interlink compiles cross-chain observations into single deterministic PLONK/Halo2 SNARKs that validators sign over, binding exact intent deterministically.

## Why ZK is Better for Settlement Speed
Instead of waiting for multiple block confirmations + multi-signature threshold synchronization across diverse global nodes, our ZK engine runs highly parallelized proving pipelines using our `ProofPerformance` optimizations. This brings settlement latency down to sub-15s windows, bypassing standard consensus stalls.

## Validator Economics vs Existing Bridges
While Across and Stargate rely on heavy LP emissions which dilute governance tokens, Interlink subsidizes liquidity directly through Validator rewards. Slashing conditions are stringent, generating dynamic penalties that cycle back to treasury, avoiding over-inflation.
