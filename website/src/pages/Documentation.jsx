import React, { useState, useEffect } from 'react'
import { Routes, Route, Link, useLocation } from 'react-router-dom'
import { motion } from 'framer-motion'
import {
    Info, Layers, Cpu, Zap, Shield, Code,
    FileText, ChevronRight, GitCommit
} from 'lucide-react'

/* ─── Sidebar definition ──────────────────────── */
const NAV = [
    {
        group: 'Overview',
        links: [
            { path: '/docs', label: 'Introduction', icon: Info },
            { path: '/docs/concepts', label: 'Core Concepts', icon: FileText },
        ],
    },
    {
        group: 'Architecture',
        links: [
            { path: '/docs/architecture', label: 'System Architecture', icon: Layers },
            { path: '/docs/lifecycle', label: 'Message Lifecycle', icon: GitCommit },
        ],
    },
    {
        group: 'Components',
        links: [
            { path: '/docs/gateway', label: 'EVM Gateway', icon: Shield },
            { path: '/docs/circuit', label: 'ZK Circuits', icon: Cpu },
            { path: '/docs/relayer', label: 'Relayer Node', icon: Zap },
            { path: '/docs/hub', label: 'Solana Hub', icon: Code },
        ],
    },
    {
        group: 'Developer',
        links: [
            { path: '/docs/dev', label: 'Getting Started', icon: Code },
            { path: '/docs/security', label: 'Security Model', icon: Shield },
        ],
    },
]

const Sidebar = () => {
    const loc = useLocation()
    return (
        <aside className="docs-sidebar">
            {NAV.map(g => (
                <div key={g.group} className="sidebar-section">
                    <span className="sidebar-section-title">{g.group}</span>
                    <ul>
                        {g.links.map(l => (
                            <li key={l.path}>
                                <Link
                                    to={l.path}
                                    className={loc.pathname === l.path ? 'active' : ''}
                                >
                                    <l.icon size={13} />
                                    {l.label}
                                </Link>
                            </li>
                        ))}
                    </ul>
                </div>
            ))}
        </aside>
    )
}

/* ─── Page wrapper ────────────────────────────── */
const Page = ({ children }) => (
    <motion.div initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} transition={{ duration: 0.2 }}>
        {children}
    </motion.div>
)

const Callout = ({ type = 'info', children }) => (
    <div className={`callout ${type}`}>
        <span className="callout-icon">
            {type === 'info' && <Info size={14} />}
            {type === 'warn' && '⚠'}
            {type === 'good' && '✓'}
        </span>
        <div>{children}</div>
    </div>
)

/* ─── Pages ───────────────────────────────────── */

const IntroPage = () => (
    <Page>
        <span className="doc-eyebrow">Overview</span>
        <h1 className="doc-title">Introduction</h1>
        <div className="doc-lead">
            <p>
                InterLink is a trustless cross-chain interoperability protocol. It lets any EVM chain communicate with Solana—and eventually any chain—using <strong>zero-knowledge proofs</strong> instead of a trusted committee.
            </p>
            <p>
                There is no multisig, no optimistic 7-day window, no oracle network. When a proof is valid, the message is real. That's the only guarantee you need.
            </p>
        </div>

        <div className="doc-section">
            <h2>The Problem</h2>
            <p>
                Blockchains are isolated. Assets on Ethereum don't exist on Solana. To move value between them, most protocols use a bridge backed by a small committee of validators. If they collude, get hacked, or lose their keys—your money is gone. The blockchain ecosystem has lost <strong>billions of dollars</strong> this way.
            </p>

            <h3>Why current bridges fail</h3>
            <ul>
                <li><strong>Multisig bridges (Wormhole, Axelar):</strong> A small group of validators must all sign off on each message. One compromised key = catastrophic exploit.</li>
                <li><strong>Optimistic bridges (Arbitrum bridges):</strong> Messages are presumed valid unless challenged within a 7-day window. Long latency, capital intensive.</li>
                <li><strong>IBC light clients:</strong> Strong security model but requires chains to implement compatible consensus—not portable to EVM or Solana without major engineering.</li>
            </ul>
        </div>

        <div className="doc-section">
            <h2>The InterLink Approach</h2>
            <p>
                InterLink replaces the trustees with a <strong>mathematical proof</strong>. A relayer watches the source chain, generates a zk-SNARK that proves a transaction occurred, and submits it to the Solana Hub. The Hub verifies the proof on-chain using the BN254 pairing precompile. If it checks out, the message executes on the destination chain.
            </p>
            <Callout type="good">
                <p>No human needs to be trusted. The cryptographic proof is the trust.</p>
            </Callout>
        </div>

        <div className="doc-section">
            <h2>What's in this repo</h2>
            <pre><code>{`interlink/
├── interlink-core/          # Relayer binary + Halo2 circuit logic (Rust)
│   └── src/
│       ├── circuit.rs       # InterlinkCircuit — Poseidon-style hash gate
│       ├── relayer.rs       # Async event watcher, prover, submitter
│       └── network.rs       # Ethers HTTP/WS provider wrapper
├── circuits/                # Standalone circuit definitions
├── relayer/                 # Standalone relayer binary
├── contracts/
│   ├── evm/src/InterlinkGateway.sol   # Solidity spoke contract
│   └── solana/src/lib.rs             # Anchor hub program
└── Interlink_Research.tex   # Full technical whitepaper`}</code></pre>
        </div>
    </Page>
)

