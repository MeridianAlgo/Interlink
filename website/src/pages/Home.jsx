import React from 'react'
import { motion } from 'framer-motion'
import { Link } from 'react-router-dom'
import { ArrowRight, Shield, Zap, Globe, Lock, GitBranch, Cpu, CheckCircle, AlertTriangle } from 'lucide-react'

const featureIn = { hidden: { opacity: 0, y: 16 }, show: { opacity: 1, y: 0 } }

const FEATURES = [
    {
        icon: Lock,
        title: 'zk-SNARK Verified',
        desc: 'Every cross-chain message is proven using the Halo2 proving system with BN254 curve arithmetic. No multisigs, no trust assumptions—just math.',
    },
    {
        icon: Zap,
        title: 'Solana Execution Hub',
        desc: 'The Hub is an on-chain Anchor program that verifies proofs in O(1) and manages global state across all connected chains.',
    },
    {
        icon: Globe,
        title: 'Hub-and-Spoke Topology',
        desc: 'O(N) connection complexity. Add a new chain by connecting it to the Hub—no pairwise bridge deployments, no coordination complexity.',
    },
    {
        icon: GitBranch,
        title: 'Recursive Proof Aggregation',
        desc: 'Using Pallas/Vesta curve cycling, multiple transaction proofs are folded into one. Amortized on-chain verification cost approaches O(1).',
    },
    {
        icon: Cpu,
        title: 'Concurrent Relayer Network',
        desc: 'Relayers run Tokio-based async engines. A 1024-message channel buffer keeps the proof pipeline saturated with zero blocking.',
    },
    {
        icon: Shield,
        title: 'Replay-Proof by Design',
        desc: 'Every Gateway tracks executed nonces. The CEI (Checks-Effects-Interactions) pattern prevents reentrancy, and sequence numbers prevent double-processing.',
    },
]

const COMPARISON = [
    { name: 'LayerZero', security: 'Multisig + DVN', cost: 'Medium', latency: '5–20 min', finality: 'Probabilistic', hl: false },
    { name: 'Axelar', security: 'Multisig Guards', cost: 'Low', latency: '1–5 min', finality: 'Probabilistic', hl: false },
    { name: 'Wormhole', security: 'Multisig Guards', cost: 'Low', latency: '1–5 min', finality: 'Probabilistic', hl: false },
    { name: 'IBC', security: 'Light Clients', cost: 'High', latency: '10–30 min', finality: 'Deterministic', hl: false },
    { name: 'Chainlink CCIP', security: 'Oracle Networks', cost: 'Medium', latency: '5–15 min', finality: 'Probabilistic', hl: false },
    { name: 'InterLink', security: 'zk-SNARK Proofs', cost: 'Low O(1)', latency: '1–5 min', finality: 'Deterministic', hl: true },
]

const FLOW = [
    { title: 'User calls the Gateway', desc: 'sendCrossChainMessage() locks assets/ETH in the EVM Gateway, increments a nonce, and emits MessagePublished.' },
    { title: 'Relayer picks up the event', desc: 'A WebSocket listener detects the event. After source-chain finality, the Relayer fetches the Merkle proof and block headers.' },
    { title: 'Halo2 proof is generated', desc: 'The InterlinkCircuit synthesizes a ZK-SNARK over BN254. The cubic s-box commitment formula is: C = (payload + 0x1337)³ + seq.' },
    { title: 'Proof submitted to Hub', desc: 'The Relayer serializes the Anchor instruction, signs it with an ed25519 key, and posts it to the Solana Hub via JSON-RPC.' },
    { title: 'Hub verifies and executes', desc: 'The Hub calls the BN254 pairing precompile (0x08). A valid e(a,b)·e(c,−g₂)=1 result triggers message execution on the destination Gateway.' },
]

