// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import "forge-std/Script.sol";
import "../src/InterlinkGateway.sol";

/// @title DeployL2 — Deploy InterLink Gateway to Optimism, Arbitrum, and Polygon
///
/// @notice Deploys the InterlinkGateway spoke contract to supported L2 networks.
///
/// Supported networks (set via --chain-id flag or FOUNDRY_CHAIN_ID env):
///   - Optimism Mainnet:  chain_id = 10
///   - Optimism Goerli:   chain_id = 420
///   - Arbitrum One:      chain_id = 42161
///   - Arbitrum Nova:     chain_id = 42170
///   - Arbitrum Goerli:   chain_id = 421613
///   - Polygon PoS:       chain_id = 137
///   - Polygon Mumbai:    chain_id = 80001
///   - Base:              chain_id = 8453
///
/// @dev L2 advantages vs Ethereum mainnet:
///   - Optimism/Base: ~1-2s sequencer finality (vs 12s Ethereum)
///   - Arbitrum:      ~1-2s sequencer finality with fraud proof window
///   - Polygon PoS:   ~2s block time, checkpoint-based finality
///
///   InterLink uses WebSocket finality detection per chain, so each L2 gets
///   its own optimized finality config (see relayer/src/finality.rs).
///
/// @dev Usage:
///   # Deploy to Arbitrum One
///   forge script script/DeployL2.s.sol:DeployL2 \
///     --rpc-url $ARBITRUM_RPC_URL \
///     --broadcast \
///     --chain-id 42161 \
///     -vvvv
///
///   # Deploy to Optimism
///   forge script script/DeployL2.s.sol:DeployL2 \
///     --rpc-url $OPTIMISM_RPC_URL \
///     --broadcast \
///     --chain-id 10 \
///     -vvvv
///
///   # Deploy to Base
///   forge script script/DeployL2.s.sol:DeployL2 \
///     --rpc-url $BASE_RPC_URL \
///     --broadcast \
///     --chain-id 8453 \
///     -vvvv
///
/// Required env vars:
///   GUARDIAN_ADDRESS  — guardian/admin EOA for the deployed gateway
///   PRIVATE_KEY       — deployer private key (use hardware wallet in prod)
contract DeployL2 is Script {
    // ─── Chain IDs ──────────────────────────────────────────────────────────

    uint256 constant OPTIMISM         = 10;
    uint256 constant OPTIMISM_GOERLI  = 420;
    uint256 constant ARBITRUM_ONE     = 42161;
    uint256 constant ARBITRUM_NOVA    = 42170;
    uint256 constant ARBITRUM_GOERLI  = 421613;
    uint256 constant POLYGON_POS      = 137;
    uint256 constant POLYGON_MUMBAI   = 80001;
    uint256 constant BASE             = 8453;
    uint256 constant BASE_GOERLI      = 84531;

    // ─── Finality config (seconds until a block is considered final) ─────────
    // Note: These are the RELAYER-side finality wait times, not L2 fraud proof windows.
    // InterLink waits for sequencer finality only (we trust the sequencer for speed,
    // and add ZK proof security on top — no need to wait for the 7-day fraud window).

    function finalitySeconds(uint256 chainId) internal pure returns (uint256) {
        if (chainId == OPTIMISM || chainId == OPTIMISM_GOERLI) return 2;   // OP sequencer
        if (chainId == BASE || chainId == BASE_GOERLI) return 2;           // Base sequencer
        if (chainId == ARBITRUM_ONE || chainId == ARBITRUM_NOVA) return 2; // Arb sequencer
        if (chainId == ARBITRUM_GOERLI) return 2;
        if (chainId == POLYGON_POS || chainId == POLYGON_MUMBAI) return 5; // PoS checkpoints
        return 12; // Ethereum mainnet fallback
    }

    function networkName(uint256 chainId) internal pure returns (string memory) {
        if (chainId == OPTIMISM)        return "Optimism Mainnet";
        if (chainId == OPTIMISM_GOERLI) return "Optimism Goerli";
        if (chainId == ARBITRUM_ONE)    return "Arbitrum One";
        if (chainId == ARBITRUM_NOVA)   return "Arbitrum Nova";
        if (chainId == ARBITRUM_GOERLI) return "Arbitrum Goerli";
        if (chainId == POLYGON_POS)     return "Polygon PoS";
        if (chainId == POLYGON_MUMBAI)  return "Polygon Mumbai";
        if (chainId == BASE)            return "Base";
        if (chainId == BASE_GOERLI)     return "Base Goerli";
        return "Unknown Network";
    }

    // ─── Main deployment ────────────────────────────────────────────────────

    function run() external {
        address guardian = vm.envAddress("GUARDIAN_ADDRESS");
        uint256 chainId = block.chainid;

        // Validate this is a supported L2 network
        require(
            chainId == OPTIMISM      ||
            chainId == OPTIMISM_GOERLI ||
            chainId == ARBITRUM_ONE  ||
            chainId == ARBITRUM_NOVA ||
            chainId == ARBITRUM_GOERLI ||
            chainId == POLYGON_POS   ||
            chainId == POLYGON_MUMBAI ||
            chainId == BASE          ||
            chainId == BASE_GOERLI,
            "DeployL2: unsupported network. Use Deploy.s.sol for Ethereum mainnet."
        );

        console2.log("=== InterLink L2 Gateway Deployment ===");
        console2.log("Network:", networkName(chainId));
        console2.log("Chain ID:", chainId);
        console2.log("Guardian:", guardian);
        console2.log(
            "Relayer finality wait (seconds):",
            finalitySeconds(chainId)
        );
        console2.log("=========================================");

        vm.startBroadcast();
        InterlinkGateway gateway = new InterlinkGateway(guardian);
        vm.stopBroadcast();

        console2.log("InterlinkGateway deployed at:", address(gateway));
        console2.log("");
        console2.log("Next steps:");
        console2.log("  1. Set GATEWAY_ADDRESS env var:", address(gateway));
        console2.log("  2. Set CHAIN_ID env var:", chainId);
        console2.log(
            "  3. Relayer will use",
            finalitySeconds(chainId),
            "second finality for this network"
        );
        console2.log("  4. Call setVK(vkBytes) as guardian to enable proof verification");

        // Emit deployment info as a Foundry artifact for scripts to pick up
        string memory deployJson = string.concat(
            '{"chain_id":', vm.toString(chainId),
            ',"network":"', networkName(chainId),
            '","gateway":"', vm.toString(address(gateway)),
            '","guardian":"', vm.toString(guardian),
            '","finality_seconds":', vm.toString(finalitySeconds(chainId)),
            '}'
        );
        console2.log("Deployment artifact:", deployJson);
    }
}

