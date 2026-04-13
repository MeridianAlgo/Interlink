/**
 * InterlinkClient — main SDK entry point.
 *
 * Provides methods to:
 *   - Get fee quotes before bridging
 *   - Initiate cross-chain transfers and swaps
 *   - Track transfer status
 *   - Register webhook subscriptions
 *   - Compare costs vs Wormhole, Stargate, Across
 *
 * @example
 * ```typescript
 * import { InterlinkClient, ChainId } from "@interlink/sdk";
 *
 * const client = new InterlinkClient({
 *   relayerUrl: "http://localhost:8080",
 * });
 *
 * // Get a quote for a $100 transfer (should be free — Tier 1)
 * const quote = await client.getQuote({
 *   sourceChain: ChainId.Ethereum,
 *   destChain: ChainId.Solana,
 *   amountWei: BigInt("100000000000000000"), // 0.1 ETH
 *   usdCents: 30_000, // $300
 * });
 * console.log(`Fee: ${quote.feeBps} bps ($${quote.feeWei})`);
 *
 * // Compare with competitors
 * const cmp = await client.compareFeesAt(30_000);
 * console.log(`InterLink wins on fee: ${cmp.interlinkWinsFee}`);
 * console.log(`Savings vs cheapest: $${cmp.savingsVsCheapestCents / 100}`);
 * ```
 */

import {
  ChainId,
  FeeComparison,
  InterlinkConfig,
  Transfer,
  TransferParams,
  TransferQuote,
  TransferStatus,
  SwapParams,
  WebhookEventType,
  WebhookRegistration,
  DEFAULT_RELAYER_URL,
  CHAIN_FINALITY_SECONDS,
} from "./types";

/** Default timeout for API requests (ms). */
const DEFAULT_TIMEOUT_MS = 10_000;

/** @interlink/sdk version. Kept in sync with package.json. */
export const SDK_VERSION = "0.7.3";

// ─── InterlinkClient ──────────────────────────────────────────────────────────

export class InterlinkClient {
  private readonly relayerUrl: string;
  private readonly timeoutMs: number;
  private readonly debug: boolean;

  constructor(config: InterlinkConfig = {}) {
    this.relayerUrl = (config.relayerUrl ?? DEFAULT_RELAYER_URL).replace(/\/$/, "");
    this.timeoutMs = config.timeoutMs ?? DEFAULT_TIMEOUT_MS;
    this.debug = config.debug ?? false;
  }

  // ─── Health ────────────────────────────────────────────────────────────────

  /**
   * Check if the relayer is online and responsive.
   *
   * @returns `true` if the relayer is healthy.
   */
  async isHealthy(): Promise<boolean> {
    try {
      const resp = await this.get("/health");
      return resp.status === "ok";
    } catch {
      return false;
    }
  }

  // ─── Quotes ────────────────────────────────────────────────────────────────

  /**
   * Get a fee quote for a prospective transfer.
   *
   * Tier 1 ($0–$1,000): **free** — InterLink charges $0 while Wormhole charges $1–$20.
   * Tier 2 ($1k–$100k): 0.05% — beats Wormhole (0.1–0.2%) and Stargate (0.5–5%).
   * Tier 3 ($100k–$10M): 0.01% — beats Across (0.25–1%) by 25–100x.
   *
   * @example
   * ```typescript
   * const quote = await client.getQuote({
   *   sourceChain: ChainId.Ethereum,
   *   destChain: ChainId.Solana,
   *   amountWei: 1_000_000_000_000_000_000n, // 1 ETH
   *   usdCents: 300_000, // $3,000
   * });
   * ```
   */
  async getQuote(params: {
    sourceChain: ChainId;
    destChain: ChainId;
    amountWei: bigint;
    usdCents: number;
    gasPriceGwei?: number;
    batchSize?: number;
    ethUsd?: number;
  }): Promise<TransferQuote> {
    const qs = new URLSearchParams({
      amount: params.amountWei.toString(),
      usd_cents: params.usdCents.toString(),
      gas_gwei: (params.gasPriceGwei ?? 30).toString(),
      batch_size: (params.batchSize ?? 100).toString(),
      eth_usd: (params.ethUsd ?? 3000).toString(),
    });

    const data = await this.get(`/quote?${qs}`);

    const est = data.estimate;
    const interlink = data.comparison.interlink;
    const finality = CHAIN_FINALITY_SECONDS[params.sourceChain] ?? 12;

    return {
      sourceChain: params.sourceChain,
      destChain: params.destChain,
      amountWei: params.amountWei,
      usdCents: params.usdCents,
      feeWei: BigInt(est.protocol_fee_amount),
      feeTier: est.fee_tier.name,
      feeBps: est.fee_tier.bps,
      estimatedSettlementSecs: finality + 5, // finality + proof + submission
      sourceGasWei: BigInt(est.source_gas_cost_wei),
      destFeeLamports: est.dest_fee_lamports,
      expiresAt: Math.floor(Date.now() / 1000) + 60, // valid for 60s
    };
  }

