/**
 * InterlinkClient — main SDK entry point.
 *
 * Provides methods to:
 *   - Get fee quotes before bridging
 *   - Initiate cross-chain transfers and swaps (gateway calls signed by the user)
 *   - Track transfer status by payload hash (read from the hub on-chain)
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

import { Signer, isHexString } from "ethers";
import {
  ZERO_ADDRESS,
  erc20Interface,
  gatewayInterface,
  parseMessagePublished,
  toBytes32Recipient,
  type PublishedMessage,
} from "./gateway";
import {
  ChainId,
  FeeComparison,
  InterlinkConfig,
  Transfer,
  TransferParams,
  TransferQuote,
  TransferStatus,
  TransferStatusInfo,
  SwapParams,
  WebhookEventType,
  WebhookRegistration,
  DEFAULT_RELAYER_URL,
  CHAIN_FINALITY_SECONDS,
} from "./types";

const MAX_U64 = (1n << 64n) - 1n;

/** Default timeout for API requests (ms). */
const DEFAULT_TIMEOUT_MS = 10_000;

/** @interlink/sdk version. Kept in sync with package.json. */
export const SDK_VERSION = "0.7.3";

// ─── InterlinkClient ──────────────────────────────────────────────────────────

export class InterlinkClient {
  private readonly relayerUrl: string;
  private readonly timeoutMs: number;
  private readonly debug: boolean;
  private readonly gatewayAddress?: string;
  private readonly sourceChain?: ChainId;
  private readonly apiToken?: string;

