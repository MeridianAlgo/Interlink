# Solana Operations Guide

This document outlines the essential Solana terminal commands for managing, deploying, and testing the InterLink Solana Hub program.

## Prerequisites

- **Solana CLI**: `solana --version` (Recommended `>=1.18`)
- **Anchor CLI**: `anchor --version` (Recommended `0.30.1` or `0.32.1`)
- **Node.js & Yarn**: For running TypeScript tests.

---

## 1. Wallet & Account Setup

### Generate a new Relayer/Deployer Keypair
```bash
solana-keygen new --outfile ~/.config/solana/id.json
```

### Check Balance
```bash
solana balance
```

### Airdrop SOL (Devnet)
```bash
solana airdrop 2 --url devnet
```

### Set Config to Devnet
```bash
solana config set --url devnet
```

---

## 2. Program Development

### Build the Program
Navigate to the Solana Hub directory and build:
```bash
cd contracts/solana/interlink-hub
anchor build
```

### Deploy to Devnet
Ensure your `Anchor.toml` is configured for devnet.
```bash
anchor deploy --provider.cluster devnet
```

### Verify Program ID
```bash
solana program show <PROGRAM_ID>
```

---

## 3. Testing

### Run All Integration Tests
This will spin up a local validator (unless configured otherwise in `Anchor.toml`) and execute the test suite.
```bash
cd contracts/solana/interlink-hub
anchor test
```

### Run Tests on Devnet
To run tests against the live devnet program:
```bash
ANCHOR_PROVIDER_URL=https://api.devnet.solana.com \
ANCHOR_WALLET=~/.config/solana/id.json \
yarn run ts-mocha -p ./tsconfig.json -t 1000000 "tests/**/*.ts"
```

---

## 4. On-Chain Inspection

### View State Registry (PDA)
The `StateRegistry` keeps track of the sequence and admin settings.
```bash
# Replace <STATE_REGISTRY_PDA> with the actual derived address
solana account <STATE_REGISTRY_PDA>
```

### List Program Accounts
```bash
solana program show --programs AKzpc9tvxfhLjj5AantKizK2YZgSjoyhLmYqRZy6b8Lz
```

---

## 5. Maintenance & Fees

### Buy-back and Burn
The protocol automates fee burning via this instruction:
```bash
# Note: Usually called via the relayer or a bot
anchor run burn-fees
```

### Updating Global Settings
Requires the admin keypair:
```bash
# Example script or CLI command if implemented
# anchor run update-settings --fee 50
```
