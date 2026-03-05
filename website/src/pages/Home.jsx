import React from "react";
import { motion } from "framer-motion";
import { Link } from "react-router-dom";
import {
  ArrowRight,
  Shield,
  Zap,
  Globe,
  Lock,
  GitBranch,
  Cpu,
  CheckCircle,
  AlertTriangle,
  Activity,
  Server,
  Layout,
} from "lucide-react";

import logo from "../InterLink.png";
const LiveMetrics = () => {
  const [block, setBlock] = React.useState(1948271);
  const [tps, setTps] = React.useState(42.5);

  React.useEffect(() => {
    const interval = setInterval(() => {
      setBlock((b) => b + 1);
      setTps((t) => (40 + Math.random() * 5).toFixed(1));
    }, 3000);
    return () => clearInterval(interval);
  }, []);

  return (
    <div
      style={{
        display: "flex",
        gap: "2rem",
        justifyContent: "center",
        padding: "1rem 0",
      }}
    ></div>
  );
};

const featureIn = { hidden: { opacity: 0, y: 16 }, show: { opacity: 1, y: 0 } };

const FEATURES = [
  {
    icon: Lock,
    title: "zk SNARK Verified",
    desc: "Every cross chain message is proven using the Halo2 proving system with BN254 curve arithmetic. No multisigs, no trust assumptions just math.",
  },
  {
    icon: Zap,
    title: "Solana Execution Hub",
    desc: "The Hub is an on-chain Anchor program that verifies proofs in O(1) and manages global state across all connected chains.",
  },
  {
    icon: Globe,
    title: "Hub-and-Spoke Topology",
    desc: "O(N) connection complexity. Add a new chain by connecting it to the Hub no pairwise bridge deployments, no coordination complexity.",
  },
  {
    icon: GitBranch,
    title: "Recursive Proof Aggregation",
    desc: "Using Pallas/Vesta curve cycling, multiple transaction proofs are folded into one. Amortized on-chain verification cost approaches O(1).",
  },
  {
    icon: Cpu,
    title: "Concurrent Relayer Network",
    desc: "Relayers run Tokio-based async engines. A 1024-message channel buffer keeps the proof pipeline saturated with zero blocking.",
  },
  {
    icon: Shield,
    title: "Replay Proof by Design",
    desc: "Every Gateway tracks executed nonces. The CEI (Checks-Effects-Interactions) pattern prevents reentrancy, and sequence numbers prevent double-processing.",
  },
];

const COMPARISON = [
  {
    name: "LayerZero",
    security: "Multisig + DVN",
    cost: "Medium",
    latency: "5 20 min",
    finality: "Probabilistic",
    hl: false,
  },
  {
    name: "Axelar",
    security: "Multisig Guards",
    cost: "Low",
    latency: "1 5 min",
    finality: "Probabilistic",
    hl: false,
  },
  {
    name: "Wormhole",
    security: "Multisig Guards",
    cost: "Low",
    latency: "1 5 min",
    finality: "Probabilistic",
    hl: false,
  },
  {
    name: "IBC",
    security: "Light Clients",
    cost: "High",
    latency: "10 30 min",
    finality: "Deterministic",
    hl: false,
  },
  {
    name: "Chainlink CCIP",
    security: "Oracle Networks",
    cost: "Medium",
    latency: "5 15 min",
    finality: "Probabilistic",
    hl: false,
  },
  {
    name: "InterLink",
    security: "zk SNARK Proofs",
    cost: "Low O(1)",
    latency: "1 5 min",
    finality: "Deterministic",
    hl: true,
  },
];

const FLOW = [
  {
    title: "User calls the Gateway",
    desc: "sendCrossChainMessage() locks assets/ETH in the EVM Gateway, increments a nonce, and emits MessagePublished.",
  },
  {
    title: "Relayer picks up the event",
    desc: "A WebSocket listener detects the event. After source-chain finality, the Relayer fetches the Merkle proof and block headers.",
  },
  {
    title: "Halo2 proof is generated",
    desc: "The InterlinkCircuit synthesizes a ZK-SNARK over BN254. The cubic s-box commitment formula is: C = (payload + 0x1337)³ + seq.",
  },
  {
    title: "Proof submitted to Hub",
    desc: "The Relayer serializes the Anchor instruction, signs it with an ed25519 key, and posts it to the Solana Hub via JSON-RPC.",
  },
  {
    title: "Hub verifies and executes",
    desc: "The Hub calls the BN254 pairing precompile (0x08). A valid e(a,b)·e(c,−g₂)=1 result triggers message execution on the destination Gateway.",
  },
];

