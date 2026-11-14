# InterLink

> **Status: experimental and unaudited. Do not use with real funds.**
> The current Groth16 circuit does not yet verify source-chain consensus or receipt
> inclusion, so proofs attest to message *format*, not to the message having happened.
> See [docs/SECURITY_REVIEW.md](docs/SECURITY_REVIEW.md) for findings and fixes, and
> [docs/SECURITY_GUARANTEES.md](docs/SECURITY_GUARANTEES.md) for what is and isn't guaranteed.
> The "goal" column in the comparison table describes targets, not the current build.

## Overview

InterLink is a cross-chain bridge in development. Its goal is ZK-proven cross-chain messages:
a Groth16 (BN254) proof per message, verified on-chain against a canonical payload hash that
every gateway recomputes itself.

<p align="center">
  <img src="InterLink.png" alt="InterLink Architecture" width="80%">
</p>

The protocol follows a **Hub-and-Spoke** architecture. Solana is the settlement and verification
hub. EVM chains (Ethereum, Optimism, Arbitrum, Base, Polygon) and Cosmos chains are spoke gateways.

> **Canonical message format.** EVM gateways, the Solana hub, the relayer and the SDK all hash
> messages identically (both chain IDs, both gateway addresses, nonce, action, sender, asset,
> amount, recipient, extra data). A shared test vector pins this across all four
> implementations. See [docs/SECURITY_REVIEW.md](docs/SECURITY_REVIEW.md#canonical-message-format).

---

## Why InterLink?

| Feature | Wormhole | Stargate | Across | **InterLink: goal** | **InterLink: today** |
|---|---|---|---|---|---|
| **Verification** | Guardian multisig (13/19) | DVNs | Optimistic oracle | ZK light-client proofs | Groth16 proof bound to the message, but the circuit does not yet verify source-chain consensus (trusts the proving-key holder) |
| **Settlement** | 2-15 min | 1-2 min | 5-60 min | Source finality + seconds | Source finality (~13 min on Ethereum) + proof + hub tx |
| **Fees (Tier 1)** | $1-20 VAA | $0.50-5 | 0.25-1% | 0% under $1k | Fee model exists in `fee.rs`; not enforced on-chain |
| **Return path** | Yes | Yes | Yes | Any → any | EVM → Solana mint/swap, Solana → EVM withdrawal |
| **Governance** | Guardian multisig | Off-chain | UMA | On-chain DAO | Guardian/admin keys with timelocks (simulation in `governance.rs`) |
| **Audits** | Multiple | Multiple | Multiple | Required before mainnet | None yet |

---

## Project Architecture

```
interlink/
├── circuits/                    # Halo2 research circuits (not used by the relayer)
│   └── src/{merkle,consensus,recursion,config}.rs
├── contracts/
│   ├── evm/
│   │   ├── src/InterlinkGateway.sol    # EVM spoke gateway (Solidity 0.8.28)
│   │   ├── script/                     # Deploy, DeployAndInit, DeployL2
│   │   └── test/InterlinkGateway.t.sol # Foundry tests incl. a real Groth16 proof
│   ├── solana/interlink-hub/           # Anchor hub program + TS integration tests
│   └── cosmos/                         # CosmWasm gateway (inbound disabled: no verifier)
├── relayer/
│   ├── src/
│   │   ├── main.rs             # Both pipelines: EVM → hub and hub → EVM
│   │   ├── lib.rs              # Production modules vs experimental modules
│   │   └── ...
│   ├── tests/                  # security.rs, integration.rs, checklist_features.rs
│   └── src/bin/                # export-vk, benchmark, load-test
├── interlink-core/             # Halo2 Poseidon chip + payload types
├── sdk/                        # @interlink/sdk (TypeScript, ethers v6)
└── docs/                       # SECURITY_REVIEW.md, SECURITY_GUARANTEES.md, ...
```

---

## Relayer Module Reference

Only the **Core Bridge Pipeline** modules run in the `relayer` binary. Every other module
below is standalone library or simulation code. It is tested, but it is not wired into the
running relayer and gives no runtime guarantees.

### Core Bridge Pipeline

| Module | File | Description |
|---|---|---|
| **listener** | `listener.rs` | Polls `eth_getLogs` over a block cursor for `MessagePublished`. Verifies the RPC chain ID, decodes strictly and drops logs whose fields don't reproduce the payload hash. |
| **finality** | `finality.rs` | Waits for the node's `finalized` block tag, then re-checks the receipt (block hash, status, log). Reorged events are never relayed. |
| **prover** | `prover.rs` | Groth16 (arkworks, BN254) proofs with a persistent proving key shared with `export-vk`. |
| **submitter** | `submitter.rs` | Hub `submit_proof` (attest) and `execute_transfer` / `execute_swap`, with confirmation tracking and idempotency. |
| **solana_tx** | `solana_tx.rs` | Legacy message compiler, PDA/ATA derivation (checked against the Solana CLI), Anchor discriminators. |
| **hub_outbound** | `hub_outbound.rs` | Reads finalized hub withdrawals (`OutboundMessageEvent`), accepting event logs only while the hub program is executing. |
| **evm_tx / evm_submitter** | `evm_tx.rs`, `evm_submitter.rs` | EIP-1559 signing (k256, cross-checked against ethers-core) and delivery via `releaseTokens`. |
| **events / evm** | `events.rs`, `evm.rs` | Canonical message type and minimal keccak/hex/JSON-RPC helpers (no ethers-rs at runtime). |
| **http_api** | `http_api.rs` | `/health`, `/quote`, `/compare`, `/metrics`, `/transfers/{payloadHash}` (hub on-chain status). Webhook routes need `API_TOKEN`. |
| **batch / metrics / webhook / fee / gas** | | Batching, Prometheus metrics, webhook registry, fee and gas models. |

### Economics & Fees (experimental)

| Module | File | Description |
|---|---|---|
| **fee** | `fee.rs` | Dynamic fee tier engine. Zero ($0-1k), Standard 0.05% ($1k-100k), Institutional 0.01% ($100k-10M), OTC 0% ($10M+). Beats Wormhole VAA fees and Across 0.25-1%. |
| **gas** | `gas.rs` | Cross-chain gas cost estimation with amortized proof generation overhead. `CostComparison` benchmarks against Wormhole/Stargate/Across fee models. |
| **mev** | `mev.rs` | MEV capture revenue modeling. Computes LP breakeven analysis — minimum daily volume for zero-fee Tier 1 sustainability. |
| **amm** | `amm.rs` | Constant-product AMM (x*y=k) for bridge vault liquidity. LP fee split: 0.25% to LPs + 0.05% protocol. 5% max price-impact guard. APY tracking with real-time yield computation. |

### Governance & Token Economics

| Module | File | Description |
|---|---|---|
| **governance** | `governance.rs` | DAO governance: 1B $INTERLINK supply (40% community, 30% team, 30% treasury). Proposal creation (100k threshold), 7-day voting, 2-day timelock, treasury disbursement. 6 proposal types: UpdateFees, AddChain, UpdateValidatorSet, TreasuryAllocation, UpdateParameters, Text. |
| **staking** | `staking.rs` | $INTERLINK staking with Bronze/Silver/Gold/Platinum tiers (1k-1M token thresholds). 10-100% fee discounts, 20%→5% APY taper, validator eligibility, configurable slashing (5% downtime, 50% byzantine). |
| **validator_rewards** | `validator_rewards.rs` | 10% bridge fee distribution to validators, weighted by stake × uptime. Minimum 90% uptime requirement. 5% bonus for perfect uptime. Epoch-based heartbeat tracking. |
| **vesting** | `vesting.rs` | Token vesting schedules: team (4yr vest, 1yr cliff), advisors (2yr vest, 6mo cliff), treasury (3yr linear, no cliff). Per-beneficiary tracking with revocation support for departed team members. |
| **liquidity_mining** | `liquidity_mining.rs` | LP incentive program: 10M $INTERLINK over 26 epochs (6 months). Early-bird 2x boost (epochs 1-4), loyalty 1.5x boost (≥4 consecutive epochs), 25/75 immediate/vesting split, 24h anti-gaming minimum deposit. |
| **bounty** | `bounty.rs` | Bug bounty lifecycle: Critical $100k-$500k, High $10k-$100k, Medium $1k-$10k, Low $100-$1k. SLA response times: Critical 4h, High 24h, Medium 72h, Low 168h. Full submission→triage→confirm→pay pipeline. |

### Security & Resilience

| Module | File | Description |
|---|---|---|
| **multisig** | `multisig.rs` | Threshold multi-signature (3-of-5 Ed25519). Validator bundle creation, signature aggregation, and verification. Upgradeable to 13-of-19 (Wormhole parity) via governance vote. |
| **circuitbreaker** | `circuitbreaker.rs` | Auto-pause on anomaly: ≥5 consecutive proof failures, ≥3 settlement failures, $1M TVL drain in 5min. Guardian emergency pause. Auto-recovery after 5min cooldown (non-guardian pauses only). |
| **retry** | `retry.rs` | Exponential backoff with jitter, per-chain retry policies (Ethereum: 2s base/8 retries, Solana: 200ms/4 retries, L2: 500ms/5 retries). Circuit-breaker-aware. Dead-letter queue (1k capacity) for manual replay of failed ops. |
| **ratelimit** | `ratelimit.rs` | Token-bucket rate limiting: Free 100 req/min, Pro 1000 req/min, Enterprise custom/unlimited. 2x burst allowance. Standard `X-RateLimit-*` response headers. |

### Routing & Simulation

| Module | File | Description |
|---|---|---|
| **intent** | `intent.rs` | Intent-based routing: user specifies desired output ("1 ETH → ≥2900 USDC on Solana"), solver finds optimal path. Route types: DirectBridge, BridgeAndSwap, MultiHop, SameChainSwap. 2% max slippage, 3-hop max, 5-min expiry. |
| **swap_routing** | `swap_routing.rs` | Multi-DEX aggregation: Uniswap V3, 1inch, 0x, SushiSwap, Curve (EVM) and Jupiter, Raydium, Orca (Solana). Best-rate selection with fallback routing, slippage tracking, and per-source reliability analytics. |
| **simulator** | `simulator.rs` | Transfer dry-run with 10 pre-flight checks: circuit breaker, source/dest chain support, cross-chain validation, amount, fee calculation, liquidity, rate limit, estimated time, route type. No on-chain submission. |
| **wrapped** | `wrapped.rs` | Canonical wrapped asset registry: wETH, wSOL, wMATIC across 6 chains (Ethereum, Optimism, Polygon, Arbitrum, Base, Solana). Deterministic resolve-or-no-wrap — no manual attestation step. |

### NFT & Atomic Settlement

| Module | File | Description |
|---|---|---|
| **nft_bridge** | `nft_bridge.rs` | Cross-chain NFT bridging with lock-mint-burn model. Full metadata preservation (name, traits, IPFS/Arweave URIs, on-chain SVG). EIP-2981 royalty forwarding. Wrapped contract registry. 24h lock timeout with auto-expiry. |
| **atomic** | `atomic.rs` | Two-phase commit for cross-chain settlement. Escrow state machine: Prepared → ProofReady → Committed → Finalized, with timeout-based rollback. Grace period before forced rollback. Guarantees: no fund loss, no double-spend. |

### Enterprise Features

| Module | File | Description |
|---|---|---|
| **enterprise** | `enterprise.rs` | Institutional bridge controls: address whitelisting, per-tx/daily/monthly spend limits ($500k/$1M/$10M defaults), N-of-M multi-approver workflows for large transfers ($100k+), configurable settlement hold period, auto-resetting spend counters. |

### Observability & Compliance

| Module | File | Description |
|---|---|---|
| **metrics** | `metrics.rs` | Prometheus-compatible metrics: proof gen time (p50/p95/p99), settlement time, verification time (>500ms alert), per-chain finality lag, RPC latency, TVL tracking, daily/cumulative volume, validator uptime %, daily transfers, unique users, top corridors. JSON + Prometheus text export. |
| **sla** | `sla.rs` | SLA monitoring: 99.9% uptime target, <60s settlement p99, <500ms API response p99. Sliding window (10k samples), automatic breach detection and reporting. |
| **audit_trail** | `audit_trail.rs` | Append-only compliance log with SHA-256 hash-chain integrity. Indexed by sender, receiver, corridor, time range. CSV + JSON export for regulatory reporting. Tamper detection via `verify_integrity()`. |
| **webhook** | `webhook.rs` | Real-time push notifications: transfer start, pending, completed, failed. 3-attempt exponential backoff. Auto-disable after 10 consecutive failures. |
| **http_api** | `http_api.rs` | REST API: `GET /health`, `GET /metrics`, `GET /quote`, `POST /simulate`. Prometheus endpoint for Grafana integration. |

### Research & Integration

| Module | File | Description |
|---|---|---|
| **proof_perf** | `proof_perf.rs` | Profiling constraints minimizing <50ms proving latencies targeting advanced boundary metrics across recursive polynomials. |
| **network_opt** | `network_opt.rs` | Replacement mechanisms for standardized websocket logic using native QUIC P2P connections avoiding network propagation stalls. |
| **benchmarks** | `benchmarks.rs` | Hard-coded throughput thresholds verifying zero-cost boundaries and competitive cross-chain latencies vs. major competitors. |
| **sdk_experience** | `sdk_experience.rs` | SDK execution pathways optimizing developer-facing integration latencies strictly beneath 500ms bounds. |
| **xchain_messaging** | `xchain_messaging.rs` | ZK-enabled cross-chain arbitrary messaging standard bypassing classical IBC limitations for EVM interoperability. |
| **zk_research** | `zk_research.rs` | Applied mathematics bounds minimizing base-constraint polynomial complexities across recursive 16-core parallel setups. |
| **byzantine_bridge** | `byzantine_bridge.rs` | Enforced distributed threshold validations scaling safely up to f < n/3 thresholds avoiding network-split vulnerabilities. |
| **defi_integration** | `defi_integration.rs` | Native integrations simulating deep intent execution paths directly into Compound and AAVE pools cross-chain seamlessly. |

---

## Testing Framework

Test counts as of this branch. "Run" means executed in the latest review; the Foundry and
Anchor suites need their CLIs and were compiled but not executed there.

### Full Test Suite Summary

| Layer | Tool | Tests | Status |
|---|---|---|---|
| Relayer lib (unit) | `cargo test -p relayer --lib` | 445 | Run, passing |
| Relayer security | `cargo test --test security` | 32 | Run, passing |
| Relayer integration | `cargo test --test integration` | 19 | Run, passing |
| Relayer checklist e2e | `cargo test --test checklist_features` | 9 | Run, passing |
| Circuits / interlink-core | `cargo test -p circuits -p interlink-core` | 15 | Run, passing |
| Solana hub (host, incl. real Groth16 proof) | `cargo test` in `contracts/solana/interlink-hub` | 11 | Run, passing |
| Cosmos gateway | `cargo test` in `contracts/cosmos` | 14 | Run, passing |
| SDK | `npm test` in `sdk` | 31 | Run, passing |
| EVM gateway (incl. real Groth16 proof) | `forge test` | 42 | Compiled with solc 0.8.28; not executed (no Foundry) |
| Solana hub integration | `anchor test` | 5 | Not executed (no Anchor CLI) |

### Security Test Coverage (32 tests)

| Module | Tests | What's Validated |
|---|---|---|
| `sequence_binding` | 3 | Double-spend prevention, commitment binding, replay rejection |
| `byzantine` | 6 | Threshold enforcement (2/5, 3/5), duplicate signer detection, empty bundle rejection |
| `malformed_input` | 5 | Invalid proof data, oversized payloads, zero-amount transfers |
| `amm_manipulation` | 4 | Price impact guard, zero-amount swap rejection, slippage protection |
| `governance_attack` | 5 | Insufficient stake voting, double-vote prevention, premature execution |
| `rate_limit` | 4 | Free tier enforcement, Pro tier scaling, Enterprise unlimited, burst handling |
| `webhook_dos` | 3 | Auto-disable after failures, delivery tracking, registration limits |

### Running Tests

```bash
# All Rust tests (circuits + relayer)
cargo test --workspace

# Relayer unit tests only
cargo test --lib

# Security tests
cargo test --test security

# Integration tests
cargo test --test integration

# EVM Solidity (requires Foundry)
cd contracts/evm && forge test -vv

# Solana devnet (requires SOL balance + Anchor CLI)
cd contracts/solana/interlink-hub
ANCHOR_PROVIDER_URL=https://api.devnet.solana.com \
ANCHOR_WALLET=~/.config/solana/id.json \
yarn run ts-mocha -p ./tsconfig.json -t 1000000 "tests/**/*.ts"

# Performance benchmarks
cargo run --bin benchmark -- --iterations 1000

# Load testing
cargo run --bin load_test -- --concurrency 50 --total 1000
```

---

## Specialized Circuits (`circuits/`)

Advanced ZK primitives built on Halo2 over BN254:

| Circuit | Purpose | Details |
|---|---|---|
| **Merkle Circuit** | Poseidon Merkle inclusion proofs | Binary tree traversal with quintic S-box hash gates |
| **SyncCommitteeCircuit** | Ethereum beacon chain finality | BLS-aggregate participation accumulation, ≥342/512 quorum threshold |
| **TendermintCircuit** | Cosmos Tendermint finality | >2/3 voting power accumulation gate |
| **FoldingCircuit** | Recursive proof aggregation | Pairwise folding with Fiat-Shamir challenge `alpha = (C1+C2)^5`, O(log N) depth |
| **BatchedInterlinkCircuit** | Multi-message batching | O(1) on-chain verification for N cross-chain messages in a single SNARK |

---

## Multi-Chain Contracts

### EVM Spoke Gateway (`contracts/evm/`)

- **InterlinkGateway.sol**: deposits (`sendCrossChainMessage`, `initiateSwap`, `lockNFT`),
  verified inbound releases (`releaseTokens`) and generic calls (`executeVerifiedMessage`,
  which can never target the gateway or a custodied asset). Groth16 verification uses the
  BN254 precompiles. VK and remote-gateway changes have a 2-day timelock, the guardian
  transfers in two steps, and emergency withdrawal only works while paused.
- **Scripts**: `Deploy.s.sol`, `DeployAndInit.s.sol` (uploads the VK from `export-vk`),
  `DeployL2.s.sol`.
- **Tests**: Foundry suite including a real arkworks Groth16 proof (`testRealProof_*`).

### Solana Hub (`contracts/solana/interlink-hub/`)

- **Program ID**: `AKzpc9tvxfhLjj5AantKizK2YZgSjoyhLmYqRZy6b8Lz`
- `submit_proof`: a staked relayer attests a fully specified message (the hash is recomputed
  on-chain and a record is kept per payload hash).
- `execute_transfer` / `execute_swap`: permissionless execution bound to the attested hash.
  Creates the recipient's token account if needed.
- `burn_for_withdrawal`: burns wrapped tokens and emits an outbound message for
  `releaseTokens` on the origin chain.
- Wrapped-asset and remote-gateway registries, 7-day unbonding, VK timelock.
- Host tests run a real Groth16 proof through the on-chain verifier.

### Cosmos Gateway (`contracts/cosmos/`)

- Deposits (one denom per message, bound into the hash), two-step guardian.
- **Inbound execution is disabled** (`VerifierUnavailable`): CosmWasm 1.5 has no BN254
  pairing, so there is no sound verifier yet.

---

## Token Economics

### $INTERLINK Token

| Parameter | Value |
|---|---|
| Total Supply | 1,000,000,000 (1B) |
| Community / Mining | 400,000,000 (40%) |
| Team (4yr vest, 1yr cliff) | 300,000,000 (30%) |
| Treasury (DAO-governed) | 300,000,000 (30%) |

### Fee Tiers

| Tier | Transfer Size | Fee | vs Wormhole | vs Across |
|---|---|---|---|---|
| Zero | $0 - $1,000 | **0%** | Wormhole: $1-20 VAA | Across: 0.25-1% |
| Standard | $1k - $100k | **0.05%** | Wormhole: 0.1-0.2% | Across: 0.25-1% |
| Institutional | $100k - $10M | **0.01%** | Wormhole: negotiated | Across: negotiated |
| OTC | $10M+ | **0% (negotiated)** | — | — |

### Staking Tiers

| Tier | Min Stake | Fee Discount | Extra Benefits |
|---|---|---|---|
| Bronze | 1,000 | 10% | Basic participation |
| Silver | 10,000 | 25% | Governance voting |
| Gold | 100,000 | 50% | Validator eligibility |
| Platinum | 1,000,000 | 100% | Enhanced APY + zero fees |

### Liquidity Mining Program

- **Budget**: 10,000,000 $INTERLINK over 26 weekly epochs (6 months)
- **Early-bird boost**: 2x rewards in epochs 1-4 to bootstrap liquidity
- **Loyalty boost**: 1.5x for LPs with ≥4 consecutive epochs
- **Vesting**: 25% immediate release, 75% linear over 90 days
- **Anti-gaming**: 24-hour minimum deposit before earning rewards

---

## Security Model

> Read [docs/SECURITY_REVIEW.md](docs/SECURITY_REVIEW.md) first. The protocol is unaudited.

### What a proof attests today

Each message gets a Groth16 (BN254) proof whose public input is
`(payloadHash mod r + keccak256("interlink_v1_domain"))^5 + nonce`. Every verifier recomputes
`payloadHash` from the message fields, so a proof cannot be reused for different amounts,
recipients, chains or gateways. The circuit does **not** yet verify source-chain consensus or
receipt inclusion. Until it does, whoever holds the proving key can attest any message.

### Defense in depth (implemented)

| Layer | Where |
|---|---|
| Canonical payload hash recomputed on-chain | EVM gateway, Solana hub, Cosmos |
| Replay protection per payload hash | EVM `executedMessages`, hub message records |
| Custody cap: releases ≤ locked amount per asset | EVM `totalLocked` |
| Verified calls cannot touch custody | EVM `executeVerifiedMessage` |
| Timelocked VK / remote-gateway changes, two-step guardian | EVM, hub, Cosmos |
| Finalized-tag wait + receipt re-check (reorg safety) | relayer `finality.rs` |
| Relayer stake in the real ILINK mint, 7-day unbonding | hub |

Not wired in (library code only): the circuit breaker, rate limiter, multisig co-signing and
audit trail modules.

---

## Technical Specifications

| Parameter | Value |
|---|---|
| Proving system | Groth16 over BN254 (arkworks), single public input |
| On-chain verification | EVM precompiles 0x06/0x07/0x08; Solana `alt_bn128` syscalls |
| Message hash | keccak256 of the ABI-encoded canonical message |
| Hub chain ID | `2` (both gateway addresses are also hashed) |
| Source finality | Node `finalized` tag (Ethereum ≈ 13 min; rollups per their L1 finality) |
| Trusted setup | Local single-party (dev). A multi-party ceremony is required for production |

---

## Developer Setup

### Prerequisites
- Rust (stable), Node.js
- Solana CLI and Anchor CLI `0.30.1` (hub program)
- Foundry (`forge`) for EVM contracts

### Building and testing
```bash
cargo test --workspace                       # relayer, circuits, interlink-core
(cd contracts/solana/interlink-hub && cargo test)   # hub host tests
(cd contracts/cosmos && cargo test)
(cd contracts/evm && forge test)
(cd sdk && npm ci && npm test)
cargo run --bin export-vk                    # proving key + VK for on-chain upload
```

### Relayer environment
See [`.env.example`](.env.example). Key variables: `CHAIN_ID` (EIP-155), `EVM_HTTP_RPC_URL`,
`GATEWAY_ADDRESS`, `SOLANA_RPC_URL`, `HUB_PROGRAM_ID`, `KEYPAIR_PATH`, `PROVING_KEY_PATH`,
and optionally `EVM_PRIVATE_KEY_FILE` (enables hub → EVM withdrawals) and `API_TOKEN`
(enables webhook endpoints).

---

## Observability

### Prometheus Metrics

The relayer exposes a `GET /metrics` endpoint compatible with Prometheus/Grafana:

```
# Proof generation
proof_gen_ms_p50, proof_gen_ms_p95, proof_gen_ms_p99
proof_gen_total, proof_gen_alerts

# Settlement
settlement_ms_p50, settlement_ms_p95, settlement_ms_p99
settlement_total, settlement_alerts

# Verification
verify_ms_mean, verify_ms_max, verify_alerts

# Chain health (per-chain)
chain_finality_mean_ms{chain_id="1"}
chain_rpc_latency_ms{chain_id="1"}

# User metrics
daily_transfers, unique_users
corridor_count{corridor="1:900"}

# TVL & Volume
tvl_usd_cents, daily_volume_usd_cents, cumulative_volume_usd_cents

# Validator uptime
validator_heartbeats_total, validator_heartbeats_expected
```

### Alerting Thresholds

| Metric | Alert Threshold | Action |
|---|---|---|
| Proof gen time | >1 second | Investigate prover load |
| Verification time | >500ms | Check verifier efficiency |
| Settlement finality | >60 seconds | Check chain congestion |
| Validator downtime | >15 minutes | Check nonce pool exhaustion |
| Circuit breaker | Any auto-pause | Incident response playbook |

---

## Solana Hub — Devnet

An earlier version of the hub was deployed to devnet at
`AKzpc9tvxfhLjj5AantKizK2YZgSjoyhLmYqRZy6b8Lz`. The program has since changed (account
layouts, instructions, verification), so it must be redeployed before devnet testing. Results
previously listed here named instructions that do not exist in the program and were removed.

---

## Documentation and Resources

- **Solana Operations**: [SOLANA_OPERATIONS.md](./docs/SOLANA_OPERATIONS.md)
- **ZK Circuit Architecture**: [ZK_CIRCUITS_V1.md](./docs/ZK_CIRCUITS_V1.md)
- **Technical Whitepaper**: [InterLink Research (PDF)](./Interlink_Research.pdf)
- **Developer Portal**: [interlink.protocol](https://meridianalgo.github.io/Interlink/)
- **GitHub Repository**: [MeridianAlgo/Interlink](https://github.com/MeridianAlgo/Interlink)
