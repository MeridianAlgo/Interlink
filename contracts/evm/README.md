# contracts/evm

EVM (Solidity) contracts for InterLink.

## What this is

This is the **source-chain gateway (spoke)** side for EVM-compatible chains. It:

- Custodies user funds (ERC-20 or native)
- Emits canonical events that relayers can watch
- Accepts “verified message” executions after the hub has validated a proof

## Layout

- `src/`
  - Solidity contract sources.
