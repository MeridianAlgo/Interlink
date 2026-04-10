# API & SDK Reference

InterLink provides a JSON-RPC and REST API via the relayer, as well as a TypeScript SDK for direct integration.

## 1. Relayer REST API

The relayer (by default) listens on port `3030`.

### Health Check
`GET /health`
- **Response**: `{"status": "ok", "version": "1.0.0"}`

### Get Transfer Quote
`GET /quote?from=1&to=900&amount=1000000`
- **Params**:
  - `from`: Source Chain ID (e.g., 1 for Ethereum)
  - `to`: Destination Chain ID (e.g., 900 for Solana)
  - `amount`: Amount in base units
- **Response**:
  ```json
  {
    "fee": "500",
    "estimated_time_seconds": 25,
    "route": "direct"
  }
  ```

### Bridge Simulation
`POST /simulate`
- **Body**:
  ```json
  {
    "source_chain": 1,
    "dest_chain": 900,
    "amount": "1000000",
    "sender": "0x...",
    "receiver": "..."
  }
  ```
- **Response**: Returns pre-flight check results (liquidity, rate limits, etc.).

---

## 2. TypeScript SDK (@interlink/sdk)

### Installation
```bash
yarn add @interlink/sdk
```

### Basic Usage

#### Initializing the Provider
```typescript
import { InterlinkProvider } from "@interlink/sdk";

const provider = new InterlinkProvider({
  relayerUrl: "https://relayer.interlink.protocol",
  solanaRpcUrl: "https://api.mainnet-beta.solana.com"
});
```

#### Initiating a Transfer (EVM -> Solana)
```typescript
const tx = await provider.transfer({
  fromChain: "ethereum",
  toChain: "solana",
  asset: "USDC",
  amount: "100.0",
  receiver: "SolanaAddress..."
});

console.log("Transfer initialized:", tx.hash);
```

#### Tracking Status
```typescript
const status = await provider.getStatus(tx.hash);
// States: PENDING_FINALITY -> PROVING -> SUBMITTING -> COMPLETED
```

---

## 3. Prometheus Metrics

The relayer exposes `/metrics` in Prometheus format. Key metrics include:
- `proof_gen_ms_p99`: Tail latency for ZK proof generation.
- `settlement_ms_p99`: E2E settlement time.
- `active_relays`: Count of ongoing transfers.
