# InterLink Security Review

Scope: `contracts/evm`, `contracts/solana/interlink-hub`, `contracts/solana/src` (legacy, now
removed), `contracts/cosmos`, `circuits`, `interlink-core`, `relayer`, `sdk`, and the docs.
Status reflects this branch.

**Bottom line:** the contract-level bugs below are fixed and tested, but the bridge is still
**not safe to hold real value**. The ZK proof does not attest to anything that happened on
the source chain (finding C1). Until that changes, security reduces to "whoever holds the
proving key", which is weaker than a 1-of-1 multisig because nobody can see who that is.

| Severity | Found | Fixed | Open |
|---|---|---|---|
| Critical | 7 | 6 | 1 (C1, architectural) |
| High | 13 | 13 | — (operational follow-ups noted) |
| Medium | 11 | 10 | 1 (M11, economic design) |
| Low / Info | 11 | 10 + 1 mitigated | — |

---

## Critical

### C1. The proof does not prove the message happened — OPEN (architectural)
`relayer/src/prover.rs` proves knowledge of `(m, s)` with `(m + rc)^5 + s = C`. Every verifier
computes `C` itself from the caller-supplied payload hash, so the witness is just
`m = hash mod r`. Anyone with the proving key can produce a valid proof for **any** payload
hash, including one for a deposit that never happened. Nothing checks block headers,
consensus signatures or receipt inclusion.

Aggravating factors:
- The trusted setup was a fresh single-party `circuit_specific_setup` on every relayer start.
  Whoever runs it could also keep the toxic waste.
- The Halo2 `circuits/` crate is not used by the relayer. Its "sync committee" circuit skips
  BLS signature verification ("simplified model") and never constrains the quorum flag
  against the threshold, so even wired in it would prove nothing about consensus.

What a real fix needs: a circuit (or zkVM program, e.g. SP1 / RISC Zero) that verifies source
consensus (Ethereum sync-committee BLS signatures, or Tendermint validator signatures) and a
Merkle-Patricia inclusion proof of the `MessagePublished` receipt, exposing the canonical
payload hash as a public output. Use a multi-party ceremony or a universal setup. Until
then, an interim M-of-N attestation (independent signers checking the source chain) is more
honest than a proof that attests nothing.

The canonical message format introduced below is designed to be that circuit's public
output, so the contracts won't need to change again.

### C2. Hub minted any amount to anyone — FIXED
`verify_and_mint` took `amount`, `recipient_token_account` and `token_mint` from the caller.
The proof covered only `(payload_hash, sequence)`. Any staked relayer could take any valid
proof and mint `u64::MAX` of any hub-controlled mint to themselves.
**Fix:** the hub recomputes the canonical payload hash on-chain from the message fields
(`canonical_hash`). Execution is permissionless but bound to that hash: a `WrappedAsset`
registry fixes the mint per `(source chain, asset)`, the recipient token account must be
owned by the hashed recipient, and amounts are decimal-scaled from the hashed amount.

### C3. Hub swap drained the pool — FIXED
Same flaw in `process_cross_chain_swap`: `amount_in`, `min_amount_out` and recipient were
unbound. **Fix:** swap parameters are bound through `extraHash`. Pool vaults are program PDAs.

### C4. Cosmos accepted any 256 non-zero bytes as a proof — FIXED
`verify_proof` checked only length and "not all zeros". Recipient, amount and denom were
caller-supplied. The contract's own test `test_execute_verified_message` released funds with
`[0xAB; 256]`. **Fix:** inbound execution fails closed (`VerifierUnavailable`) until a real
verifier exists (CosmWasm 1.5 has no BN254 pairing host function). All inbound fields are
hashed on-chain for when one does. Regression test added.

### C5. Relayer stake could be a worthless token — FIXED
`stake` accepted any mint as `ilink_mint`, so a relayer could mint 100,000 of a junk token and
pass `MIN_STAKE_AMOUNT`. **Fix:** the ILINK mint is pinned in `StateRegistry` at init, and
`stake`, `slash_relayer` and `buy_back_and_burn` all check it.