/// @notice Batch deployment: deploy to all supported L2s in one transaction bundle.
///
/// @dev Requires all network RPC URLs set as env vars:
///   ARBITRUM_RPC_URL, OPTIMISM_RPC_URL, POLYGON_RPC_URL, BASE_RPC_URL
///
/// Usage (multi-chain from a single script invocation is not natively supported
/// by Foundry — use the deploy-l2-all.sh helper script instead):
///   bash script/deploy-l2-all.sh
contract DeployL2Verify is Script {
    /// @notice Verify a deployed gateway has the correct guardian and is operational.
    function verify(address gatewayAddr, address expectedGuardian) external view {
        InterlinkGateway gateway = InterlinkGateway(gatewayAddr);

        address actualGuardian = gateway.guardian();
        require(
            actualGuardian == expectedGuardian,
            "Verification failed: guardian mismatch"
        );

        bool paused = gateway.paused();
        require(!paused, "Verification failed: gateway is paused at deployment");

        uint256 nonce = gateway.nonce();
        require(nonce == 0, "Verification failed: nonce should be 0 at deployment");

        console2.log("Verification PASSED for gateway:", gatewayAddr);
        console2.log("  Guardian:", actualGuardian);
        console2.log("  Paused:", paused);
        console2.log("  Nonce:", nonce);
    }
}