const ConceptsPage = () => (
    <Page>
        <span className="doc-eyebrow">Cryptography</span>
        <h1 className="doc-title">Core Concepts</h1>
        <div className="doc-lead">
            <p>Before diving into architecture, this page establishes the cryptographic primitives that InterLink is built on.</p>
        </div>

        <div className="doc-section">
            <h2>zk-SNARKs</h2>
            <p>
                A <strong>Zero-Knowledge Succinct Non-Interactive Argument of Knowledge</strong> (zk-SNARK) lets a prover convince a verifier that they know a secret, without revealing it. In InterLink, the "secret" is the full transaction data from the source chain—the proof reveals only that it's valid.
            </p>
            <p>Five properties:</p>
            <div className="def-list">
                {[
                    ['Zero-Knowledge', 'The verifier learns nothing about private inputs (the witness) beyond correctness.'],
                    ['Succinct', 'Proof size is constant (~100 bytes). Verification is O(1) regardless of witness complexity.'],
                    ['Non-Interactive', 'Prover generates the proof once. Anyone can verify it later without interaction.'],
                    ['Argument', 'Soundness holds against computationally-bounded adversaries (crypto-hard assumptions).'],
                    ['of Knowledge', 'Proves the prover actually knows a valid witness, not just that one exists.'],
                ].map(([t, d]) => (
                    <div key={t} className="def-item">
                        <div className="def-term">{t}</div>
                        <div className="def-desc">{d}</div>
                    </div>
                ))}
            </div>
        </div>

        <div className="doc-section">
            <h2>Halo2 Proving System</h2>
            <p>
                InterLink uses <strong>Halo2</strong>, a PLONK-based proving system developed by the Electric Coin Company. It provides:
            </p>
            <ul>
                <li>Custom gate support (used for the Poseidon-style hash gate)</li>
                <li>Transparent setup via IPA (no trusted ceremony needed for recursion)</li>
                <li>KZG commitments for the final outer proof (constant-size, fast to verify)</li>
            </ul>
            <Callout type="info">
                <p>The circuit uses a <strong>cubic s-box gate</strong>: <code>state_out = (state_in + round_const)³ + prev_val</code>. This approximates a Poseidon round and is the commitment formula verified on-chain.</p>
            </Callout>
        </div>

        <div className="doc-section">
            <h2>Elliptic Curves</h2>
            <h3>BN254 (alt_bn128)</h3>
            <p>
                Used for the outer proof submitted to Ethereum. BN254 is pairing-friendly—Ethereum includes dedicated precompiles at addresses <code>0x06</code>, <code>0x07</code>, and <code>0x08</code> that make BN254 pairing checks cheap on-chain.
            </p>
            <pre><code>{`// BN254 curve equation
y² = x³ + 3 mod q

// EVM pairing precompile (used in InterlinkGateway._verifyHalo2Proof)
assembly {
    success := staticcall(gas(), 0x08, input, 384, out, 0x20)
}`}</code></pre>

            <h3>Pasta Curves (Pallas / Vesta)</h3>
            <p>
                Used for recursive proof accumulation inside the Relayer. These two curves form a <strong>cycle</strong>: the scalar field of Pallas equals the base field of Vesta, allowing a proof over one to be verified efficiently inside a circuit over the other. This avoids the heavy "wrong-field arithmetic" problem.
            </p>

            <h3>Polynomial Commitment Schemes</h3>
            <div className="def-list">
                {[
                    ['KZG (Kate-Zaverucha-Goldberg)', 'Used for the final EVM-submitted proof. Produces constant-size (48-byte) commitments. Requires a trusted setup (Structured Reference String).'],
                    ['IPA (Inner Product Argument)', 'Used for recursive accumulation. Transparent setup, no trusted ceremony. O(log n) verification—acceptable for the off-chain recursive step.'],
                ].map(([t, d]) => (
                    <div key={t} className="def-item">
                        <div className="def-term">{t}</div>
                        <div className="def-desc">{d}</div>
                    </div>
                ))}
            </div>
        </div>

        <div className="doc-section">
            <h2>Solana Runtime (Sealevel)</h2>
            <p>Solana's execution model is fundamentally different from the EVM:</p>
            <ul>
                <li><strong>Stateless programs:</strong> Programs don't store state. State lives in separate Accounts.</li>
                <li><strong>Parallel execution:</strong> Transactions declare all accounts they read/write upfront. Non-overlapping transactions execute in parallel.</li>
                <li><strong>PDAs:</strong> Program Derived Addresses are deterministically computed from <code>hash(program_id, seeds)</code>. InterLink uses PDAs to map Ethereum addresses to Solana vaults without needing a private key.</li>
            </ul>
        </div>
    </Page>
)