  constructor(config: InterlinkConfig = {}) {
    this.relayerUrl = (config.relayerUrl ?? DEFAULT_RELAYER_URL).replace(/\/$/, "");
    this.timeoutMs = config.timeoutMs ?? DEFAULT_TIMEOUT_MS;
    this.debug = config.debug ?? false;
    this.gatewayAddress = config.gatewayAddress;
    this.sourceChain = config.sourceChain;
    this.apiToken = config.apiToken;
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
   * Initiate a cross-chain transfer by calling `InterlinkGateway.sendCrossChainMessage`
   * with the user's own signer. The relayer picks the event up from the chain;
   * nothing is sent to the relayer and it never holds user funds.
   *
   * For ERC-20 tokens the gateway's allowance is checked and, if needed, an
   * `approve` transaction is sent first.
   *
   * @param params - Transfer parameters (recipient, destination chain, amount).
   * @param quote - A valid quote obtained from {@link getQuote}. Must not be expired.
   * @param signer - ethers v6 signer on the source chain (pays gas and funds).
   * @returns The transfer, identified by its canonical payload hash.
   *
   * @example
   * ```typescript
   * const client = new InterlinkClient({ gatewayAddress: "0x…" });
   * const quote = await client.getQuote({ ... });
   * const transfer = await client.transfer({
   *   recipient: "8xk2…",                // Solana address (base58)
   *   destinationChain: ChainId.Solana,
   *   amountWei: 1_000_000_000_000_000_000n,
   * }, quote, signer);
   * await client.waitForSettlement(transfer.payloadHash);
   * ```
   */
  async transfer(params: TransferParams, quote: TransferQuote, signer: Signer): Promise<Transfer> {
    if (quote.expiresAt < Math.floor(Date.now() / 1000)) {
      throw new QuoteExpiredError(quote.expiresAt);
    }
    const token = params.tokenAddress ?? ZERO_ADDRESS;
    const data = gatewayInterface.encodeFunctionData("sendCrossChainMessage", [
      params.destinationChain,
      token,
      params.amountWei,
      toBytes32Recipient(params.recipient),
      params.payload ?? new Uint8Array(),
    ]);
    const published = await this.sendGatewayTx(signer, token, params.amountWei, data);
    return {
      sequence: Number(published.message.nonce),
      payloadHash: published.payloadHash,
      txHash: published.txHash,
      blockNumber: published.blockNumber,
      status: TransferStatus.Pending,
      params,
      quote,
    };
  }

  /**
   * Initiate a cross-chain swap (bridge, then swap on the hub's pool) by
   * calling `InterlinkGateway.initiateSwap` with the user's signer.
   *
   * @param params - Swap parameters (destination chain, output token, min amount).
   * @param amountWei - Amount of `params.tokenIn` (default native) to bridge.
   * @param signer - ethers v6 signer on the source chain.
   *
   * @example
   * ```typescript
   * const transfer = await client.swap({
   *   destinationChain: ChainId.Solana,
   *   recipient: "8xk2…",
   *   tokenOut: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", // USDC mint
   *   minAmountOut: 3000_000_000n,
   * }, 1_000_000_000_000_000_000n, signer);
   * ```
   */
  async swap(params: SwapParams, amountWei: bigint, signer: Signer): Promise<Transfer> {
    if (params.minAmountOut > MAX_U64) {
      throw new Error("minAmountOut must fit in a u64 (Solana token amounts)");
    }
    const token = params.tokenIn ?? ZERO_ADDRESS;
    const data = gatewayInterface.encodeFunctionData("initiateSwap", [
      params.destinationChain,
      token,
      amountWei,
      toBytes32Recipient(params.tokenOut),
      params.minAmountOut,
      toBytes32Recipient(params.recipient),
      params.swapData ?? new Uint8Array(),
    ]);
    const published = await this.sendGatewayTx(signer, token, amountWei, data);
    return {
      sequence: Number(published.message.nonce),
      payloadHash: published.payloadHash,
      txHash: published.txHash,
      blockNumber: published.blockNumber,
      status: TransferStatus.Pending,
      params: {
        recipient: params.recipient,
        destinationChain: params.destinationChain,
        tokenAddress: token,
        amountWei,
      },
    };
  }

  /**
   * Hub status of a message, read from its on-chain record by the relayer.
   *
   * @param payloadHash - Canonical payload hash returned by {@link transfer} / {@link swap}.
   */
  async getTransferStatus(payloadHash: string): Promise<TransferStatusInfo> {
    if (!isHexString(payloadHash, 32)) {
      throw new Error(`invalid payload hash: ${payloadHash}`);
    }
    const data = await this.get(`/transfers/${payloadHash.toLowerCase()}`);
    const status =
      data.status === "executed"
        ? TransferStatus.Complete
        : data.status === "attested"
          ? TransferStatus.PendingSettlement
          : TransferStatus.Pending;
    return {
      payloadHash,
      status,
      attestedBy: data.attested_by ?? undefined,
      attestedAt: data.attested_at ?? undefined,
    };
  }

  /**
   * Poll {@link getTransferStatus} until the message is executed on the hub.
   * Source-chain finality alone is ~13 minutes on Ethereum, so the default
   * timeout is 30 minutes.
   */
  async waitForSettlement(
    payloadHash: string,
    opts: { pollIntervalMs?: number; timeoutMs?: number } = {}
  ): Promise<TransferStatusInfo> {
    const pollInterval = opts.pollIntervalMs ?? 15_000;
    const timeout = opts.timeoutMs ?? 30 * 60_000;
    const deadline = Date.now() + timeout;

    while (Date.now() < deadline) {
      const info = await this.getTransferStatus(payloadHash);
      if (info.status === TransferStatus.Complete) {
        return info;
      }
      await new Promise((resolve) => setTimeout(resolve, pollInterval));
    }
    throw new Error(`Transfer ${payloadHash} did not settle within ${timeout}ms`);
  }

  /** Approve if needed, send the gateway call and decode its MessagePublished event. */
  private async sendGatewayTx(
    signer: Signer,
    token: string,
    amount: bigint,
    data: string
  ): Promise<PublishedMessage & { txHash: string; blockNumber: number }> {
    if (!this.gatewayAddress) {
      throw new Error("gatewayAddress must be set in InterlinkConfig to send transfers");
    }
    const gateway = this.gatewayAddress;
    const native = token === ZERO_ADDRESS;

    if (!native) {
      const owner = await signer.getAddress();
      const raw = await signer.call({
        to: token,
        data: erc20Interface.encodeFunctionData("allowance", [owner, gateway]),
      });
      const [allowance] = erc20Interface.decodeFunctionResult("allowance", raw);
      if (allowance < amount) {
        this.log(`approving ${amount} of ${token} for ${gateway}`);
        const approval = await signer.sendTransaction({
          to: token,
          data: erc20Interface.encodeFunctionData("approve", [gateway, amount]),
        });
        await approval.wait();
      }
    }

    const tx = await signer.sendTransaction({ to: gateway, data, value: native ? amount : 0n });
    const receipt = await tx.wait();
    if (!receipt || receipt.status !== 1) {
      throw new Error(`gateway transaction ${tx.hash} failed`);
    }
    const srcChain = BigInt(this.sourceChain ?? (await signer.provider!.getNetwork()).chainId);
    const published = parseMessagePublished(receipt.logs, gateway, srcChain);
    return { ...published, txHash: receipt.hash, blockNumber: receipt.blockNumber };
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
      const resp = await fetch(url, { headers: this.authHeaders(path), signal: controller.signal });
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
        headers: { "Content-Type": "application/json", ...this.authHeaders(path) },
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

    const resp = await fetch(url, { method: "DELETE", headers: this.authHeaders(path) });
    if (!resp.ok) {
      throw new InterlinkApiError(resp.status, await resp.text(), path);
    }
    return resp.json();
  }

  private authHeaders(path: string): Record<string, string> {
    return this.apiToken && path.startsWith("/webhooks")
      ? { Authorization: `Bearer ${this.apiToken}` }
      : {};
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
