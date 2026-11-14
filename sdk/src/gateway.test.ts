import { encodeBase58, getBytes, zeroPadValue } from "ethers";
import { MessageAction, hashMessage, toBytes32Recipient } from "./gateway";

describe("gateway helpers", () => {
  /**
   * Shared vector: also pinned in contracts/evm/test (VECTOR_HASH), the hub's
   * Rust tests and the relayer, and checked against the compiled gateway.
   */
  test("hashMessage matches the cross-language vector", () => {
    const h = hashMessage({
      srcChain: 2n,
      srcGateway: "0x" + "7b".repeat(32),
      dstChain: 31337n,
      dstGateway: zeroPadValue("0x000000000000000000000000000000000000beef", 32),
      nonce: 7n,
      action: MessageAction.Transfer,
      sender: "0x" + "22".repeat(32),
      asset: "0x" + "00".repeat(32),
      amount: 2_000_000_000_000_000_000n,
      recipient: zeroPadValue("0x" + "a1".repeat(20), 32),
      extraHash: "0x" + "00".repeat(32),
    });
    expect(h).toBe("0x07b0f8b8ce5c6f82e0bb552073f1c35df9b0987c6e2c598c52ed5b59ee1c09bd");
  });

  test("toBytes32Recipient handles EVM, hex32 and Solana addresses", () => {
    expect(toBytes32Recipient("0x000000000000000000000000000000000000bEEF")).toBe(
      "0x000000000000000000000000000000000000000000000000000000000000beef"
    );
    const hex32 = "0x" + "ab".repeat(32);
    expect(toBytes32Recipient(hex32)).toBe(hex32);

    const sol = "AKzpc9tvxfhLjj5AantKizK2YZgSjoyhLmYqRZy6b8Lz";
    const encoded = toBytes32Recipient(sol);
    expect(getBytes(encoded)).toHaveLength(32);
    expect(encodeBase58(encoded)).toBe(sol);
  });

  test("toBytes32Recipient rejects garbage", () => {
    expect(() => toBytes32Recipient("not an address!")).toThrow("invalid recipient");
    expect(() => toBytes32Recipient("")).toThrow("invalid recipient");
  });
});