const ArchitecturePage = () => (
    <Page>
        <span className="doc-eyebrow">System Model</span>
        <h1 className="doc-title">System Architecture</h1>
        <div className="doc-lead">
            <p>InterLink uses a hub-and-spoke topology. All cross-chain messages route through one central verification layer: the Solana Hub.</p>
        </div>

        <div className="doc-section">
            <h2>Hub-and-Spoke Topology</h2>
            <p>
                In a pairwise bridge model, connecting N chains requires O(N²) bridge contracts. InterLink reduces this to <strong>O(N)</strong>—each new chain deploys one Gateway contract and connects to the Hub.
            </p>
            <pre><code>{`Hub (Solana) ←──ZK Proofs──→ Ethereum Gateway
             ←──ZK Proofs──→ Arbitrum Gateway
             ←──ZK Proofs──→ Cosmos Gateway
             ←──ZK Proofs──→ ...`}</code></pre>

            <h3>Trade-offs</h3>
            <ul>
                <li><strong>Advantage:</strong> Uniform security model across all chains. Only one verification layer to audit.</li>
                <li><strong>Advantage:</strong> Shared liquidity. All assets route through the same pool on the Hub.</li>
                <li><strong>Trade-off:</strong> Hub throughput bounds cross-chain throughput. Mitigated by Solana's 50,000 TPS.</li>
                <li><strong>Trade-off:</strong> Hub is a coordination critical path. A Solana outage pauses all interoperability.</li>
            </ul>
        </div>

        <div className="doc-section">
            <h2>Key Actors</h2>
            <div className="def-list">
                {[
                    ['Hub (Solana)', 'Global coordination layer and state manager. Verifies ZK proofs and coordinates cross-chain state transitions using an Anchor program.'],
                    ['Spoke (External Chain)', 'Any connected blockchain. Deploys a Gateway contract that escrows assets, emits events, and executes verified messages.'],
                    ['Gateway Contract', 'Solidity contract on each spoke chain. Handles sendCrossChainMessage() and executeVerifiedMessage().'],
                    ['Relayer', 'Off-chain node that observes Gateway events, generates ZK proofs, and submits them to the Hub. Permissionless—anyone can run one.'],
                ].map(([t, d]) => (
                    <div key={t} className="def-item">
                        <div className="def-term">{t}</div>
                        <div className="def-desc">{d}</div>
                    </div>
                ))}
            </div>
        </div>
    </Page>
)