### C6. Legacy Solana program had a forgeable verifier — FIXED (deleted)
`contracts/solana/src/lib.rs` checks `e(A,B)·e(C,−G2) = 1` with no verification key and no
public inputs. `A = C = s·G1, B = G2` passes for any `s`. `sequence > processed` also lets
anyone submit `u64::MAX` and brick it. It's still configured in `contracts/solana/Anchor.toml`.
**Fix:** `contracts/solana/src` and its Anchor/Cargo config were deleted.

### C7. EVM gateway made arbitrary calls from the custody contract — FIXED
`executeVerifiedMessage` did `target.call(payload)` from the contract holding every locked
token. With C1, any proving-key holder could call `token.transfer(attacker, balance)`. This is
the pattern behind the 2021 Poly Network hack.
**Fix:** verified calls can't target the gateway or any asset it custodies. Releasing custody
goes through a dedicated `releaseTokens` path that can never exceed `totalLocked[token]`.

---

## High (all fixed)

| # | Issue | Fix |
|---|---|---|
| H1 | EVM proof not bound to source chain, destination chain or gateway: one proof replays on every deployment sharing a VK. | Canonical message binds both chain IDs and both gateways. |
| H2 | EVM public input was `keccak(...) mod r`, but the prover's circuit outputs `(h+rc)^5+seq`. Relayer proofs could never verify on EVM. | `commitment()` matches the prover. Verified by running the compiled gateway in revm with a real arkworks proof. |
| H3 | Nonce collisions: EVM `executedNonces[nonce]` and the hub's single `next_sequence` ignored the source chain. | Replay protection keyed by canonical payload hash. |
| H4 | Hub required strict global sequencing. One stuck message (e.g. slippage) halted the bridge forever, and any relayer could consume a mint's sequence by submitting it through `submit_proof`. | Independent per-message attestation records; the action type is part of the hash. |
| H5 | Hub `field_mul` used byte-wise shift-subtract (~3,900 32-byte passes per multiplication, measured). The three multiplications alone exceed Solana's 1.4M CU cap even at 4 CU/step, so `submit_proof` could not succeed on-chain. | 4×64-bit Montgomery arithmetic, checked against arkworks on 2,000 random inputs. |
| H6 | Relayer ran a new random setup on every start, and `export-vk` ran its own. The on-chain VK never matched the running relayer. | Persistent proving key (`PROVING_KEY_PATH`) shared by relayer and `export-vk`. |
| H7 | No reorg safety: `removed` logs ignored; finality was a block-depth count (10 blocks on L2s, about 2.5s on Arbitrum; real EVM chain IDs from `.env` fell through to the default); no canonical re-check. A reorged-out deposit would be minted. | Wait for the node's `finalized` tag, then re-fetch the receipt and check block hash, status, gateway and payload hash. Non-canonical events are never retried. |
| H8 | Unchecked admin power: immutable single-key EVM guardian, instant VK replacement (EVM and Solana), `emergencyWithdraw` at any time, single-step Cosmos guardian. | Two-step guardian transfer; 2-day timelock on VK and remote-gateway changes; emergency withdraw only while paused. Operationally, the guardian should still be a multisig. |
| H9 | No unbonding: stake, attest fraud and unstake in one transaction. | 7-day unbonding from last activity. Slashing is still admin discretion; there are no on-chain fraud proofs. |
| H10 | Hub `initialize` could be front-run (first caller becomes admin). | Restricted to the program upgrade authority. |
| H11 | EVM recipients were 20-byte `address`, but the destination is Solana (32 bytes). `sendCrossChainMessage` had no recipient at all. The relayer parsed the payload *length* as the amount. | `bytes32` recipients; a unified `MessagePublished` event carrying every canonical field. |
| H12 | Cosmos deposits summed amounts across denoms (1 ATOM + 1000 junk = "1001") with no denom recorded. | Exactly one coin per message; denom bound into the hash. |
| H13 | 18-decimal `uint256` amounts vs SPL `u64`: anything over 18.4 ETH was unrepresentable, with no scaling. | `decimals_shift` per wrapped asset; EVM caps amounts at `uint128`. |