  // ─── Transfers ─────────────────────────────────────────────────────────────

  /**
   * Initiate a cross-chain transfer.
   *
   * Submits a transfer request to the relayer, which handles finality
   * confirmation, ZK proof generation, and settlement on the destination chain.
   *
   * @param params - Transfer parameters (recipient, destination chain, amount).
   * @param quote - A valid quote obtained from {@link getQuote}. Must not be expired.
   * @returns The submitted transfer with a sequence number for tracking.
   *
   * @example
   * ```typescript
   * const quote = await client.getQuote({ ... });
   * const transfer = await client.transfer({
   *   recipient: "8xk2...",
   *   destinationChain: ChainId.Solana,
   *   amountWei: 1_000_000_000_000_000_000n,
   * }, quote);
   * console.log("Sequence:", transfer.sequence);
   * ```
   */
  async transfer(params: TransferParams, quote: TransferQuote): Promise<Transfer> {
    if (quote.expiresAt < Math.floor(Date.now() / 1000)) {
      throw new QuoteExpiredError(quote.expiresAt);
    }

    const data = await this.post("/transfers", {
      recipient: params.recipient,
      destination_chain: params.destinationChain,
      token_address: params.tokenAddress ?? "0x0000000000000000000000000000000000000000",
      amount_wei: params.amountWei.toString(),
      payload: params.payload ? Buffer.from(params.payload).toString("base64") : null,
      quote_expires_at: quote.expiresAt,
    });

    return {
      sequence: data.sequence,
      txHash: data.tx_hash,
      blockNumber: data.block_number,
      status: data.status as TransferStatus,
      params,
      quote,
      settlementSignature: data.settlement_signature ?? undefined,
      totalElapsedMs: data.total_elapsed_ms ?? undefined,
      error: data.error ?? undefined,
    };
  }

  /**
   * Initiate a cross-chain swap (bridge + DEX swap on destination).
   *
   * Combines bridging with a destination-chain swap in a single atomic operation.
   * Uses the intent solver to find the optimal path.
   *
   * @param params - Swap parameters (destination chain, output token, min amount).
   * @param amountWei - Amount to bridge in wei.
   * @returns The submitted transfer with swap details.
   *
   * @example
   * ```typescript
   * const transfer = await client.swap({
   *   destinationChain: ChainId.Solana,
   *   recipient: "8xk2...",
   *   tokenOut: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", // USDC on Solana
   *   minAmountOut: 3000_000_000n, // 3000 USDC (6 decimals)
   * }, 1_000_000_000_000_000_000n);
   * ```
   */
  async swap(params: SwapParams, amountWei: bigint): Promise<Transfer> {
    const data = await this.post("/swaps", {
      destination_chain: params.destinationChain,
      recipient: params.recipient,
      token_out: params.tokenOut,
      min_amount_out: params.minAmountOut.toString(),
      amount_wei: amountWei.toString(),
      swap_data: params.swapData ? Buffer.from(params.swapData).toString("base64") : null,
    });

    return {
      sequence: data.sequence,
      txHash: data.tx_hash,
      blockNumber: data.block_number,
      status: data.status as TransferStatus,
      params: {
        recipient: params.recipient,
        destinationChain: params.destinationChain,
        amountWei,
      },
      quote: data.quote,
      settlementSignature: data.settlement_signature ?? undefined,
      totalElapsedMs: data.total_elapsed_ms ?? undefined,
      error: data.error ?? undefined,
    };
  }

