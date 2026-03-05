// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import "forge-std/Script.sol";
import "../src/InterlinkGateway.sol";

contract DeployInterlinkGateway is Script {
    function run() external {
        address guardian = vm.envAddress("GUARDIAN_ADDRESS");

        vm.startBroadcast();
        InterlinkGateway gateway = new InterlinkGateway(guardian);
        vm.stopBroadcast();

        console2.log("InterlinkGateway deployed at:", address(gateway));
        console2.log("Guardian:", guardian);
    }
}