---

## Medium

| # | Issue | Status |
|---|---|---|
| M1 | USDT-style tokens reverted; fee-on-transfer tokens recorded the requested amount (insolvency); ETH sent with an ERC-20 deposit was trapped. | FIXED: safe transfer helpers, balance-delta accounting, `msg.value == 0` for tokens. |
| M2 | No destination allowlist: deposits to unsupported chains were locked forever. | FIXED: the remote-gateway allowlist means deposits can only target configured chains. |
| M3 | Anyone could squat the canonical pool PDA with vaults they controlled. `fee_rate_bps > 10000` made every swap panic. | FIXED: admin-only pools, PDA vaults, fee cap. |
| M4 | `buy_back_and_burn` was callable by anyone, so `total_burned` could be inflated. | FIXED: admin only. |
| M5 | `reduce_mod_scalar_field` stopped after 4 subtractions; ~5.5% of hashes need 5. | FIXED. |
| M6 | Pipeline: the prover permit was held through the ~13 min finality wait (8 CPUs = 8 events per 13 min); events were dropped after one quick retry; dedup marked events before success; no backfill after WS reconnect; the reconnect counter never reset; "settlement complete" was logged before confirmation. | FIXED. |
| M7 | Relayer only called `submit_proof`, so no transfer ever completed end to end. | FIXED: the relayer attests and then calls `execute_transfer` / `execute_swap`; the hub creates the recipient's token account if needed. |
| M8 | Unauthenticated `POST /webhooks/register` and `DELETE /webhooks/{id}` on the HTTP API, bound to `0.0.0.0` in Docker (SSRF/abuse). | FIXED: all `/webhooks*` routes need `Authorization: Bearer $API_TOKEN` (disabled when unset, constant-time compare); default bind is localhost outside Docker. |
| M9 | `ring` 0.16 (RUSTSEC-2025-0009/0010) in the tree via `ethers-providers → jsonwebtoken`; the `Cargo.toml` "override" didn't apply to transitive deps. | FIXED: ethers-rs removed from the runtime (HTTP-polling listener and small keccak/hex/RPC helpers instead) and the stale `interlink-core` relayer deleted. `ring` is no longer in the dependency tree, and the advisory ignores were removed. |
| M10 | No return path: no burn-for-withdrawal on Solana; NFTs lock with no destination support; failed swaps have no refund. | FIXED: hub `burn_for_withdrawal` + relayer hub→EVM delivery (`releaseTokens`); `claim_swap_as_wrapped` lets a swap recipient settle in wrapped tokens; `lockNFT` is off by default (`nftBridgingEnabled`) until a destination supports NFTs. |
| M11 | Pool `reserve_a` is virtual (input tokens stay on the source chain) and swap fees vanish. | OPEN (economic design). |

## Low / Info

| # | Issue | Status |
|---|---|---|
| L1 | Docs overstate security: claimed Trail of Bits/PSE audit, but `formal_verification.rs` returns `true` with a placeholder IPFS link; the circuit breaker and rate limiter aren't wired into the relayer; README says "trustless math", "<30s settlement", Halo2. | `SECURITY_GUARANTEES.md` rewritten; README status notice added. |
| L2 | ~45 of 56 relayer modules are standalone stubs the binary never uses (e.g. `check_wormhole_parity() -> true`). | FIXED: `lib.rs` and the README separate production from experimental modules; the fake functions and the `formal_verification` mock were removed. |
| L3 | `forge build` failed: `DeployAndInit.s.sol` called a non-existent `setVerificationKey(bytes)` and had a non-ASCII string; `DeployL2Verify` called non-existent `guardian()` / `nonce()`. | FIXED. |
| L4 | Files `.gitignore` excludes are still tracked: `.claude/settings.local.json`, and `interlink-hub/out.log` (stale log from another project path, leaks a local username). | FIXED: untracked (kept locally). |
| L5 | Hand-rolled SHA-256 in the Cosmos contract. | FIXED (`sha2`). |
| L6 | `@coral-xyz/anchor ^0.32.1` (TS) vs `anchor-lang 0.30.1` (Rust). | FIXED: aligned to `^0.30.1`, lockfile regenerated. |
| L7 | EVM verifier didn't range-check proof coordinates. | FIXED. |
| L8 | Chain IDs mix custom values (`2` = Solana) with EIP-155 (`2` is also Expanse). | Mitigated: gateway addresses are hashed too. |
| L9 | `interlink-core` held a second, older relayer producing Halo2 proofs the Groth16 verifiers can't check, which silently proved the nonce instead of the hash for ~81% of hashes. | FIXED: removed; `interlink-core` is now just the Poseidon chip and types. |
| L10 | The TS SDK's `transfer()` / `swap()` / `getTransferStatus()` called relayer endpoints that didn't exist; a relayer can't sign a user's deposit. | FIXED: the SDK builds and sends gateway transactions with the user's signer (incl. ERC-20 approval), tracks by payload hash, and the relayer serves `GET /transfers/{payloadHash}` from the hub's on-chain record. |
| L11 | The SDK used chain ID `900` for Solana; the contracts and relayer use `2`, so SDK-built transfers would fail with "unsupported destination". | FIXED: `ChainId.Solana = 2`. |

