import React, { useState } from 'react'
import { motion } from 'framer-motion'
import { ArrowDown, Coins, Shield, Zap, Info, ChevronRight, CheckCircle } from 'lucide-react'

const Bridge = () => {
    const [amount, setAmount] = useState('')
    const [fromChain, setFromChain] = useState('Ethereum')
    const [toChain, setToChain] = useState('Solana')
    const [loading, setLoading] = useState(false)
    const [step, setStep] = useState(0) // 0: Idle, 1: Confirm, 2: Proving, 3: Success

    const handleBridge = () => {
        setLoading(true)
        setStep(1)
        setTimeout(() => setStep(2), 1500)
        setTimeout(() => setStep(3), 4000)
        setTimeout(() => {
            setLoading(false)
            setStep(0)
            setAmount('')
        }, 8000)
    }

    return (
        <div className="page-container" style={{ paddingTop: '8rem', paddingBottom: '6rem' }}>
            <motion.div
                initial={{ opacity: 0, y: 20 }}
                animate={{ opacity: 1, y: 0 }}
                className="container"
                style={{ maxWidth: '440px' }}
            >
                <div className="section-header" style={{ textAlign: 'center', marginBottom: '2rem' }}>
                    <span className="doc-eyebrow">Trustless Bridge</span>
                    <h1 style={{ fontSize: '2rem', marginBottom: '0.5rem' }}>Transfer Assets</h1>
                    <p style={{ color: 'var(--text-2)', fontSize: '0.9rem' }}>Powered by Zero-Knowledge Proofs</p>
                </div>

                <div className="glass-panel" style={{ padding: '1.5rem', borderRadius: '1.5rem' }}>
                    {/* From Chain */}
                    <div style={{ marginBottom: '1.5rem' }}>
                        <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: '0.5rem', fontSize: '0.75rem', fontWeight: 600, color: 'var(--text-3)' }}>
                            <span>FROM</span>
                            <span>BALANCE: 1.42 ETH</span>
                        </div>
                        <div className="glass-panel" style={{ padding: '0.75rem 1rem', display: 'flex', alignItems: 'center', gap: '0.75rem', background: 'rgba(255,255,255,0.03)' }}>
                            <div style={{ width: 24, height: 24, borderRadius: '50%', background: 'var(--blue)', display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
                                <Zap size={14} color="#fff" />
                            </div>
                            <span style={{ fontWeight: 600, fontSize: '0.9rem' }}>{fromChain}</span>
                            <div style={{ marginLeft: 'auto', textAlign: 'right' }}>
                                <input
                                    type="number"
                                    placeholder="0.0"
                                    value={amount}
                                    onChange={(e) => setAmount(e.target.value)}
                                    style={{ background: 'none', border: 'none', textAlign: 'right', color: '#fff', fontSize: '1.2rem', fontWeight: 700, outline: 'none', width: '120px' }}
                                />
                            </div>
                        </div>
                    </div>

                    <div style={{ display: 'flex', justifyContent: 'center', margin: '-0.75rem 0 0.75rem' }}>
                        <div className="glass-panel" style={{ padding: '0.5rem', borderRadius: '50%', background: 'var(--bg-1)', border: '1px solid var(--blue-border)', zIndex: 1 }}>
                            <ArrowDown size={14} className="text-blue" />
                        </div>
                    </div>

                    {/* To Chain */}
                    <div style={{ marginBottom: '1.5rem' }}>
                        <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: '0.5rem', fontSize: '0.75rem', fontWeight: 600, color: 'var(--text-3)' }}>
                            <span>TO (ESTIMATED)</span>
                            <span>BALANCE: 12.0 SOL</span>
                        </div>
                        <div className="glass-panel" style={{ padding: '0.75rem 1rem', display: 'flex', alignItems: 'center', gap: '0.75rem', background: 'rgba(255,255,255,0.03)' }}>
                            <div style={{ width: 24, height: 24, borderRadius: '50%', background: 'var(--green)', display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
                                <Shield size={14} color="#fff" />
                            </div>
                            <span style={{ fontWeight: 600, fontSize: '0.9rem' }}>{toChain}</span>
                            <div style={{ marginLeft: 'auto', textAlign: 'right', fontSize: '1.2rem', fontWeight: 700, color: 'var(--text-2)' }}>
                                {amount ? (parseFloat(amount) * 48.5).toFixed(2) : '0.00'}
                            </div>
                        </div>
                    </div>

                    {/* Rate & Fee Info */}
                    <div className="glass-panel" style={{ padding: '1rem', background: 'rgba(59, 130, 246, 0.03)', marginBottom: '1.5rem', borderRadius: '1rem' }}>
                        <div style={{ display: 'flex', justifyContent: 'space-between', fontSize: '0.75rem', marginBottom: '0.4rem' }}>
                            <span style={{ color: 'var(--text-3)' }}>Rate</span>
                            <span>1 ETH = 48.5 SOL</span>
                        </div>
                        <div style={{ display: 'flex', justifyContent: 'space-between', fontSize: '0.75rem' }}>
                            <span style={{ color: 'var(--text-3)' }}>Prover Fee (ZK)</span>
                            <span style={{ color: 'var(--blue)' }}>0.002 ETH</span>
                        </div>
                    </div>

                    {/* Action Button */}
                    <button
                        className={`btn ${loading ? 'btn-ghost' : 'btn-primary'}`}
                        style={{ width: '100%', padding: '1rem', borderRadius: '1rem', display: 'flex', alignItems: 'center', justifyContent: 'center', gap: '0.5rem' }}
                        disabled={!amount || loading}
                        onClick={handleBridge}
                    >
                        {loading ? 'Processing...' : 'Transfer Assets'}
                    </button>
                </div>

                {/* Status Stepper */}
                {loading && (
                    <motion.div
                        initial={{ opacity: 0, y: 10 }}
                        animate={{ opacity: 1, y: 0 }}
                        className="glass-panel"
                        style={{ marginTop: '1.5rem', padding: '1.5rem' }}
                    >
                        <div style={{ display: 'flex', flexDirection: 'column', gap: '1rem' }}>
                            <div style={{ display: 'flex', alignItems: 'center', gap: '0.75rem' }}>
                                <div style={{ width: 20, height: 20, borderRadius: '50%', background: step >= 1 ? 'var(--green)' : 'var(--bg-3)', display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
                                    {step > 1 ? <CheckCircle size={12} /> : <div style={{ width: 6, height: 6, borderRadius: '50%', background: '#fff' }} />}
                                </div>
                                <span style={{ fontSize: '0.85rem', color: step >= 1 ? '#fff' : 'var(--text-3)' }}>Confirming Source Finality</span>
                                {step === 1 && <div className="pulse-dot" style={{ marginLeft: 'auto' }} />}
                            </div>
                            <div style={{ display: 'flex', alignItems: 'center', gap: '0.75rem' }}>
                                <div style={{ width: 20, height: 20, borderRadius: '50%', background: step >= 2 ? 'var(--green)' : 'var(--bg-3)', display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
                                    {step > 2 ? <CheckCircle size={12} /> : <div style={{ width: 6, height: 6, borderRadius: '50%', background: '#fff' }} />}
                                </div>
                                <span style={{ fontSize: '0.85rem', color: step >= 2 ? '#fff' : 'var(--text-3)' }}>Generating Halo2 SNARK</span>
                                {step === 2 && <div className="pulse-dot" style={{ marginLeft: 'auto' }} />}
                            </div>
                            <div style={{ display: 'flex', alignItems: 'center', gap: '0.75rem' }}>
                                <div style={{ width: 20, height: 20, borderRadius: '50%', background: step >= 3 ? 'var(--green)' : 'var(--bg-3)', display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
                                    {step > 3 ? <CheckCircle size={12} /> : <div style={{ width: 6, height: 6, borderRadius: '50%', background: '#fff' }} />}
                                </div>
                                <span style={{ fontSize: '0.85rem', color: step >= 3 ? '#fff' : 'var(--text-3)' }}>Verifying on Solana Hub</span>
                                {step === 3 && <div className="pulse-dot" style={{ marginLeft: 'auto' }} />}
                            </div>
                        </div>
                    </motion.div>
                )}

                <div className="info-banner" style={{ marginTop: '2rem', textAlign: 'center' }}>
                    <div style={{ display: 'inline-flex', alignItems: 'center', gap: '0.5rem', color: 'var(--text-3)', fontSize: '0.75rem' }}>
                        <Shield size={12} />
                        Your funds are secured by cryptographic proofs, not a multisig.
                    </div>
                </div>
            </motion.div>
        </div>
    )
}

export default Bridge
