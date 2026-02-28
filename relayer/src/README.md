# relayer/src

Source code for the `relayer` binary.

## Key file

- `main.rs`
  - Creates a `RelayerConfig`, constructs a `Relayer`, and runs it.

## Local/dev expectations

The sample config references:

- A local EVM node websocket at `ws://localhost:8545`
- Solana devnet RPC

In a real deployment you would replace those values with your target environments and secrets.