---

## Canonical message format

All gateways and the relayer hash outbound messages identically:

```
payloadHash = keccak256(abi.encode(
    keccak256("InterlinkMessage(v1)"),
    uint64 srcChain,  bytes32 srcGateway,
    uint64 dstChain,  bytes32 dstGateway,
    uint64 nonce,     uint8   action,      // 1 transfer, 2 swap, 3 NFT, 4 call
    bytes32 sender,   bytes32 asset,
    uint256 amount,   bytes32 recipient,   // amount ≤ 2^128−1
    bytes32 extraHash))                    // keccak(payload) / swap params / tokenId

commitment  = ((payloadHash mod r) + keccak256("interlink_v1_domain") mod r)^5 + nonce   (mod r)
```

The same vector (`0x07b0f8b8…09bd`) is pinned in the Foundry tests, the relayer unit tests,
and was checked against the compiled gateway in an EVM.

## Verification performed

- **Cosmos:** 14 unit tests pass, including regression tests for C4, H12 and per-chain replay.
- **Solana hub:** 9 host tests pass, including a real arkworks Groth16 proof verified through
  the on-chain verifier (and rejected after tampering), Montgomery arithmetic vs arkworks, and
  canonical-hash binding of every field. Not run: an SBF build and the Anchor TS tests (the
  local toolchain couldn't install platform-tools); on-chain CU usage is unmeasured.
- **EVM:** compiled with solc 0.8.28 (`via_ir`; contract, tests, all scripts). The compiled
  gateway was executed in revm: a real proof releases funds; tampered amounts, corrupted
  proofs, replays, gateway-targeted calls and live emergency withdrawals are rejected;
  ~334k gas per release. Not run: `forge test` (Foundry not installed).
- **Relayer / workspace:** `cargo test --workspace` passes natively (relayer lib 445,
  integration 19, security 32, checklist 9, circuits 10, interlink-core 5); clippy
  (`-D warnings`) and `cargo fmt --check` are clean. Highlights: canonical-hash vector shared
  with EVM, PDA/ATA derivation matching `solana find-program-derived-address`, EIP-1559
  signing byte-identical to ethers-core, hub-event parsing that rejects lookalike logs from
  other programs, webhook auth, and proving-key persistence.
- **Relayer → EVM end to end:** the relayer's own `ProverEngine` proof and
  `release_calldata` released funds through the compiled gateway in revm; replay and
  tampered amounts reverted.
- **SDK:** 31 Jest tests pass (gateway calldata built, sent through a mock signer and
  decoded back with a payload-hash check; shared hash vector); `tsc` clean; coverage
  above thresholds.
- **Not run here:** `forge test`, the Anchor TS tests, an SBF build / on-chain CU
  measurement, and the relayer against live chains.
- **Local build note:** with `ring` gone, the workspace now builds natively on ARM64 Windows
  (previously it needed clang plus the x86_64 target).
