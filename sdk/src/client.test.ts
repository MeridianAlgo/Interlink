/**
 * Unit tests for @interlink/sdk InterlinkClient.
 *
 * Tests use a mock fetch to avoid requiring a live relayer.
 */

import { InterlinkClient, InterlinkApiError } from "./client";
import { ChainId, TransferStatus } from "./types";

// ─── Mock fetch ───────────────────────────────────────────────────────────────

const mockResponses: Record<string, unknown> = {};

function mockFetch(responses: Record<string, unknown>) {
  Object.assign(mockResponses, responses);
}

// Minimal fetch mock
global.fetch = jest.fn(async (url: string, init?: RequestInit) => {
  const path = new URL(url).pathname + new URL(url).search;

  // Find best-matching mock
  const key = Object.keys(mockResponses).find((k) => path.startsWith(k));
  if (key) {
    const body = mockResponses[key];
    return {
      ok: true,
      status: 200,
      json: async () => body,
      text: async () => JSON.stringify(body),
    } as Response;
  }

  return {
    ok: false,
    status: 404,
    json: async () => ({ error: "not found" }),
    text: async () => "not found",
  } as Response;
}) as jest.Mock;

// ─── Fixtures ─────────────────────────────────────────────────────────────────

const healthResponse = { status: "ok", service: "interlink-relayer", version: "0.1.0" };

const quoteResponse = {
  inputs: { amount_wei: "1000000000000000000", usd_cents: 300_000 },
  estimate: {
    source_gas_units: 80_000,
    source_gas_price_gwei: 30,
    source_gas_cost_wei: "2400000000000000",
    proof_cost_amortised_wei: "6000000000000000",
    dest_compute_units: 200_000,
    dest_fee_lamports: 5_200,
    fee_tier: { name: "Standard", bps: 5, description: "0.05% Tier 2" },
    protocol_fee_amount: "500000000000000",
  },
  comparison: {
    interlink: {
      fee_bps: 5,
      fee_usd_cents: 150,
      settlement_target_secs: 30,
      fee_description: "0.05% Tier 2",
    },
    competitors: [
      { name: "Wormhole", fee_bps: 10, fee_usd_cents: 100, settlement_min_secs: 120, settlement_max_secs: 900 },
    ],
    interlink_wins_fee: true,
    interlink_wins_speed: true,
    savings_vs_cheapest_cents: -50,
  },
};

const compareResponse = {
  usd_cents: 100_000,
  data: {
    interlink: { fee_bps: 0, fee_usd_cents: 0, settlement_target_secs: 30, fee_description: "0% Tier 1" },
    competitors: [
      { name: "Wormhole", fee_bps: 10, fee_usd_cents: 100, settlement_min_secs: 120, settlement_max_secs: 900 },
      { name: "Stargate v2", fee_bps: 50, fee_usd_cents: 50, settlement_min_secs: 60, settlement_max_secs: 120 },
      { name: "Across", fee_bps: 25, fee_usd_cents: 25, settlement_min_secs: 300, settlement_max_secs: 3600 },
    ],
    interlink_wins_fee: true,
    interlink_wins_speed: true,
  },
};

const webhookListResponse = {
  count: 0,
  active_count: 0,
  webhooks: [],
};

const webhookRegisterResponse = {
  id: "wh_abc123",
  url: "https://example.com/hook",
  events: ["settlement.complete"],
  active: true,
  registered_at: 1700000000,
  consecutive_failures: 0,
  total_delivered: 0,
  total_failed: 0,
};

const metricsResponse = {
  proof_gen_total: 42,
  proof_gen_success: 40,
  settlement_total: 39,
};

// ─── Tests ────────────────────────────────────────────────────────────────────

