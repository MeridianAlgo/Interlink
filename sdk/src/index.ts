/**
 * @interlink/sdk — InterLink ZK cross-chain bridge SDK
 *
 * Zero-fee bridging under $1,000. ZK-SNARK security. <30s settlement.
 *
 * @example
 * ```typescript
 * import { InterlinkClient, ChainId } from "@interlink/sdk";
 *
 * const client = new InterlinkClient({ relayerUrl: "http://localhost:8080" });
 *
 * // Tier 1: $0 fee for small transfers
 * const quote = await client.getQuote({
 *   sourceChain: ChainId.Ethereum,
 *   destChain: ChainId.Solana,
 *   amountWei: 50_000_000_000_000_000n, // 0.05 ETH
 *   usdCents: 15_000, // $150 — Tier 1, free
 * });
 * console.log(quote.feeBps); // 0
 *
 * // Register a webhook for real-time events
 * const webhook = await client.registerWebhook(
 *   "https://api.myapp.com/hooks",
 *   ["settlement.complete", "transfer.failed"]
 * );
 * ```
 *
 * @packageDocumentation
 */

export { InterlinkClient, InterlinkApiError, QuoteExpiredError, SDK_VERSION } from "./client";
export {
  ChainId,
  TransferStatus,
  CHAIN_NAMES,
  CHAIN_FINALITY_SECONDS,
  DEFAULT_RELAYER_URL,
} from "./types";
export type {
  InterlinkConfig,
  TransferParams,
  TransferQuote,
  Transfer,
  SwapParams,
  FeeComparison,
  CompetitorFee,
  FeeTier,
  WebhookEventType,
  WebhookRegistration,
} from "./types";
