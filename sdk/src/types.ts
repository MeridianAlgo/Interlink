/**
 * Core types for the @interlink/sdk
 */

// ─── Chain identifiers ────────────────────────────────────────────────────────

/** Supported source/destination chains. */
export enum ChainId {
  Ethereum = 1,
  Optimism = 10,
  BNBChain = 56,
  Polygon = 137,
  ArbitrumOne = 42161,
  ArbitrumNova = 42170,
  Base = 8453,
  Solana = 900, // InterLink internal ID for Solana Hub
}

export const CHAIN_NAMES: Record<ChainId, string> = {
  [ChainId.Ethereum]: "Ethereum",
  [ChainId.Optimism]: "Optimism",
  [ChainId.BNBChain]: "BNB Chain",
  [ChainId.Polygon]: "Polygon PoS",
  [ChainId.ArbitrumOne]: "Arbitrum One",
  [ChainId.ArbitrumNova]: "Arbitrum Nova",
  [ChainId.Base]: "Base",
  [ChainId.Solana]: "Solana",
};

/** Finality wait time per chain in seconds. */
export const CHAIN_FINALITY_SECONDS: Partial<Record<ChainId, number>> = {
  [ChainId.Ethereum]: 12,
  [ChainId.Optimism]: 2,
  [ChainId.Base]: 2,
  [ChainId.ArbitrumOne]: 2,
  [ChainId.ArbitrumNova]: 2,
  [ChainId.Polygon]: 5,
};

// ─── Transfer types ───────────────────────────────────────────────────────────

/** Status of a cross-chain transfer. */
export enum TransferStatus {
  /** Transfer submitted on source chain, waiting for finality. */
  PendingFinality = "pending_finality",
  /** Source block is final, ZK proof being generated. */
  PendingProof = "pending_proof",
  /** ZK proof generated, submitting to Solana Hub. */
  PendingSettlement = "pending_settlement",
  /** Transfer complete — funds released on destination. */
  Complete = "complete",
  /** Transfer failed at some stage. */
  Failed = "failed",
}

/** A cross-chain transfer quote. */
export interface TransferQuote {
  /** Source chain. */
  sourceChain: ChainId;
  /** Destination chain. */
  destChain: ChainId;
  /** Amount to transfer in wei (source token). */
  amountWei: bigint;
  /** USD value of the transfer in cents. */
  usdCents: number;
  /** Protocol fee in wei. 0 for Tier 1 transfers (<$1,000). */
  feeWei: bigint;
  /** Fee tier name. */
  feeTier: string;
  /** Fee rate in basis points. */
  feeBps: number;
  /** Estimated settlement time in seconds. */
  estimatedSettlementSecs: number;
  /** Source chain gas estimate in wei. */
  sourceGasWei: bigint;
  /** Destination Solana fee in lamports. */
  destFeeLamports: number;
  /** Quote expiry timestamp (Unix seconds). */
  expiresAt: number;
}

/** Parameters for initiating a cross-chain transfer. */
export interface TransferParams {
  /** Recipient address on the destination chain. */
  recipient: string;
  /** Destination chain ID. */
  destinationChain: ChainId;
  /** Token contract address (zero address = native ETH). */
  tokenAddress?: string;
  /** Amount in wei. */
  amountWei: bigint;
  /** Optional arbitrary payload (max 1024 bytes). */
  payload?: Uint8Array;
}

/** A submitted transfer with tracking info. */
export interface Transfer {
  /** Unique sequence number assigned by the gateway. */
  sequence: number;
  /** Source transaction hash. */
  txHash: string;
  /** Source block number. */
  blockNumber: number;
  /** Current status. */
  status: TransferStatus;
  /** Parameters used for this transfer. */
  params: TransferParams;
  /** Quote used for this transfer. */
  quote: TransferQuote;
  /** Solana Hub settlement signature (once complete). */
  settlementSignature?: string;
  /** Total elapsed time in ms (once complete). */
  totalElapsedMs?: number;
  /** Error message (if failed). */
  error?: string;
}

// ─── Swap types ───────────────────────────────────────────────────────────────

/** Parameters for initiating a cross-chain swap. */
export interface SwapParams {
  /** Destination chain. */
  destinationChain: ChainId;
  /** Recipient on destination. */
  recipient: string;
  /** Desired output token address on destination chain. */
  tokenOut: string;
  /** Minimum output amount (slippage protection). */
  minAmountOut: bigint;
  /** Optional swap routing data for destination DEX. */
  swapData?: Uint8Array;
}

// ─── Webhook types ────────────────────────────────────────────────────────────

/** Events emitted by the relayer that can be subscribed to. */
export type WebhookEventType =
  | "transfer.initiated"
  | "finality.confirmed"
  | "proof.generated"
  | "settlement.complete"
  | "transfer.failed"
  | "all";

/** Webhook registration request. */
export interface WebhookRegistration {
  id: string;
  url: string;
  events: WebhookEventType[];
  active: boolean;
  registeredAt: number;
  totalDelivered: number;
  totalFailed: number;
}

// ─── Fee tier types ───────────────────────────────────────────────────────────

/** Protocol fee tier. */
export interface FeeTier {
  name: "Zero" | "Standard" | "Institutional" | "OTC";
  bps: number;
  description: string;
}

/** Competitor fee comparison. */
export interface CompetitorFee {
  name: string;
  feeBps: number;
  feeUsdCents: number;
  settlementMinSecs: number;
  settlementMaxSecs: number;
}

/** Full fee comparison result. */
export interface FeeComparison {
  interlink: {
    feeBps: number;
    feeUsdCents: number;
    settlementTargetSecs: number;
    feeDescription: string;
  };
  competitors: CompetitorFee[];
  interlinkWinsFee: boolean;
  interlinkWinsSpeed: boolean;
  savingsVsCheapestCents: number;
}

// ─── SDK config ───────────────────────────────────────────────────────────────

/** Configuration for the InterLink SDK client. */
export interface InterlinkConfig {
  /**
   * InterLink relayer API base URL.
   * Defaults to the public devnet relayer.
   */
  relayerUrl?: string;
  /**
   * EVM provider URL for the source chain.
   * Can be an RPC URL string or an ethers Provider.
   */
  evmRpcUrl?: string;
  /** Request timeout in milliseconds (default: 10_000). */
  timeoutMs?: number;
  /** Whether to enable verbose SDK logging (default: false). */
  debug?: boolean;
}

export const DEFAULT_RELAYER_URL = "https://relayer.interlink.protocol";
