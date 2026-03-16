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
  TransferParams,
  TransferQuote,
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
    return {
      interlink: data.data.interlink,
      competitors: data.data.competitors,
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