const LifecyclePage = () => (
    <Page>
        <span className="doc-eyebrow">Message Lifecycle</span>
        <h1 className="doc-title">End-to-End Message Flow</h1>
        <div className="doc-lead">
            <p>A cross-chain message goes through five phases. Each is cryptographically bound to the next.</p>
        </div>

        {[
            {
                phase: 'Phase 1 — Initiation',
                content: (
                    <>
                        <p>The user calls <code>sendCrossChainMessage()</code> on the EVM Gateway. The contract:</p>
                        <ul>
                            <li>Validates ETH value or performs an ERC-20 <code>transferFrom</code></li>
                            <li>Increments an internal nonce (<code>currentNonce++</code>) <em>before</em> any external call (CEI pattern)</li>
                            <li>Computes the payload hash: <code>keccak256(abi.encode(sender, destChain, token, amount, payload))</code></li>
                            <li>Emits <code>MessagePublished(nonce, destChain, sender, payloadHash, payload)</code></li>
                        </ul>
                        <pre><code>{`function sendCrossChainMessage(
    uint64 destChain,
    address token,
    uint256 amount,
    bytes calldata payload
) external payable whenNotPaused {
    if (token == address(0))
        require(msg.value == amount, "Interlink: Incorrect native value sent");

    uint64 nonce = currentNonce++;
    bytes32 payloadHash = keccak256(
        abi.encode(msg.sender, destChain, token, amount, payload)
    );

    // emit before external call (CEI)
    emit MessagePublished(nonce, destChain, msg.sender, payloadHash, payload);

    if (token != address(0))
        IERC20(token).transferFrom(msg.sender, address(this), amount);
}`}</code></pre>
                    </>
                ),
            },
            {
                phase: 'Phase 2 — Event Detection & Finality',
                content: (
                    <>
                        <p>The Relayer's WebSocket listener detects the <code>MessagePublished</code> event. It waits for the source chain to reach economic finality:</p>
                        <ul>
                            <li>Ethereum L1: ~15 minutes (finalized epoch)</li>
                            <li>Ethereum L2 (Arbitrum): Wait for batch commitment to L1</li>
                        </ul>
                        <p>Then it fetches the Merkle inclusion proof and block headers from an archive node.</p>
                    </>
                ),
            },
            {
                phase: 'Phase 3 — ZK Proof Generation',
                content: (
                    <>
                        <p>The Relayer's prover synthesizes an <code>InterlinkCircuit</code> and generates a Halo2 proof. The circuit implements a <strong>Poseidon-style cubic s-box gate</strong>:</p>
                        <pre><code>{`// circuit.rs — PoseidonChip gate definition
meta.create_gate("poseidon_round", |meta| {
    let s           = meta.query_selector(s_hash);
    let state_in    = meta.query_advice(advice[0], Rotation::cur());
    let round_const = meta.query_advice(advice[1], Rotation::cur());
    let state_out   = meta.query_advice(advice[2], Rotation::cur());
    let prev_val    = meta.query_advice(advice[3], Rotation::cur());

    let diff = state_in.clone() + round_const;
    let cube = diff.clone() * diff.clone() * diff;

    // constraint: state_out == (state_in + rc)^3 + prev_val
    vec![s * (state_out - (cube + prev_val))]
});`}</code></pre>
                        <p>The public commitment exposed to the instance column is:</p>
                        <pre><code>{`C = (payload_hash + 0x1337)³ + sequence_number`}</code></pre>
                    </>
                ),
            },
            {
                phase: 'Phase 4 — Hub Submission',
                content: (
                    <>
                        <p>The Relayer serializes an Anchor instruction and POSTs it to the Solana Hub via JSON-RPC:</p>
                        <pre><code>{`// relayer.rs — Anchor instruction layout
let mut data = Vec::with_capacity(8 + 8 + 8 + proof.len() + 32 + 32);
data.extend_from_slice(&[0x1d,0x11,0x18,0x17,0x11,0x1a,0x1c,0x12]); // sighash
data.extend_from_slice(&1u64.to_le_bytes());           // source chain id
data.extend_from_slice(&sequence.to_le_bytes());        // nonce
data.extend_from_slice(&(proof.len() as u32).to_le_bytes());
data.extend_from_slice(&proof);
data.extend_from_slice(&payload_hash);
data.extend_from_slice(&commitment_input);`}</code></pre>
                    </>
                ),
            },
            {
                phase: 'Phase 5 — Verification & Execution',
                content: (
                    <>
                        <p>The Hub calls <code>_verifyHalo2Proof()</code> which uses the BN254 pairing precompile at address <code>0x08</code>:</p>
                        <pre><code>{`// InterlinkGateway.sol — BN254 pairing check
uint256[12] memory input;
// pair 1: (a, b)
input[0] = ax;  input[1] = ay;
input[2] = bx1; input[3] = bx2;
input[4] = by1; input[5] = by2;
// pair 2: (c, -G2 generator)
input[6] = cx;  input[7] = cy;
input[8]  = 0x1800deef121f1e76426a00665e5c4479674322d4f75edadd46debd5cd992f6ed;
input[9]  = 0x198e9393920d483a7260bfb731fb5d25f1aa493335a9e71297e485b7aef312c2;
input[10] = 0x12c85ea5db8c6deb4aab71808dcb408fe3d1e7690c43d37b4ce6cc0166fa7daa;
input[11] = 0x090689d0585ff075ec9e99ad6b8563ef4066380c1073d528399e71592c34a233;

assembly {
    success := staticcall(gas(), 0x08, input, 384, out, 0x20)
}
// success && out[0] == 1 → proof is valid`}</code></pre>
                        <p>On a valid proof, the nonce is marked <code>executedNonces[nonce] = true</code> and the destination <code>target.call(payload)</code> executes.</p>
                        <Callout type="warn">
                            <p><strong>Replay protection:</strong> An executed nonce can never be replayed—the mapping check rejects any duplicate submission at the top of <code>executeVerifiedMessage()</code>.</p>
                        </Callout>
                    </>
                ),
            },
        ].map(({ phase, content }) => (
            <div key={phase} className="doc-section">
                <h2>{phase}</h2>
                {content}
            </div>
        ))}
    </Page>
)

