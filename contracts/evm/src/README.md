# contracts/evm/src

Solidity source code for the EVM gateway.

## Key file

- `InterlinkGateway.sol`
  - Main gateway contract.
  - Emits `MessagePublished` events for relayers.
  - Executes hub-authorized messages via `executeVerifiedMessage`.

## Important note

The `_verifyHalo2Proof` function currently represents the intended architecture (pairing check via the BN254 precompile) rather than a production-ready verifier implementation.
