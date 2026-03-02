import React from 'react'
import { motion } from 'framer-motion'
import { Link } from 'react-router-dom'
import { ArrowRight, Shield, Zap, Globe, Lock, GitBranch, Cpu, CheckCircle, AlertTriangle, Activity, Server, Layout } from 'lucide-react'

/*Standerd AI website no im not tryna make a website rn i have better things to do*/

const LiveMetrics = () => {
    const [block, setBlock] = React.useState(1948271)
    const [tps, setTps] = React.useState(42.5)

    React.useEffect(() => {
        const interval = setInterval(() => {
            setBlock(b => b + 1)
            setTps(t => (40 + Math.random() * 5).toFixed(1))
        }, 3000)
        return () => clearInterval(interval)
    }, [])

    return (
        <div className="glass-panel" style={{ padding: '1rem 1.5rem', display: 'flex', gap: '3rem', marginTop: '2rem', justifyContent: 'center', background: 'rgba(0,0,0,0.2)' }}>
            <div style={{ textAlign: 'center' }}>
                <div style={{ fontSize: '0.65rem', color: 'var(--text-3)', fontWeight: 600, letterSpacing: '0.05em' }}>CURRENT BLOCK</div>
                <div style={{ fontSize: '1.2rem', fontWeight: 800, color: 'var(--blue)' }}>#{block.toLocaleString()}</div>
            </div>
            <div style={{ width: 1, height: 40, background: 'var(--border)' }} />
            <div style={{ textAlign: 'center' }}>
                <div style={{ fontSize: '0.65rem', color: 'var(--text-3)', fontWeight: 600, letterSpacing: '0.05em' }}>PROOF THROUGHPUT</div>
                <div style={{ fontSize: '1.2rem', fontWeight: 800, color: '#fff' }}>{tps} SNARK/s</div>
            </div>
            <div style={{ width: 1, height: 40, background: 'var(--border)' }} />
            <div style={{ textAlign: 'center' }}>
                <div style={{ fontSize: '0.65rem', color: 'var(--text-3)', fontWeight: 600, letterSpacing: '0.05em' }}>VERIFICATION COST</div>
                <div style={{ fontSize: '1.2rem', fontWeight: 800, color: 'var(--green)' }}>O(1) CONSTANT</div>
            </div>
        </div>
    )
}

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
        <section className="hero" style={{ paddingBottom: '3rem', paddingTop: '6rem', position: 'relative', overflow: 'hidden' }}>
            <div className="container" style={{ position: 'relative', zIndex: 10 }}>
                <motion.div initial="hidden" animate="show" variants={featureIn} style={{ textAlign: 'center' }}>
                    <div className="hero-eyebrow" style={{ margin: '0 auto 1.5rem', background: 'rgba(59, 130, 246, 0.1)', border: '1px solid rgba(59, 130, 246, 0.3)', color: 'var(--blue)' }}>

                        v0.7.0
                    </div>
                    <h1 className="text-gradient" style={{ fontSize: 'clamp(3rem, 7vw, 5.5rem)', lineHeight: 1.05, fontWeight: 900, letterSpacing: '-0.02em' }}>
                        The O(1) Bridge.
                    </h1>
                    <p style={{ maxWidth: '650px', margin: '1.5rem auto 2.5rem', color: 'var(--text-1)', fontSize: '1.25rem', lineHeight: 1.7, fontWeight: 500 }}>
                        Unifying fragmented liquidity securely. No committees. No multisigs. Just pure zero-knowledge cryptography and math.
                    </p>
                    <div className="hero-actions" style={{ justifyContent: 'center', gap: '1.2rem', display: 'flex', flexWrap: 'wrap' }}>
                        <Link to="/bridge" className="btn btn-primary" style={{ padding: '0.9rem 2.2rem', fontSize: '1.05rem', borderRadius: '2rem', fontWeight: 600, boxShadow: '0 0 20px rgba(59, 130, 246, 0.4)' }}>
                            Launch Bridge
                        </Link>
                        <Link to="/docs" className="btn btn-ghost" style={{ padding: '0.9rem 2.2rem', fontSize: '1.05rem', borderRadius: '2rem', border: '1px solid var(--border)' }}>
                            Explore Docs
                        </Link>
                    </div>

                    <LiveMetrics />
                </motion.div>
            </div>
        </section>

        <hr className="divider" />

        {/* ── Stats ─────────────────────────── */}
        <div className="stats-bar">
            {[
                { val: 'O(1)', label: 'Verification' },
                { val: '~100 B', label: 'Proof size' },
                { val: 'BN254', label: 'EVM precompile' },
                { val: 'Halo2', label: 'Prover' },
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
                    <h2>Built without trust.</h2>
                    <p>Every component is designed so that no participant needs to trust another—only the cryptographic proof.</p>
                </div>

                <motion.div
                    className="feature-grid glass-panel"
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
                            <h4>Lifecycle</h4>
                            <h2>Deterministic paths.</h2>
                            <p>Cross-chain messages move through five verified steps from source to destination.</p>
                        </div>
                        <Link to="/docs/lifecycle" className="btn btn-ghost" style={{ marginTop: '0.5rem' }}>
                            View Lifecycle <ArrowRight size={14} />
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
        <section className="pad" style={{ paddingBottom: '8rem' }}>
            <div className="container">
                <div className="section-header">
                    <h4>Benchmarks</h4>
                    <h2>Security Landscape.</h2>
                    <p>InterLink achieves deterministic finality without the latency or security tradeoffs of multisig-based bridges.</p>
                </div>

                <div className="table-wrap glass-panel">
                    <table className="comp-table">
                        <thead>
                            <tr>
                                <th>Protocol</th>
                                <th>Security Model</th>
                                <th>Proof Cost</th>
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