  /**
   * Get the current status of a transfer by sequence number.
   *
   * Poll this endpoint to track transfer progress through stages:
   * PendingFinality → PendingProof → PendingSettlement → Complete
   *
   * @param sequence - The sequence number returned from {@link transfer} or {@link swap}.
   * @returns The current transfer state, or null if not found.
   *
   * @example
   * ```typescript
   * const status = await client.getTransferStatus(42);
   * if (status?.status === TransferStatus.Complete) {
   *   console.log("Settled:", status.settlementSignature);
   * }
   * ```
   */
  async getTransferStatus(sequence: number): Promise<Transfer | null> {
    try {
      const data = await this.get(`/transfers/${sequence}`);
      return {
        sequence: data.sequence,
        txHash: data.tx_hash,
        blockNumber: data.block_number,
        status: data.status as TransferStatus,
        params: {
          recipient: data.recipient,
          destinationChain: data.destination_chain,
          amountWei: BigInt(data.amount_wei),
        },
        quote: data.quote,
        settlementSignature: data.settlement_signature ?? undefined,
        totalElapsedMs: data.total_elapsed_ms ?? undefined,
        error: data.error ?? undefined,
      };
    } catch (e) {
      if (e instanceof InterlinkApiError && e.statusCode === 404) {
        return null;
      }
      throw e;
    }
  }

  /**
   * Wait for a transfer to reach a terminal state (Complete or Failed).
   *
   * Polls {@link getTransferStatus} at the given interval until the transfer
   * settles or the timeout is reached.
   *
   * @param sequence - Transfer sequence number.
   * @param opts - Polling options.
   * @returns The completed/failed transfer.
   * @throws Error if the timeout is reached before settlement.
   *
   * @example
   * ```typescript
   * const result = await client.waitForSettlement(42, { timeoutMs: 120_000 });
   * console.log("Final status:", result.status);
   * ```
   */
  async waitForSettlement(
    sequence: number,
    opts: { pollIntervalMs?: number; timeoutMs?: number } = {}
  ): Promise<Transfer> {
    const pollInterval = opts.pollIntervalMs ?? 3_000;
    const timeout = opts.timeoutMs ?? 120_000;
    const deadline = Date.now() + timeout;

    while (Date.now() < deadline) {
      const transfer = await this.getTransferStatus(sequence);
      if (
        transfer &&
        (transfer.status === TransferStatus.Complete ||
          transfer.status === TransferStatus.Failed)
      ) {
        return transfer;
      }
      await new Promise((resolve) => setTimeout(resolve, pollInterval));
    }

    throw new Error(
      `Transfer ${sequence} did not settle within ${timeout}ms`
    );
  }

  // ─── Fee comparison ────────────────────────────────────────────────────────

  /**
   * Compare InterLink fees against Wormhole, Stargate, and Across for a given transfer size.
   *
   * InterLink wins on fee for ALL transfer sizes except very large institutional
   * transfers where Wormhole's flat $1 VAA fee beats our 0.01%.
   *
   * @param usdCents - Transfer value in USD cents (e.g., 100_000 = $1,000)
   */
  async compareFeesAt(usdCents: number): Promise<FeeComparison> {
    const data = await this.get(`/compare?usd_cents=${usdCents}`);
    const il = data.data.interlink;
    return {
      interlink: {
        feeBps: il.fee_bps,
        feeUsdCents: il.fee_usd_cents,
        settlementTargetSecs: il.settlement_target_secs,
        feeDescription: il.fee_description,
      },
      competitors: data.data.competitors.map((c: Record<string, unknown>) => ({
        name: c.name,
        feeBps: c.fee_bps,
        feeUsdCents: c.fee_usd_cents,
        settlementMinSecs: c.settlement_min_secs,
        settlementMaxSecs: c.settlement_max_secs,
      })),
      interlinkWinsFee: data.data.interlink_wins_fee,
      interlinkWinsSpeed: data.data.interlink_wins_speed,
      savingsVsCheapestCents: data.comparison?.savings_vs_cheapest_cents ?? 0,
    };
  }

  // ─── Metrics ───────────────────────────────────────────────────────────────

  /**
   * Fetch current relayer metrics (proof generation times, settlement latency, etc.)
   */
  async getMetrics(): Promise<Record<string, unknown>> {
    return this.get("/metrics/json");
  }