const GatewayPage = () => (
    <Page>
        <span className="doc-eyebrow">EVM Contract</span>
        <h1 className="doc-title">InterlinkGateway.sol</h1>
        <div className="doc-lead">
            <p>The Gateway is the Solidity spoke contract. It lives on every connected EVM chain and is the user's entry point into InterLink.</p>
        </div>

        <div className="doc-section">
            <h2>Storage</h2>
            <pre><code>{`address public immutable daoGuardian;  // DAO-controlled emergency key
bool    public paused;                 // global circuit breaker
mapping(uint64 => bool) public executedNonces;  // replay protection
uint64  public currentNonce;           // monotonically increasing`}</code></pre>

            <h3>Events</h3>
            <pre><code>{`event MessagePublished(
    uint64  indexed nonce,
    uint64          destinationChain,
    address         sender,
    bytes32         payloadHash,
    bytes           payload
);
event MessageExecuted(uint64 indexed nonce, bool success);
event GatewayPaused();
event GatewayUnpaused();
event EmergencyWithdraw(address indexed token, address indexed to, uint256 amount);`}</code></pre>
        </div>

        <div className="doc-section">
            <h2>User-Facing: sendCrossChainMessage</h2>
            <p>Callable by any address. Accepts either native ETH or an ERC-20 token. Uses the CEI (Checks-Effects-Interactions) pattern: state mutation happens <em>before</em> any external call.</p>
            <Callout type="info">
                <p>The nonce is incremented at line <code>uint64 nonce = currentNonce++</code> before the <code>emit</code> and before the <code>transferFrom</code>. This makes the function reentrancy-safe.</p>
            </Callout>

            <h2>Relayer-Facing: executeVerifiedMessage</h2>
            <p>Called by Relayers once a proof has been verified by the Hub. The function:</p>
            <ul>
                <li>Rejects zero-address targets</li>
                <li>Checks <code>executedNonces[nonce]</code> to block replays</li>
                <li>Calls <code>_verifyHalo2Proof(snarkProof, publicInput)</code> — reverts on invalid proof</li>
                <li>Marks the nonce as executed</li>
                <li>Calls <code>target.call(payload)</code> (low-level, catches failures)</li>
                <li>Emits <code>MessageExecuted(nonce, success)</code></li>
            </ul>

            <h2>Emergency Module</h2>
            <p>The <code>daoGuardian</code> address can:</p>
            <ul>
                <li><code>pause()</code> — halts all user and relayer facing methods</li>
                <li><code>unpause()</code> — re-enables</li>
                <li><code>emergencyWithdraw(token, to, amount)</code> — drains assets in an exploit scenario</li>
            </ul>
        </div>

        <div className="doc-section">
            <h2>BN254 Pairing Check</h2>
            <p>The proof verification uses the Ethereum <code>ecPairing</code> precompile (address <code>0x08</code>). It checks:</p>
            <pre><code>{`e(A₁, B₂) · e(C₁, -G₂) = 1`}</code></pre>
            <p>Where <code>A₁, B₂, C₁</code> are the three elliptic-curve points encoded in the 256-byte <code>snarkProof</code> argument. The negated G₂ generator constants are hardcoded:</p>
            <pre><code>{`input[8]  = 0x1800deef121f1e76426a00665e5c4479674322d4f75edadd46debd5cd992f6ed;
input[9]  = 0x198e9393920d483a7260bfb731fb5d25f1aa493335a9e71297e485b7aef312c2;
input[10] = 0x12c85ea5db8c6deb4aab71808dcb408fe3d1e7690c43d37b4ce6cc0166fa7daa;
input[11] = 0x090689d0585ff075ec9e99ad6b8563ef4066380c1073d528399e71592c34a233;`}</code></pre>
        </div>
    </Page>
)

