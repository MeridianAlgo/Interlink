# Competitive Landscape Comparison

| Bridge | Settlement Time | Throughput | Validation Model | Key Drawback | Interlink Advantage |
|---|---|---|---|---|---|
| **Wormhole** | 2-15m | High | 19 Guardians | Costly VAA fees, slow EVM | 15s deterministic ZK latency |
| **LayerZero / Stargate** | 1-2m | Medium | Oracle/Relayer pair | High native gas abstraction overhead | Cheaper parallel relayer compute |
| **Synapse** | 1-3m | High | Validator consensus | Slippage on deep pools | Fully deterministic intent paths |
| **Across** | 5-60m | Medium | Optimistic | Optimistic challenge windows | Zero-knowledge immediacy |
| **Lifi/Socket** | N/A | Variable| Multi-bridge router | Meta-routing latency + slippage | Native settlement |

### Where Interlink Wins
- **Finality**: 15s guaranteed via recursive SNARKs.
- **Batch execution**: Handling 1000 txs inside a single verified proof payload drastically amortizes per-user fees to $0 for tiers <$1k.

### Where Interlink Loses (Currently)
- **Chain diversity**: Only covering core EVM/Solana implementations during Phase 1 rollout compared to Wormhole’s expansive 30+ chains.