const Home = () => (
    <div>
        {/* ── Hero ──────────────────────────── */}
        <section className="hero">
            <div className="container">
                <motion.div initial="hidden" animate="show" variants={featureIn}>
                    <div className="hero-eyebrow">
                        <CheckCircle size={11} />
                        v0.6.1 · Audit-Candidate Draft
                    </div>
                    <h1>Trustless cross-chain messaging <span>powered by zero-knowledge proofs.</span></h1>
                    <p>
                        InterLink connects any two blockchains using zk-SNARKs instead of trusted committees. One proof. One hub. No middlemen.
                    </p>
                    <div className="hero-actions">
                        <Link to="/docs" className="btn btn-primary">
                            Read the docs <ArrowRight size={15} />
                        </Link>
                        <a
                            href="https://github.com/MeridianAlgo/Cobalt"
                            target="_blank"
                            rel="noopener noreferrer"
                            className="btn btn-ghost"
                        >
                            View source
                        </a>
                    </div>
                </motion.div>
            </div>
        </section>

        <hr className="divider" />

        {/* ── Stats ─────────────────────────── */}
        <div className="stats-bar">
            {[
                { val: 'O(1)', label: 'On-chain verification' },
                { val: '~100 B', label: 'Aggregated proof size' },
                { val: 'BN254', label: 'Curve + EVM precompile' },
                { val: 'Halo2', label: 'Proving system' },
            ].map(s => (
                <div className="stat" key={s.label}>
                    <span className="stat-val">{s.val}</span>
                    <span className="stat-label">{s.label}</span>
                </div>
            ))}
        </div>

        {/* ── Features ──────────────────────── */}
        <section className="pad">
            <div className="container">
                <div className="section-header">
                    <h4>Protocol Design</h4>
                    <h2>Built without trust assumptions.</h2>
                    <p>Every component of InterLink is designed so that no participant needs to trust another—only the math.</p>
                </div>

                <motion.div
                    className="feature-grid"
                    initial="hidden"
                    whileInView="show"
                    viewport={{ once: true }}
                    variants={{ show: { transition: { staggerChildren: 0.07 } } }}
                >
                    {FEATURES.map(f => (
                        <motion.div key={f.title} className="feature-cell" variants={featureIn}>
                            <div className="feature-icon"><f.icon size={16} /></div>
                            <h3>{f.title}</h3>
                            <p>{f.desc}</p>
                        </motion.div>
                    ))}
                </motion.div>
            </div>
        </section>

        {/* ── Flow ──────────────────────────── */}
        <section className="pad">
            <div className="container">
                <div className="two-col">
                    <div>
                        <div className="section-header" style={{ marginBottom: '1.5rem' }}>
                            <h4>Message lifecycle</h4>
                            <h2>From deposit to execution.</h2>
                            <p>A cross-chain transfer goes through five deterministic steps, each cryptographically guaranteed.</p>
                        </div>
                        <Link to="/docs/lifecycle" className="btn btn-ghost" style={{ marginTop: '0.5rem' }}>
                            Full lifecycle docs <ArrowRight size={14} />
                        </Link>
                    </div>
                    <div className="flow-steps">
                        {FLOW.map((s, i) => (
                            <div key={i} className="flow-step">
                                <div className="step-num">{i + 1}</div>
                                <div>
                                    <h3>{s.title}</h3>
                                    <p>{s.desc}</p>
                                </div>
                            </div>
                        ))}
                    </div>
                </div>
            </div>
        </section>

        {/* ── Comparison ────────────────────── */}
        <section className="pad">
            <div className="container">
                <div className="section-header">
                    <h4>Competitive Analysis</h4>
                    <h2>How InterLink compares.</h2>
                    <p>Most bridges trade security for convenience. InterLink achieves both with deterministic finality.</p>
                </div>

                <div className="table-wrap">
                    <table className="comp-table">
                        <thead>
                            <tr>
                                <th>Protocol</th>
                                <th>Security Model</th>
                                <th>Verification Cost</th>
                                <th>Latency</th>
                                <th>Finality</th>
                            </tr>
                        </thead>
                        <tbody>
                            {COMPARISON.map(r => (
                                <tr key={r.name} className={r.hl ? 'highlight' : ''}>
                                    <td><strong>{r.name}</strong></td>
                                    <td>{r.security}</td>
                                    <td>{r.cost}</td>
                                    <td>{r.latency}</td>
                                    <td>
                                        <span className={`tag ${r.finality === 'Deterministic' ? 'tag-green' : 'tag-orange'}`}>
                                            {r.finality === 'Deterministic' ? <CheckCircle size={10} /> : <AlertTriangle size={10} />}
                                            {r.finality}
                                        </span>
                                    </td>
                                </tr>
                            ))}
                        </tbody>
                    </table>
                </div>
            </div>
        </section>
    </div>
)

export default Home