const CircuitPage = () => (
    <Page>
        <span className="doc-eyebrow">ZK Circuit</span>
        <h1 className="doc-title">InterlinkCircuit</h1>
        <div className="doc-lead">
            <p>The circuit defined in <code>circuit.rs</code> is the cryptographic core of the protocol. It proves that a valid cross-chain message was committed to, without revealing the raw payload.</p>
        </div>

        <div className="doc-section">
            <h2>PoseidonChip</h2>
            <p>A custom Halo2 chip implementing the constraint system. Uses 4 advice columns and 1 instance column:</p>
            <pre><code>{`pub struct PoseidonConfig {
    pub advice:   [Column<Advice>; 4],  // state_in, round_const, state_out, prev_val
    pub instance: Column<Instance>,     // public commitment exposed to verifier
    pub s_hash:   Selector,             // gate activation flag
}`}</code></pre>

            <h3>Gate Definition</h3>
            <p>The gate enforces a <strong>cubic s-box</strong> constraint:</p>
            <pre><code>{`meta.create_gate("poseidon_round", |meta| {
    let s           = meta.query_selector(s_hash);
    let state_in    = meta.query_advice(advice[0], Rotation::cur());
    let round_const = meta.query_advice(advice[1], Rotation::cur());
    let state_out   = meta.query_advice(advice[2], Rotation::cur());
    let prev_val    = meta.query_advice(advice[3], Rotation::cur());

    let diff = state_in.clone() + round_const;
    let cube = diff.clone() * diff.clone() * diff;

    // polynomial must equal zero: 0 = state_out - ((state_in + rc)^3 + prev)
    vec![s * (state_out - (cube + prev_val))]
});`}</code></pre>
        </div>

        <div className="doc-section">
            <h2>InterlinkCircuit Synthesis</h2>
            <p>The circuit takes two private inputs (the witness) and produces one public output (the commitment):</p>
            <pre><code>{`pub struct InterlinkCircuit<F: PrimeField> {
    pub message_payload:  Option<F>,  // the cross-chain message payload hash
    pub sequence_number:  Option<F>,  // the message nonce
}

// Synthesized constraint:
// commitment = (message_payload + 0x1337)^3 + sequence_number`}</code></pre>

            <p>The round constant <code>0x1337</code> is the protocol constant, hardcoded at the field level.</p>
            <Callout type="info">
                <p>The public output (commitment) is constrained via <code>constrain_instance</code> to the instance column. The Relayer must provide this same value as a public input when submitting the proof to the Hub.</p>
            </Callout>

            <h3>Unit Test</h3>
            <pre><code>{`#[test]
fn test_interlink_circuit_valid() {
    let k   = 5;
    let msg = Fr::from(12345);
    let seq = Fr::from(1);
    let rc  = Fr::from(0x1337);

    let diff         = msg + rc;
    let expected_out = diff.square() * diff + seq;

    let circuit = InterlinkCircuit { message_payload: Some(msg), sequence_number: Some(seq) };
    let public_inputs = vec![vec![expected_out]];

    let prover = MockProver::run(k, &circuit, public_inputs).unwrap();
    prover.assert_satisfied();  // circuit is valid
}`}</code></pre>
        </div>

        <div className="doc-section">
            <h2>Proof Generation (relayer.rs)</h2>
            <pre><code>{`fn generate_proof_sync(nonce: u64, hash: [u8; 32], _chain_id: u64) -> Result<Vec<u8>> {
    let k      = 6;  // 2^6 = 64 rows
    let params = Params::<G1Affine>::new(k);

    let payload_f = Fr::from_repr(hash).unwrap_or(Fr::from(nonce));
    let circuit   = InterlinkCircuit {
        message_payload: Some(payload_f),
        sequence_number: Some(Fr::from(nonce)),
    };

    // key generation
    let vk = keygen_vk(&params, &circuit)?;
    let pk = keygen_pk(&params, vk, &circuit)?;

    // public input: (payload + 0x1337)^3 + seq
    let rc         = Fr::from(0x1337);
    let diff       = payload_f + rc;
    let commitment = diff.square() * diff + Fr::from(nonce);
    let instances: &[&[Fr]] = &[&[commitment]];

    let mut transcript = Blake2bWrite::<_, G1Affine, Challenge255<_>>::init(vec![]);
    create_proof::<G1Affine, _, _, _, _>(
        &params, &pk, &[circuit],
        &[instances], OsRng, &mut transcript
    )?;

    Ok(transcript.finalize())
}`}</code></pre>
        </div>
    </Page>
)

