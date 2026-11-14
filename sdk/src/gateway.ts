/**
 * InterlinkGateway helpers: ABI, calldata builders, recipient encoding and the
 * canonical payload hash (identical to `InterlinkGateway.hashMessage`, the hub's
 * `canonical_hash` and the relayer's `CanonicalMessage::hash`).
 */

import {
  AbiCoder,
  Interface,
  decodeBase58,
  getAddress,
  isAddress,
  isHexString,
  keccak256,
  toBeHex,
  toUtf8Bytes,
  zeroPadValue,
  type Log,
} from "ethers";

export const ZERO_ADDRESS = "0x0000000000000000000000000000000000000000";

/** Message actions (must match the contracts). */
export enum MessageAction {
  Transfer = 1,
  Swap = 2,
  Nft = 3,
  Call = 4,
}

export const GATEWAY_ABI = [
  "function sendCrossChainMessage(uint64 dstChain, address token, uint256 amount, bytes32 recipient, bytes payload) payable returns (bytes32)",
  "function initiateSwap(uint64 dstChain, address tokenIn, uint256 amountIn, bytes32 tokenOut, uint256 minAmountOut, bytes32 recipient, bytes swapData) payable returns (bytes32)",
  "function executedMessages(bytes32 payloadHash) view returns (bool)",
  "function remoteGateways(uint64 chainId) view returns (bytes32)",
  "event MessagePublished(bytes32 indexed payloadHash, uint64 indexed nonce, uint64 dstChain, bytes32 dstGateway, uint8 action, bytes32 sender, bytes32 asset, uint256 amount, bytes32 recipient, bytes32 extraHash, bytes data)",
];

export const ERC20_ABI = [
  "function allowance(address owner, address spender) view returns (uint256)",
  "function approve(address spender, uint256 amount) returns (bool)",
];

export const gatewayInterface = new Interface(GATEWAY_ABI);
export const erc20Interface = new Interface(ERC20_ABI);

/**
 * Encode a destination recipient as bytes32.
 * Accepts an EVM address (left-padded), a 0x-prefixed 32-byte hex value, or a
 * base58 Solana address (32 bytes).
 */
export function toBytes32Recipient(recipient: string): string {
  // Check a copy: `isAddress` is a type guard and would narrow `recipient` to never.
  const asAddress = recipient;
  if (isAddress(asAddress)) {
    return zeroPadValue(getAddress(asAddress), 32);
  }
  if (isHexString(recipient, 32)) {
    return recipient.toLowerCase();
  }
  let value: bigint;
  try {
    value = decodeBase58(recipient);
  } catch {
    throw new Error(`invalid recipient: ${recipient}`);
  }
  if (value === 0n || value >= 1n << 256n) {
    throw new Error(`invalid recipient: ${recipient}`);
  }
  return toBeHex(value, 32);
}

/** All fields of a canonical cross-chain message. */
export interface CanonicalMessage {
  srcChain: bigint;
  srcGateway: string; // bytes32
  dstChain: bigint;
  dstGateway: string; // bytes32
  nonce: bigint;
  action: MessageAction;
  sender: string; // bytes32
  asset: string; // bytes32
  amount: bigint;
  recipient: string; // bytes32
  extraHash: string; // bytes32
}

const MESSAGE_TYPEHASH = keccak256(toUtf8Bytes("InterlinkMessage(v1)"));

/** keccak256(abi.encode(typeHash, ...fields)) */
export function hashMessage(m: CanonicalMessage): string {
  return keccak256(
    AbiCoder.defaultAbiCoder().encode(
      [
        "bytes32", "uint64", "bytes32", "uint64", "bytes32", "uint64",
        "uint8", "bytes32", "bytes32", "uint256", "bytes32", "bytes32",
      ],
      [
        MESSAGE_TYPEHASH, m.srcChain, m.srcGateway, m.dstChain, m.dstGateway, m.nonce,
        m.action, m.sender, m.asset, m.amount, m.recipient, m.extraHash,
      ]
    )
  );
}

/** `extraHash` the gateway computes for a swap. */
export function swapExtraHash(tokenOut: string, minAmountOut: bigint, swapData: Uint8Array): string {
  return keccak256(
    AbiCoder.defaultAbiCoder().encode(
      ["bytes32", "uint256", "bytes32"],
      [tokenOut, minAmountOut, keccak256(swapData)]
    )
  );
}

/** A `MessagePublished` event decoded from a receipt. */
export interface PublishedMessage {
  payloadHash: string;
  message: CanonicalMessage;
  data: string;
}

/**
 * Find and decode the gateway's `MessagePublished` log, rebuilding the
 * canonical message and checking that it hashes to the emitted payload hash.
 */
export function parseMessagePublished(
  logs: readonly Log[],
  gatewayAddress: string,
  srcChain: bigint
): PublishedMessage {
  const gateway = getAddress(gatewayAddress);
  for (const log of logs) {
    if (getAddress(log.address) !== gateway) continue;
    const parsed = gatewayInterface.parseLog({ topics: [...log.topics], data: log.data });
    if (!parsed || parsed.name !== "MessagePublished") continue;
    const a = parsed.args;
    const message: CanonicalMessage = {
      srcChain,
      srcGateway: zeroPadValue(gateway, 32),
      dstChain: a.dstChain,
      dstGateway: a.dstGateway,
      nonce: a.nonce,
      action: Number(a.action) as MessageAction,
      sender: a.sender,
      asset: a.asset,
      amount: a.amount,
      recipient: a.recipient,
      extraHash: a.extraHash,
    };
    if (hashMessage(message) !== a.payloadHash) {
      throw new Error("MessagePublished fields do not match its payload hash");
    }
    return { payloadHash: a.payloadHash, message, data: a.data };
  }
  throw new Error("no MessagePublished event from the gateway in this receipt");
}