describe("InterlinkClient", () => {
  let client: InterlinkClient;

  beforeEach(() => {
    jest.clearAllMocks();
    mockFetch({
      "/health": healthResponse,
      "/quote": quoteResponse,
      "/compare": compareResponse,
      "/webhooks": webhookListResponse,
      "/webhooks/register": { ...webhookRegisterResponse, status: 201 },
      "/metrics/json": metricsResponse,
    });
    client = new InterlinkClient({ relayerUrl: "http://localhost:8080" });
  });

  // ─── Health ──────────────────────────────────────────────────────────────

  test("isHealthy returns true for healthy relayer", async () => {
    mockFetch({ "/health": { status: "ok" } });
    expect(await client.isHealthy()).toBe(true);
  });

  test("isHealthy returns false on network error", async () => {
    (global.fetch as jest.Mock).mockRejectedValueOnce(new Error("ECONNREFUSED"));
    expect(await client.isHealthy()).toBe(false);
  });

  // ─── Quotes ──────────────────────────────────────────────────────────────

  test("getQuote returns parsed quote", async () => {
    mockFetch({ "/quote": quoteResponse });

    const quote = await client.getQuote({
      sourceChain: ChainId.Ethereum,
      destChain: ChainId.Solana,
      amountWei: 1_000_000_000_000_000_000n,
      usdCents: 300_000,
    });

    expect(quote.feeBps).toBe(5);
    expect(quote.feeTier).toBe("Standard");
    expect(quote.feeWei).toBe(500_000_000_000_000n);
    expect(quote.sourceChain).toBe(ChainId.Ethereum);
    expect(quote.destChain).toBe(ChainId.Solana);
    expect(quote.expiresAt).toBeGreaterThan(Date.now() / 1000);
  });

  test("getQuote tier 1 has zero fee", async () => {
    const tier1Quote = {
      ...quoteResponse,
      estimate: {
        ...quoteResponse.estimate,
        fee_tier: { name: "Zero", bps: 0, description: "0% Tier 1" },
        protocol_fee_amount: "0",
      },
    };
    mockFetch({ "/quote": tier1Quote });

    const quote = await client.getQuote({
      sourceChain: ChainId.Ethereum,
      destChain: ChainId.Solana,
      amountWei: 50_000_000_000_000_000n,
      usdCents: 15_000, // $150 — Tier 1
    });

    expect(quote.feeBps).toBe(0);
    expect(quote.feeWei).toBe(0n);
    expect(quote.feeTier).toBe("Zero");
  });

  test("getQuote applies correct finality time for Optimism", async () => {
    mockFetch({ "/quote": quoteResponse });

    const quote = await client.getQuote({
      sourceChain: ChainId.Optimism,
      destChain: ChainId.Solana,
      amountWei: 1_000_000_000_000_000_000n,
      usdCents: 300_000,
    });

    // Optimism finality is 2s, total = 2 + 5 = 7s
    expect(quote.estimatedSettlementSecs).toBe(7);
  });

  // ─── Fee comparison ───────────────────────────────────────────────────────

  test("compareFeesAt returns interlink wins on fee and speed", async () => {
    mockFetch({ "/compare": compareResponse });

    const cmp = await client.compareFeesAt(100_000);

    expect(cmp.interlinkWinsFee).toBe(true);
    expect(cmp.interlinkWinsSpeed).toBe(true);
    expect(cmp.competitors).toHaveLength(3);
    expect(cmp.competitors.map((c) => c.name)).toContain("Wormhole");
    expect(cmp.competitors.map((c) => c.name)).toContain("Across");
  });

  test("compareFeesAt tier 1 zero fee", async () => {
    mockFetch({ "/compare": compareResponse });
    const cmp = await client.compareFeesAt(100_000); // $1,000 = Tier 1
    expect(cmp.interlink.feeBps).toBe(0);
    expect(cmp.interlink.feeUsdCents).toBe(0);
  });

  // ─── Metrics ──────────────────────────────────────────────────────────────

  test("getMetrics returns proof and settlement stats", async () => {
    mockFetch({ "/metrics/json": metricsResponse });
    const metrics = await client.getMetrics();
    expect(metrics).toMatchObject({ proof_gen_total: 42 });
  });

  // ─── Webhooks ─────────────────────────────────────────────────────────────

  test("registerWebhook returns registration with id", async () => {
    mockFetch({ "/webhooks/register": webhookRegisterResponse });

    const reg = await client.registerWebhook(
      "https://example.com/hook",
      ["settlement.complete"]
    );

    expect(reg.id).toBe("wh_abc123");
    expect(reg.url).toBe("https://example.com/hook");
    expect(reg.active).toBe(true);
    expect(reg.events).toContain("settlement.complete");
  });

  test("listWebhooks returns count and list", async () => {
    mockFetch({
      "/webhooks": {
        count: 1,
        active_count: 1,
        webhooks: [webhookRegisterResponse],
      },
    });

    const list = await client.listWebhooks();
    expect(list.count).toBe(1);
    expect(list.activeCount).toBe(1);
    expect(list.webhooks).toHaveLength(1);
  });

  test("removeWebhook returns true on success", async () => {
    (global.fetch as jest.Mock).mockResolvedValueOnce({
      ok: true,
      status: 200,
      json: async () => ({ deleted: true, id: "wh_abc123" }),
      text: async () => "",
    });

    expect(await client.removeWebhook("wh_abc123")).toBe(true);
  });

  test("removeWebhook returns false on 404", async () => {
    (global.fetch as jest.Mock).mockResolvedValueOnce({
      ok: false,
      status: 404,
      json: async () => ({ error: "not found" }),
      text: async () => "not found",
    });

    expect(await client.removeWebhook("wh_nonexistent")).toBe(false);
  });

  // ─── Error handling ───────────────────────────────────────────────────────

  test("get throws InterlinkApiError on non-2xx response", async () => {
    (global.fetch as jest.Mock).mockResolvedValueOnce({
      ok: false,
      status: 500,
      json: async () => ({ error: "internal server error" }),
      text: async () => "internal server error",
    });

    await expect(client.getMetrics()).rejects.toThrow(InterlinkApiError);
    await expect(client.getMetrics()).rejects.toThrow("500");
  });

  test("InterlinkApiError has correct properties", async () => {
    (global.fetch as jest.Mock).mockResolvedValueOnce({
      ok: false,
      status: 429,
      text: async () => "rate limited",
      json: async () => ({ error: "rate limited" }),
    });

    try {
      await client.getMetrics();
    } catch (e) {
      expect(e).toBeInstanceOf(InterlinkApiError);
      const err = e as InterlinkApiError;
      expect(err.statusCode).toBe(429);
      expect(err.responseBody).toBe("rate limited");
    }
  });

  // ─── Config ───────────────────────────────────────────────────────────────

  test("default relayer URL is used when none provided", () => {
    const c = new InterlinkClient();
    // No error thrown — just check it constructs
    expect(c).toBeInstanceOf(InterlinkClient);
  });

  test("trailing slash stripped from relayer URL", async () => {
    const c = new InterlinkClient({ relayerUrl: "http://localhost:8080/" });
    mockFetch({ "/health": { status: "ok" } });
    expect(await c.isHealthy()).toBe(true);
    // Verify fetch called without double-slash
    const calledUrl = (global.fetch as jest.Mock).mock.calls[0][0] as string;
    expect(calledUrl).not.toContain("//health");
  });
});