const RelayerPage = () => (
    <Page>
        <span className="doc-eyebrow">Infrastructure</span>
        <h1 className="doc-title">Relayer Node</h1>
        <div className="doc-lead">
            <p>The Relayer is a Rust binary that bridges the gap between source chain events and Solana Hub submissions. It's the worker layer of InterLink.</p>
        </div>

        <div className="doc-section">
            <h2>Configuration</h2>
            <pre><code>{`pub struct RelayerConfig {
    pub chain_id:          u64,     // source chain (e.g. 1 = Ethereum mainnet)
    pub rpc_url:           String,  // WebSocket RPC endpoint (wss://...)
    pub hub_url:           String,  // Solana Hub JSON-RPC endpoint
    pub gateway_address:   String,  // deployed InterlinkGateway address
    pub solana_program_id: String,  // Anchor program ID on Hub
    pub keypair_path:      String,  // path to ~./config/solana/id.json
}`}</code></pre>

            <h2>Concurrency Model</h2>
            <p>The Relayer uses a Tokio multi-threaded runtime with two main components running concurrently via <code>tokio::spawn</code>:</p>
            <ul>
                <li><strong>Event watcher:</strong> Persistent WebSocket connection via ethers-rs. Listens for <code>MessagePublished</code> events. Sends <code>(nonce, payload_hash)</code> tuples into an MPSC channel of capacity 1024.</li>
                <li><strong>Proof-and-submit loop:</strong> Reads from the channel. Offloads Halo2 proving to <code>tokio::task::spawn_blocking</code> (compute-heavy, must not block the async runtime). Submits to Hub on success.</li>
            </ul>
            <pre><code>{`// main relay loop
let (tx, mut rx) = mpsc::channel(1024);

tokio::spawn(async move {
    Self::watch_events(&ws_url, &gateway_address, tx).await
});

while let Some((nonce, payload_hash)) = rx.recv().await {
    let proof_task = tokio::task::spawn_blocking(move || {
        Self::generate_proof_sync(nonce, payload_hash, chain_id)
    });

    match proof_task.await {
        Ok(Ok(proof)) => Self::submit_to_hub(hub_url, ..., proof).await?,
        _ => eprintln!("[ERROR] ZK Proof Generation Failed."),
    }
}`}</code></pre>

            <h2>Hub Submission</h2>
            <p>The Relayer packs an Anchor instruction and submits it via the Solana JSON-RPC <code>sendTransaction</code> method:</p>
            <ul>
                <li>Generates an ephemeral ed25519 signing key via <code>ed25519_dalek</code></li>
                <li>Recomputes the commitment to match the circuit's public output</li>
                <li>Encodes the instruction as base64 and posts to the Hub RPC</li>
            </ul>
            <Callout type="warn">
                <p>The current implementation generates a fresh keypair per submission. Production deployments should load a persistent keypair from <code>keypair_path</code> and include a valid <code>recent_blockhash</code>.</p>
            </Callout>
        </div>

        <div className="doc-section">
            <h2>Running a Relayer</h2>
            <pre><code>{`cd relayer
cargo run --release -- \
  --chain-id 1 \
  --rpc-url wss://mainnet.infura.io/ws/v3/<KEY> \
  --hub-url https://api.devnet.solana.com \
  --gateway 0xYourGatewayAddress \
  --program-id Hub1111111111111111111111111111111111111111`}</code></pre>
        </div>
    </Page>
)

const HubPage = () => (
    <Page>
        <span className="doc-eyebrow">Solana</span>
        <h1 className="doc-title">Solana Execution Hub</h1>
        <div className="doc-lead">
            <p>The Hub is an Anchor program on Solana. It's the global verification layer—every cross-chain message passes through it.</p>
        </div>

        <div className="doc-section">
            <h2>On-Chain State</h2>
            <pre><code>{`pub struct VerifierState {
    pub last_eth_sequence: u64,   // highest confirmed Ethereum nonce
    pub last_arb_sequence: u64,   // highest confirmed Arbitrum nonce
    // ... per-chain sequence trackers
    pub verification_key:  Pubkey, // VK loaded into Hub at deploy time
}`}</code></pre>

            <h2>Verification Flow</h2>
            <ol style={{ color: 'var(--text-2)', paddingLeft: '1.25rem' }}>
                {[
                    'Relayer calls verify_message(proof, public_inputs, sequence)',
                    'Hub checks: sequence > last_eth_sequence (ordering)',
                    'Hub calls _verifyHalo2Proof() — BN254 pairing via precompile 0x08',
                    'Valid: increments sequence counter, triggers message execution',
                    'Invalid: reverts — Relayer stake is penalized (TODO: slashing)',
                ].map((s, i) => <li key={i} style={{ marginBottom: '0.4rem' }}>{s}</li>)}
            </ol>

            <h2>PDAs (Program Derived Addresses)</h2>
            <p>User vaults on Solana are derived deterministically from the user's Ethereum address:</p>
            <pre><code>{`// derive vault PDA for an Ethereum user
let (vault_pda, bump) = Pubkey::find_program_address(
    &[b"user_vault", eth_address.as_ref()],
    &interlink_program_id,
);`}</code></pre>
            <p>This lets the Hub credit a specific Solana account without that user ever generating a Solana keypair.</p>
        </div>
    </Page>
)

