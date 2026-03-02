import React from 'react'
import { motion } from 'framer-motion'
import { Search, ExternalLink, Hash, Clock, ArrowRight, CheckCircle, Zap } from 'lucide-react'

const Explorer = () => {
    const transactions = [
        { id: '0x7d2...f1a', source: 'Ethereum', dest: 'Solana', amount: '1.5 ETH', time: '2 mins ago', status: 'Success' },
        { id: '0x3a4...e92', source: 'Ethereum', dest: 'Solana', amount: '500 USDT', time: '12 mins ago', status: 'Success' },
        { id: '0x9b1...c43', source: 'Solana', dest: 'Ethereum', amount: '25 SOL', time: '45 mins ago', status: 'Success' },
        { id: '0x1f8...a0c', source: 'Ethereum', dest: 'Solana', amount: '0.1 ETH', time: '1 hour ago', status: 'Success' },
        { id: '0x6e2...d54', source: 'Arbitrum', dest: 'Solana', amount: '1200 USDC', time: '3 hours ago', status: 'Success' },
    ]

    return (
        <div className="page-container" style={{ paddingTop: '8rem', paddingBottom: '6rem' }}>
            <div className="container">
                <div className="section-header" style={{ marginBottom: '3rem' }}>
                    <span className="doc-eyebrow">Explorer</span>
                    <h1 className="doc-title" style={{ fontSize: '2.5rem' }}>Network Transactions</h1>
                    <p className="doc-lead" style={{ border: 'none', padding: 0 }}>Real-time feed of cross-chain messages verified by InterLink.</p>
                </div>

                <div className="glass-panel" style={{ padding: '0.4rem 1.25rem', marginBottom: '2rem', display: 'flex', alignItems: 'center', gap: '1rem', maxWidth: '600px' }}>
                    <Search size={18} className="text-3" />
                    <input
                        type="text"
                        placeholder="Search by Transaction Hash / Address / Message ID"
                        style={{ background: 'none', border: 'none', padding: '0.8rem 0', color: '#fff', fontSize: '0.9rem', outline: 'none', width: '100%' }}
                    />
                </div>

                <div className="glass-panel" style={{ overflow: 'hidden' }}>
                    <div style={{ overflowX: 'auto' }}>
                        <table style={{ width: '100%', borderCollapse: 'collapse', textAlign: 'left', minWidth: '800px' }}>
                            <thead>
                                <tr style={{ borderBottom: '1px solid var(--border)', background: 'rgba(255,255,255,0.02)' }}>
                                    <th style={{ padding: '1.25rem 1.5rem', fontSize: '0.75rem', fontWeight: 600, color: 'var(--text-3)' }}>TRANSACTION HASH</th>
                                    <th style={{ padding: '1.25rem 1.5rem', fontSize: '0.75rem', fontWeight: 600, color: 'var(--text-3)' }}>ROUTE</th>
                                    <th style={{ padding: '1.25rem 1.5rem', fontSize: '0.75rem', fontWeight: 600, color: 'var(--text-3)' }}>AMOUNT</th>
                                    <th style={{ padding: '1.25rem 1.5rem', fontSize: '0.75rem', fontWeight: 600, color: 'var(--text-3)' }}>TIME</th>
                                    <th style={{ padding: '1.25rem 1.5rem', fontSize: '0.75rem', fontWeight: 600, color: 'var(--text-3)' }}>STATUS</th>
                                    <th style={{ padding: '1.25rem 1.5rem', fontSize: '0.75rem', fontWeight: 600, color: 'var(--text-3)' }}></th>
                                </tr>
                            </thead>
                            <tbody>
                                {transactions.map((tx, i) => (
                                    <motion.tr
                                        initial={{ opacity: 0, y: 10 }}
                                        animate={{ opacity: 1, y: 0 }}
                                        transition={{ delay: i * 0.1 }}
                                        key={tx.id}
                                        style={{ borderBottom: '1px solid var(--border)', transition: 'background 0.2s' }}
                                        className="explorer-row"
                                    >
                                        <td style={{ padding: '1.25rem 1.5rem' }}>
                                            <div style={{ display: 'flex', alignItems: 'center', gap: '0.6rem' }}>
                                                <div className="glass-panel" style={{ padding: '0.3rem', borderRadius: '4px' }}>
                                                    <Hash size={12} className="text-blue" />
                                                </div>
                                                <span style={{ fontSize: '0.85rem', color: 'var(--blue)', fontWeight: 500 }}>{tx.id}</span>
                                            </div>
                                        </td>
                                        <td style={{ padding: '1.25rem 1.5rem' }}>
                                            <div style={{ display: 'flex', alignItems: 'center', gap: '0.6rem', fontSize: '0.85rem' }}>
                                                <span style={{ color: 'var(--text)' }}>{tx.source}</span>
                                                <ArrowRight size={12} className="text-3" />
                                                <span style={{ color: 'var(--text)' }}>{tx.dest}</span>
                                            </div>
                                        </td>
                                        <td style={{ padding: '1.25rem 1.5rem', fontSize: '0.85rem', fontWeight: 600 }}>{tx.amount}</td>
                                        <td style={{ padding: '1.25rem 1.5rem', fontSize: '0.85rem', color: 'var(--text-2)' }}>
                                            <div style={{ display: 'flex', alignItems: 'center', gap: '0.4rem' }}>
                                                <Clock size={12} />
                                                {tx.time}
                                            </div>
                                        </td>
                                        <td style={{ padding: '1.25rem 1.5rem' }}>
                                            <div style={{ display: 'flex', alignItems: 'center', gap: '0.4rem' }}>
                                                <div style={{ width: 6, height: 6, borderRadius: '50%', background: 'var(--green)' }} />
                                                <span style={{ fontSize: '0.8rem', color: 'var(--green)', fontWeight: 600 }}>{tx.status}</span>
                                            </div>
                                        </td>
                                        <td style={{ padding: '1.25rem 1.5rem', textAlign: 'right' }}>
                                            <button className="btn btn-ghost" style={{ padding: '0.4rem' }}>
                                                <ExternalLink size={14} className="text-3" />
                                            </button>
                                        </td>
                                    </motion.tr>
                                ))}
                            </tbody>
                        </table>
                    </div>
                </div>

                <div className="glass-panel" style={{ marginTop: '2rem', padding: '1.5rem', display: 'flex', alignItems: 'center', gap: '2rem' }}>
                    <div>
                        <div style={{ fontSize: '0.7rem', color: 'var(--text-3)', fontWeight: 600, marginBottom: '0.25rem' }}>TOTAL MESSAGES</div>
                        <div style={{ fontSize: '1.5rem', fontWeight: 800 }}>1.2M+</div>
                    </div>
                    <div style={{ width: 1, height: 40, background: 'var(--border)' }} />
                    <div>
                        <div style={{ fontSize: '0.7rem', color: 'var(--text-3)', fontWeight: 600, marginBottom: '0.25rem' }}>TOTAL VOLUME</div>
                        <div style={{ fontSize: '1.5rem', fontWeight: 800 }}>$840M+</div>
                    </div>
                    <div style={{ width: 1, height: 40, background: 'var(--border)' }} />
                    <div style={{ marginLeft: 'auto', display: 'flex', gap: '0.5rem', alignItems: 'center' }}>
                        <Zap size={14} className="text-blue" />
                        <span style={{ fontSize: '0.8rem', color: 'var(--text-2)' }}>Finality in &lt; 2 minutes</span>
                    </div>
                </div>
            </div>
        </div>
    )
}

export default Explorer