const Home = () => (
  <div>
    <section
      className="hero"
      style={{
        paddingBottom: "4rem",
        paddingTop: "8rem",
        position: "relative",
        overflow: "hidden",
      }}
    >
      {/* Ambient background glows for premium feel */}
      <div
        style={{
          position: "absolute",
          top: "-10%",
          left: "50%",
          transform: "translateX(-50%)",
          width: "800px",
          height: "400px",
          background:
            "radial-gradient(ellipse at center, rgba(255, 255, 255, 0.05), transparent 70%)",
          zIndex: 0,
        }}
      />

      <div className="container" style={{ position: "relative", zIndex: 10 }}>
        <motion.div
          initial="hidden"
          animate="show"
          variants={featureIn}
          style={{ textAlign: "center" }}
        >
          <div
            className="hero-eyebrow"
            style={{
              margin: "0 auto 1.5rem",
              background: "rgba(255, 255, 255, 0.05)",
              border: "1px solid rgba(255, 255, 255, 0.15)",
              color: "#fff",
              padding: "0.4rem 1rem",
              borderRadius: "2rem",
            }}
          >
            v0.7.3
          </div>

          <h1
            className="text-gradient"
            style={{
              fontSize: "clamp(3.5rem, 8vw, 6rem)",
              lineHeight: 1,
              fontWeight: 900,
              letterSpacing: "-0.04em",
              margin: "0 auto 1.5rem",
              maxWidth: "900px",
            }}
          >
            Zero Knowledge. Infinite Scale.
          </h1>

          <p
            style={{
              maxWidth: "680px",
              margin: "0 auto 2rem",
              color: "#a0a0a0",
              fontSize: "1.35rem",
              lineHeight: 1.6,
              fontWeight: 400,
            }}
          >
            The first mathematically proven interoperability protocol. Seamless
            liquidity across EVM, Solana, and beyond with O(1) continuous
            finality.
          </p>

          <LiveMetrics />

          <div
            className="hero-actions"
            style={{
              justifyContent: "center",
              gap: "1.5rem",
              display: "flex",
              flexWrap: "wrap",
            }}
          >
            <Link
              to="/bridge"
              className="btn"
              style={{
                padding: "1.1rem 2.5rem",
                fontSize: "1.1rem",
                borderRadius: "3rem",
                fontWeight: 600,
                boxShadow: "0 0 30px rgba(255, 255, 255, 0.1)",
                textTransform: "uppercase",
                letterSpacing: "0.05em",
                color: "#000",
                backgroundColor: "#fff",
              }}
            >
              Start Bridging
            </Link>
            <Link
              to="/docs"
              className="btn btn-ghost"
              style={{
                padding: "1.1rem 2.5rem",
                fontSize: "1.1rem",
                borderRadius: "3rem",
                border: "1px solid rgba(255,255,255,0.2)",
                backgroundColor: "rgba(255,255,255,0.03)",
                backdropFilter: "blur(10px)",
                color: "#fff",
              }}
            >
              Read Specifications
            </Link>
          </div>
        </motion.div>
      </div>
    </section>

    <hr className="divider" />

    {/* ── Stats ─────────────────────────── */}
    <div className="stats-bar">
      {[
        { val: "O(1)", label: "Verification" },
        { val: "~100 B", label: "Proof size" },
        { val: "BN254", label: "EVM precompile" },
        { val: "Halo2", label: "Prover" },
      ].map((s) => (
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
          <p>
            Every component is designed so that no participant needs to trust
            another only the cryptographic proof.
          </p>
        </div>

        <motion.div
          className="feature-grid glass-panel"
          initial="hidden"
          whileInView="show"
          viewport={{ once: true }}
          variants={{ show: { transition: { staggerChildren: 0.07 } } }}
        >
          {FEATURES.map((f) => (
            <motion.div
              key={f.title}
              className="feature-cell"
              variants={featureIn}
            >
              <div className="feature-icon">
                <f.icon size={16} />
              </div>
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
            <div className="section-header" style={{ marginBottom: "1.5rem" }}>
              <h4>Lifecycle</h4>
              <h2>Deterministic paths.</h2>
              <p>
                Cross-chain messages move through five verified steps from
                source to destination.
              </p>
            </div>
            <Link
              to="/docs/lifecycle"
              className="btn btn-ghost"
              style={{ marginTop: "0.5rem" }}
            >
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

    {/* ── Architecture Deep Dive (Documentation) ──────────────────────────── */}
    <section className="pad" style={{ background: "var(--bg-1)" }}>
      <div className="container">
        <div className="section-header" style={{ textAlign: "center" }}>
          <h4>Architecture Documentation</h4>
          <h2>The Relayer & Prover Engine.</h2>
          <p style={{ margin: "0 auto", maxWidth: "600px" }}>
            Under the hood, InterLink orchestrates complex cryptographic
            constraints over standard compute. The Relayers act as untrusted
            builders that synthesize recursive ZK-SNARKs.
          </p>
        </div>

        <div
          style={{
            display: "grid",
            gridTemplateColumns: "repeat(auto-fit, minmax(400px, 1fr))",
            gap: "2rem",
            marginTop: "3rem",
          }}
        >
          <div className="glass-panel" style={{ padding: "2rem" }}>
            <h3
              style={{
                marginBottom: "1rem",
                display: "flex",
                alignItems: "center",
                gap: "0.5rem",
              }}
            >
              <Server size={18} />
              v0.7.3 Simplifyed Layers
            </h3>
            <p style={{ fontSize: "0.9rem", marginBottom: "1.5rem" }}>
              The rust <code>interlink-core</code> library wraps `halo2_proofs`
              to produce BatchedInterlinkCircuit constraints.
              This enables amortizing the gas cost of verifying $N$ payloads
              inside a single SNARK.
            </p>
            <pre
              style={{
                margin: 0,
                fontSize: "0.8rem",
                background: "#000",
                border: "1px solid #333",
              }}
            >
              <code>{`// Batch compiling payloads inside Circuit
for i in 0..BATCH_SIZE {
    let state_in = self.payloads[i];
    let seq = self.sequence_numbers[i];
    
    let out_cell = chip.hash_round(
        state_in, round_const, seq
    )?;
    
    layouter.constrain_instance(
        out_cell.cell(), instance, i
    )?;
}`}</code>
            </pre>
          </div>

          <div className="glass-panel" style={{ padding: "2rem" }}>
            <h3
              style={{
                marginBottom: "1rem",
                display: "flex",
                alignItems: "center",
                gap: "0.5rem",
              }}
            >
              <Layout size={18} />
              Solana PDA Resolution
            </h3>
            <p style={{ fontSize: "0.9rem", marginBottom: "1.5rem" }}>
              Using `ed25519-dalek` off-curve validation, the relayer natively
              crafts cross chain execution payload bounds without depending on
              external Typescript SDK tooling.
            </p>
            <pre
              style={{
                margin: 0,
                fontSize: "0.8rem",
                background: "#000",
                border: "1px solid #333",
              }}
            >
              <code>{`// Deterministic Ed25519 off-curve validation
for bump in (0..=255).rev() {
    let result = hasher.finalize();
    registry_pda.copy_from_slice(&result);

    // If it errors, it's NOT a valid point,
    // thereby satisfying Solana PDA rules!
    if VerifyingKey::from_bytes(&registry_pda).is_err() {
        break; // Safe PDA acquired.
    }
}`}</code>
            </pre>
          </div>
        </div>
      </div>
    </section>

    {/* ── Comparison ────────────────────── */}
    <section className="pad" style={{ paddingBottom: "8rem" }}>
      <div className="container">
        <div className="section-header">
          <h4>Benchmarks</h4>
          <h2>Security Landscape.</h2>
          <p>
            InterLink achieves deterministic finality without the latency or
            security tradeoffs of multisig-based bridges.
          </p>
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
              {COMPARISON.map((r) => (
                <tr key={r.name} className={r.hl ? "highlight" : ""}>
                  <td>
                    {r.name}
                  </td>
                  <td>{r.security}</td>
                  <td>{r.cost}</td>
                  <td>{r.latency}</td>
                  <td>
                    <span
                      className={`tag ${r.finality === "Deterministic" ? "tag-green" : "tag-orange"}`}
                    >
                      {r.finality === "Deterministic" ? (
                        <CheckCircle size={10} />
                      ) : (
                        <AlertTriangle size={10} />
                      )}
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
);

export default Home;