  // ─── Webhooks ──────────────────────────────────────────────────────────────

  /**
   * Register a webhook to receive real-time transfer event notifications.
   *
   * Unlike Wormhole (polling only) or Across (subgraph queries),
   * InterLink pushes events via webhook — like Stripe or GitHub.
   *
   * @param url - HTTPS callback URL. Must start with https:// in production.
   * @param events - Array of event types to receive. Use ["all"] for everything.
   *
   * @example
   * ```typescript
   * const webhook = await client.registerWebhook(
   *   "https://api.myapp.com/interlink-events",
   *   ["settlement.complete", "transfer.failed"]
   * );
   * console.log("Webhook ID:", webhook.id);
   * ```
   */
  async registerWebhook(
    url: string,
    events: WebhookEventType[] = ["all"]
  ): Promise<WebhookRegistration> {
    const data = await this.post("/webhooks/register", { url, events });
    return {
      id: data.id,
      url: data.url,
      events: data.events,
      active: data.active,
      registeredAt: data.registered_at,
      totalDelivered: data.total_delivered,
      totalFailed: data.total_failed,
    };
  }

  /**
   * List all registered webhooks.
   */
  async listWebhooks(): Promise<{
    count: number;
    activeCount: number;
    webhooks: WebhookRegistration[];
  }> {
    const data = await this.get("/webhooks");
    return {
      count: data.count,
      activeCount: data.active_count,
      webhooks: data.webhooks,
    };
  }

  /**
   * Remove a webhook registration.
   */
  async removeWebhook(id: string): Promise<boolean> {
    try {
      const data = await this.delete(`/webhooks/${id}`);
      return data.deleted === true;
    } catch {
      return false;
    }
  }

  /**
   * Get a webhook registration by ID.
   */
  async getWebhook(id: string): Promise<WebhookRegistration | null> {
    try {
      const data = await this.get(`/webhooks/${id}`);
      return data as WebhookRegistration;
    } catch {
      return null;
    }
  }

  // ─── HTTP helpers ──────────────────────────────────────────────────────────

  private async get(path: string): Promise<any> {
    const url = `${this.relayerUrl}${path}`;
    this.log(`GET ${url}`);

    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), this.timeoutMs);

    try {
      const resp = await fetch(url, { signal: controller.signal });
      if (!resp.ok) {
        throw new InterlinkApiError(resp.status, await resp.text(), path);
      }
      return resp.json();
    } finally {
      clearTimeout(timeout);
    }
  }

  private async post(path: string, body: unknown): Promise<any> {
    const url = `${this.relayerUrl}${path}`;
    this.log(`POST ${url}`);

    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), this.timeoutMs);

    try {
      const resp = await fetch(url, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
        signal: controller.signal,
      });
      if (!resp.ok) {
        throw new InterlinkApiError(resp.status, await resp.text(), path);
      }
      return resp.json();
    } finally {
      clearTimeout(timeout);
    }
  }

  private async delete(path: string): Promise<any> {
    const url = `${this.relayerUrl}${path}`;
    this.log(`DELETE ${url}`);

    const resp = await fetch(url, { method: "DELETE" });
    if (!resp.ok) {
      throw new InterlinkApiError(resp.status, await resp.text(), path);
    }
    return resp.json();
  }

  private log(msg: string) {
    if (this.debug) {
      console.debug(`[@interlink/sdk] ${msg}`);
    }
  }
}

// ─── Errors ───────────────────────────────────────────────────────────────────

/** Error thrown by the SDK when the relayer API returns a non-2xx response. */
export class InterlinkApiError extends Error {
  constructor(
    public readonly statusCode: number,
    public readonly responseBody: string,
    public readonly path: string
  ) {
    super(
      `InterLink API error ${statusCode} on ${path}: ${responseBody.slice(0, 200)}`
    );
    this.name = "InterlinkApiError";
  }
}

/** Error thrown when a quote expires before the transfer is submitted. */
export class QuoteExpiredError extends Error {
  constructor(public readonly expiredAt: number) {
    super(`InterLink quote expired at ${new Date(expiredAt * 1000).toISOString()}`);
    this.name = "QuoteExpiredError";
  }
}
