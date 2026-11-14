/**
 * Unit tests for @interlink/sdk InterlinkClient.
 *
 * Tests use a mock fetch to avoid requiring a live relayer.
 */

import { Signer, getBytes, keccak256, zeroPadValue } from "ethers";
import { InterlinkClient, InterlinkApiError, QuoteExpiredError } from "./client";
import {
  MessageAction,
  erc20Interface,
  gatewayInterface,
  hashMessage,
  swapExtraHash,
  toBytes32Recipient,
} from "./gateway";
import { ChainId, TransferQuote, TransferStatus } from "./types";

// ─── Mock fetch ───────────────────────────────────────────────────────────────

const mockResponses: Record<string, unknown> = {};

function mockFetch(responses: Record<string, unknown>) {
  Object.assign(mockResponses, responses);
}

// Minimal fetch mock
global.fetch = jest.fn(async (url: string, init?: RequestInit) => {
  const path = new URL(url).pathname + new URL(url).search;

  // Find best-matching mock (longest prefix wins to avoid /webhooks matching /webhooks/register)
  const key = Object.keys(mockResponses)
    .filter((k) => path.startsWith(k))
    .sort((a, b) => b.length - a.length)[0];
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
    const cmp = await client.compareFeesAt(100_000);

    expect(cmp.interlinkWinsFee).toBe(true);
    expect(cmp.interlinkWinsSpeed).toBe(true);
    expect(cmp.competitors).toHaveLength(3);
    expect(cmp.competitors.map((c: { name: string }) => c.name)).toContain("Wormhole");
    expect(cmp.competitors.map((c: { name: string }) => c.name)).toContain("Across");
  });

  test("compareFeesAt tier 1 zero fee", async () => {
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
    (global.fetch as jest.Mock)
      .mockResolvedValueOnce({
        ok: false,
        status: 500,
        json: async () => ({ error: "internal server error" }),
        text: async () => "internal server error",
      })
      .mockResolvedValueOnce({
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

  // ─── Transfers (gateway calls signed by the user) ─────────────────────────

  const GATEWAY = "0x000000000000000000000000000000000000bEEF";
  const USER = "0x00000000000000000000000000000000000b0b0b";
  const HUB_GATEWAY = "0x" + "7b".repeat(32);
  const SOL_RECIPIENT = "8xk2fA3NVuwBnbYQSgqxk5jhUuA9DUFRh3JZbXMwG6ra";

  /**
   * Mock ethers signer. `sendTransaction` decodes the gateway call and emits
   * the MessagePublished log the real gateway would, so the SDK's calldata,
   * receipt parsing and payload-hash check are exercised end to end.
   */
  function mockSigner(opts: { allowance?: bigint } = {}) {
    const sent: { to: string; data: string; value?: bigint }[] = [];
    const signer = {
      sent,
      provider: { getNetwork: async () => ({ chainId: 1n }) },
      getAddress: async () => USER,
      call: async () => erc20Interface.encodeFunctionResult("allowance", [opts.allowance ?? 0n]),
      sendTransaction: async (tx: { to: string; data: string; value?: bigint }) => {
        sent.push(tx);
        const logs: { address: string; topics: string[]; data: string }[] = [];
        if (tx.to === GATEWAY) {
          const call = gatewayInterface.parseTransaction({ data: tx.data, value: tx.value })!;
          const a = call.args;
          const isSwap = call.name === "initiateSwap";
          const message = {
            srcChain: 1n,
            srcGateway: zeroPadValue(GATEWAY, 32),
            dstChain: a[0] as bigint,
            dstGateway: HUB_GATEWAY,
            nonce: 7n,
            action: isSwap ? MessageAction.Swap : MessageAction.Transfer,
            sender: zeroPadValue(USER, 32),
            asset: zeroPadValue(a[1], 32),
            amount: a[2] as bigint,
            recipient: isSwap ? a[5] : a[3],
            extraHash: isSwap ? swapExtraHash(a[3], a[4], getBytes(a[6])) : keccak256(a[4]),
          };
          const ev = gatewayInterface.encodeEventLog("MessagePublished", [
            hashMessage(message), message.nonce, message.dstChain, message.dstGateway,
            message.action, message.sender, message.asset, message.amount,
            message.recipient, message.extraHash, "0x",
          ]);
          logs.push({ address: GATEWAY, topics: ev.topics, data: ev.data });
        }
        return {
          hash: "0x" + "ab".repeat(32),
          wait: async () => ({ status: 1, hash: "0x" + "ab".repeat(32), blockNumber: 12345, logs }),
        };
      },
    };
    return signer as unknown as Signer & { sent: typeof sent };
  }

  const freshQuote = () =>
    ({
      sourceChain: ChainId.Ethereum,
      destChain: ChainId.Solana,
      amountWei: 10n ** 18n,
      usdCents: 300_000,
      feeWei: 0n,
      feeTier: "Zero",
      feeBps: 0,
      estimatedSettlementSecs: 900,
      sourceGasWei: 0n,
      destFeeLamports: 0,
      expiresAt: Math.floor(Date.now() / 1000) + 60,
    }) as TransferQuote;

  test("transfer sends a native gateway call and returns its payload hash", async () => {
    const c = new InterlinkClient({ relayerUrl: "http://localhost:8080", gatewayAddress: GATEWAY });
    const signer = mockSigner();
    const t = await c.transfer(
      { recipient: SOL_RECIPIENT, destinationChain: ChainId.Solana, amountWei: 10n ** 18n },
      freshQuote(),
      signer
    );
    expect(signer.sent).toHaveLength(1);
    expect(signer.sent[0].value).toBe(10n ** 18n);
    const call = gatewayInterface.parseTransaction({ data: signer.sent[0].data })!;
    expect(call.name).toBe("sendCrossChainMessage");
    expect(call.args[0]).toBe(BigInt(ChainId.Solana));
    expect(call.args[3]).toBe(toBytes32Recipient(SOL_RECIPIENT));
    expect(t.sequence).toBe(7);
    expect(t.payloadHash).toMatch(/^0x[0-9a-f]{64}$/);
    expect(t.status).toBe(TransferStatus.Pending);
    expect(t.blockNumber).toBe(12345);
  });

  test("transfer approves ERC-20 allowance first when insufficient", async () => {
    const c = new InterlinkClient({ gatewayAddress: GATEWAY, sourceChain: ChainId.Ethereum });
    const token = "0x00000000000000000000000000000000000a11ce";
    const signer = mockSigner({ allowance: 0n });
    await c.transfer(
      { recipient: SOL_RECIPIENT, destinationChain: ChainId.Solana, amountWei: 500n, tokenAddress: token },
      freshQuote(),
      signer
    );
    expect(signer.sent).toHaveLength(2);
    expect(erc20Interface.parseTransaction({ data: signer.sent[0].data })!.name).toBe("approve");
    expect(signer.sent[1].value).toBe(0n);
  });

  test("transfer skips approval when allowance suffices", async () => {
    const c = new InterlinkClient({ gatewayAddress: GATEWAY });
    const signer = mockSigner({ allowance: 10_000n });
    await c.transfer(
      {
        recipient: SOL_RECIPIENT,
        destinationChain: ChainId.Solana,
        amountWei: 500n,
        tokenAddress: "0x00000000000000000000000000000000000a11ce",
      },
      freshQuote(),
      signer
    );
    expect(signer.sent).toHaveLength(1);
  });

  test("transfer throws QuoteExpiredError for expired quote", async () => {
    const c = new InterlinkClient({ gatewayAddress: GATEWAY });
    const expired = { ...freshQuote(), expiresAt: Math.floor(Date.now() / 1000) - 60 };
    await expect(
      c.transfer({ recipient: SOL_RECIPIENT, destinationChain: ChainId.Solana, amountWei: 1n }, expired, mockSigner())
    ).rejects.toBeInstanceOf(QuoteExpiredError);
  });

  test("transfer requires a gateway address", async () => {
    const c = new InterlinkClient();
    await expect(
      c.transfer({ recipient: SOL_RECIPIENT, destinationChain: ChainId.Solana, amountWei: 1n }, freshQuote(), mockSigner())
    ).rejects.toThrow("gatewayAddress");
  });

  test("swap sends initiateSwap with bytes32 tokenOut and recipient", async () => {
    const c = new InterlinkClient({ gatewayAddress: GATEWAY });
    const signer = mockSigner();
    const usdcMint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
    const t = await c.swap(
      { destinationChain: ChainId.Solana, recipient: SOL_RECIPIENT, tokenOut: usdcMint, minAmountOut: 3_000_000_000n },
      10n ** 18n,
      signer
    );
    const call = gatewayInterface.parseTransaction({ data: signer.sent[0].data, value: signer.sent[0].value })!;
    expect(call.name).toBe("initiateSwap");
    expect(call.args[3]).toBe(toBytes32Recipient(usdcMint));
    expect(call.args[4]).toBe(3_000_000_000n);
    expect(t.quote).toBeUndefined();
    expect(t.payloadHash).toMatch(/^0x[0-9a-f]{64}$/);
  });

  test("swap rejects minAmountOut above u64", async () => {
    const c = new InterlinkClient({ gatewayAddress: GATEWAY });
    await expect(
      c.swap(
        { destinationChain: ChainId.Solana, recipient: SOL_RECIPIENT, tokenOut: SOL_RECIPIENT, minAmountOut: 1n << 64n },
        1n,
        mockSigner()
      )
    ).rejects.toThrow("u64");
  });

  // ─── Status (hub on-chain record via relayer) ─────────────────────────────

  const HASH = "0x" + "07".repeat(32);

  test("getTransferStatus maps hub record states", async () => {
    mockFetch({ [`/transfers/${HASH}`]: { status: "executed", attested_by: "Relayer111", attested_at: 1700000000 } });
    const info = await client.getTransferStatus(HASH);
    expect(info.status).toBe(TransferStatus.Complete);
    expect(info.attestedBy).toBe("Relayer111");
  });

  test("getTransferStatus reports pending when not yet attested", async () => {
    const other = "0x" + "08".repeat(32);
    mockFetch({ [`/transfers/${other}`]: { status: "not_found" } });
    expect((await client.getTransferStatus(other)).status).toBe(TransferStatus.Pending);
  });

  test("getTransferStatus rejects malformed hashes", async () => {
    await expect(client.getTransferStatus("42")).rejects.toThrow("invalid payload hash");
  });

  test("waitForSettlement resolves once executed", async () => {
    const h = "0x" + "09".repeat(32);
    mockFetch({ [`/transfers/${h}`]: { status: "executed" } });
    const info = await client.waitForSettlement(h, { pollIntervalMs: 1, timeoutMs: 1000 });
    expect(info.status).toBe(TransferStatus.Complete);
  });

  test("webhook calls send the API token as a bearer header", async () => {
    const c = new InterlinkClient({ relayerUrl: "http://localhost:8080", apiToken: "s3cret" });
    mockFetch({ "/webhooks": { count: 0, active_count: 0, webhooks: [] } });
    await c.listWebhooks();
    const init = (global.fetch as jest.Mock).mock.calls.at(-1)[1] as RequestInit;
    expect((init.headers as Record<string, string>).Authorization).toBe("Bearer s3cret");
  });

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