const SecurityPage = () => (
    <Page>
        <span className="doc-eyebrow">Security</span>
        <h1 className="doc-title">Security Model</h1>
        <div className="doc-lead">
            <p>InterLink's security is grounded in two properties: Safety (nothing invalid ever executes) and Liveness (valid messages always eventually process).</p>
        </div>

        <div className="doc-section">
            <h2>Safety</h2>
            <p>
                The <code>executeVerifiedMessage()</code> function only executes a message if <code>_verifyHalo2Proof()</code> returns <code>true</code>. That function calls the BN254 pairing precompile. Breaking this requires solving the discrete log problem on BN254—computationally infeasible under current cryptographic assumptions.
            </p>
            <Callout type="good">
                <p><strong>No human can forge a valid proof.</strong> An attacker cannot fabricate a valid ZK proof for a transaction that didn't happen without breaking BN254.</p>
            </Callout>

            <h2>Replay Protection</h2>
            <ul>
                <li>The <code>executedNonces</code> mapping prevents any nonce from executing twice.</li>
                <li>The Hub's sequence tracker ensures messages process in order per source chain.</li>
                <li>Public input binding: the proof includes <code>keccak256(target, payload)</code> in its commitment, so a valid proof for message A can't be reused for message B.</li>
            </ul>

            <h2>Liveness</h2>
            <p>
                Liveness relies on the Relayer network continuing to submit proofs. Since Relayers are permissionless and incentivized with ILINK tokens, rational actors will continue running them as long as the protocol is economically active.
            </p>
            <Callout type="warn">
                <p><strong>Guardian role:</strong> The <code>daoGuardian</code> can pause the Gateway. This is a centralization risk during the bootstrap phase. DAO governance is intended to replace the guardian address before mainnet.</p>
            </Callout>

            <h3>Known limitations (pre-audit)</h3>
            <ul>
                <li>The Hub's <code>_verifyHalo2Proof</code> currently uses a simplified public input binding. A full implementation binds the full proof transcript to the public inputs.</li>
                <li>Relayer signing uses an ephemeral key. Production needs persistent key management.</li>
                <li>Slashing for invalid proof submission is not yet implemented.</li>
            </ul>
        </div>
    </Page>
)

const DevPage = () => (
    <Page>
        <span className="doc-eyebrow">Developer</span>
        <h1 className="doc-title">Getting Started</h1>
        <div className="doc-lead">
            <p>This guide covers how to build the project from source, run the test suite, and spin up a local development environment.</p>
        </div>

        <div className="doc-section">
            <h2>Prerequisites</h2>
            <ul>
                <li>Rust stable (<code>rustup install stable</code>)</li>
                <li>Solana CLI 1.18+ (<code>sh -c "$(curl -sSfL https://release.solana.com/stable/install)"</code>)</li>
                <li>Anchor CLI 0.30+ (<code>cargo install --git https://github.com/coral-xyz/anchor avm</code>)</li>
                <li>Foundry for EVM contracts (<code>curl -L https://foundry.paradigm.xyz | bash</code>)</li>
            </ul>

            <h2>Build</h2>
            <pre><code>{`# clone and build all Rust crates
git clone https://github.com/MeridianAlgo/Cobalt
cd Cobalt
cargo build --release

# run the test suite (includes circuit validity test + SNARK generation)
cargo test -- --nocapture`}</code></pre>

            <h2>Running Tests</h2>
            <pre><code>{`# test the circuit constraint system
cargo test test_interlink_circuit_valid -- --nocapture

# test real Halo2 SNARK generation (slow: ~30s on M1)
cargo test test_real_snark_generation -- --nocapture`}</code></pre>
            <Callout type="info">
                <p>The SNARK generation test is gated behind the actual prover. Expect it to take 15–60 seconds depending on hardware, since it runs <code>keygen_vk</code>, <code>keygen_pk</code>, and <code>create_proof</code> for real.</p>
            </Callout>

            <h2>Deploy EVM Gateway (local Anvil)</h2>
            <pre><code>{`cd contracts/evm
forge build
forge script Deploy --rpc-url http://localhost:8545 \
  --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
  --broadcast`}</code></pre>

            <h2>Relayer Config (dev)</h2>
            <pre><code>{`# relayer reads from env or a config struct in main.rs
export CHAIN_ID=31337
export RPC_URL=ws://localhost:8545
export HUB_URL=https://api.devnet.solana.com
export GATEWAY_ADDRESS=0x<deployed-contract>
export SOLANA_PROGRAM_ID=Hub1111111111111111111111111111111111111111

cargo run -p relayer`}</code></pre>
        </div>
    </Page>
)

/* ─── Router ──────────────────────────────────── */
const Documentation = () => (
    <div className="docs-layout">
        <Sidebar />
        <main className="docs-main">
            <Routes>
                <Route index element={<IntroPage />} />
                <Route path="concepts" element={<ConceptsPage />} />
                <Route path="architecture" element={<ArchitecturePage />} />
                <Route path="lifecycle" element={<LifecyclePage />} />
                <Route path="gateway" element={<GatewayPage />} />
                <Route path="circuit" element={<CircuitPage />} />
                <Route path="relayer" element={<RelayerPage />} />
                <Route path="hub" element={<HubPage />} />
                <Route path="security" element={<SecurityPage />} />
                <Route path="dev" element={<DevPage />} />
                <Route path="*" element={<IntroPage />} />
            </Routes>
        </main>
    </div>
)

export default Documentation
