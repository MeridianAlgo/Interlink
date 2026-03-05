// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import "forge-std/Script.sol";
import "../src/InterlinkGateway.sol";

/// @dev Full deployment + VK initialization script for InterlinkGateway.
///
/// Usage (Sepolia testnet):
///   GUARDIAN_ADDRESS=0x... \
///   VK_HEX=<576-byte hex from `cargo run --bin export-vk`> \
///   forge script script/DeployAndInit.s.sol \
///     --rpc-url $SEPOLIA_RPC_URL \
///     --private-key $DEPLOYER_PRIVATE_KEY \
///     --broadcast --verify
///
/// After running, export GATEWAY_ADDRESS from the logged output and
/// add it to your .env file.
contract DeployAndInit is Script {
    function run() external {
        address guardian = vm.envAddress("GUARDIAN_ADDRESS");
        // VK is optional at deploy time — can be set later via setVerificationKey
        bytes memory vkData = _tryLoadVK();

        vm.startBroadcast();

        InterlinkGateway gateway = new InterlinkGateway(guardian);
        console2.log("InterlinkGateway deployed at:", address(gateway));
        console2.log("Guardian:", guardian);

        if (vkData.length == 576) {
            // VK provided — initialize immediately so the contract is ready
            // Note: caller must be guardian for this to succeed
            gateway.setVerificationKey(vkData);
            console2.log("Verification key set successfully (576 bytes)");
        } else {
            console2.log("No VK provided — call setVerificationKey() before processing proofs");
        }

        vm.stopBroadcast();

        // Print env var for easy copy-paste into .env
        console2.log("\n--- Add to .env ---");
        console2.log("GATEWAY_ADDRESS=", address(gateway));
        console2.log("-------------------");
    }

    /// @dev Tries to load VK from VK_HEX env var. Returns empty bytes if not set.
    function _tryLoadVK() internal view returns (bytes memory) {
        try vm.envBytes("VK_HEX") returns (bytes memory vk) {
            return vk;
        } catch {
            return new bytes(0);
        }
    }
}
