// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

/**
 * @title IInterlinkRouter
 * @dev Router interface for cross-chain message sending.
 * Abstracts the gateway details and provides a simple API for
 * dApps to send cross-chain messages and estimate fees.
 */
interface IInterlinkRouter {
    /**
     * @dev Send a cross-chain message to a destination chain.
     *
     * @param destinationChain  the target chain id
     * @param recipient         recipient address on the destination chain (32 bytes)
     * @param payload           arbitrary payload data to be executed on destination
     * @param feeToken          token used to pay the relayer fee (address(0) for native)
     * @param feeAmount         amount of fee tokens to pay
     * @return nonce             the sequence number assigned to this message
     */
    function sendMessage(
        uint64 destinationChain,
        bytes32 recipient,
        bytes calldata payload,
        address feeToken,
        uint256 feeAmount
    ) external payable returns (uint64 nonce);

    /**
     * @dev Estimate the fee required to send a message to the destination chain.
     *
     * @param destinationChain  the target chain id
     * @param payload           the payload to be sent
     * @return feeAmount         estimated fee in native tokens
     */
    function estimateFee(
        uint64 destinationChain,
        bytes calldata payload
    ) external view returns (uint256 feeAmount);

    /**
     * @dev Returns the current sequence number (next nonce to be assigned).
     */
    function currentSequence() external view returns (uint64);

    /**
     * @dev Returns whether a specific nonce has been executed.
     */
    function isExecuted(uint64 nonce) external view returns (bool);
}
