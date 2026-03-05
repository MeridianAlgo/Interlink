// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import "forge-std/Test.sol";
import "../src/InterlinkGateway.sol";

/// @dev Minimal ERC-20 stub for testing token custody.
contract MockERC20 {
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    function mint(address to, uint256 amount) external {
        balanceOf[to] += amount;
    }

    function approve(address spender, uint256 amount) external returns (bool) {
        allowance[msg.sender][spender] = amount;
        return true;
    }

    function transferFrom(address from, address to, uint256 amount) external returns (bool) {
        require(balanceOf[from] >= amount, "insufficient balance");
        require(allowance[from][msg.sender] >= amount, "insufficient allowance");
        balanceOf[from] -= amount;
        allowance[from][msg.sender] -= amount;
        balanceOf[to] += amount;
        return true;
    }

    function transfer(address to, uint256 amount) external returns (bool) {
        require(balanceOf[msg.sender] >= amount, "insufficient balance");
        balanceOf[msg.sender] -= amount;
        balanceOf[to] += amount;
        return true;
    }
}

/// @dev Minimal ERC-721 stub for testing NFT custody.
contract MockERC721 {
    mapping(uint256 => address) public ownerOf;
    mapping(address => mapping(address => bool)) public isApprovedForAll;

    function mint(address to, uint256 tokenId) external {
        ownerOf[tokenId] = to;
    }

    function setApprovalForAll(address operator, bool approved) external {
        isApprovedForAll[msg.sender][operator] = approved;
    }

    function transferFrom(address from, address to, uint256 tokenId) external {
        require(ownerOf[tokenId] == from, "not owner");
        require(
            isApprovedForAll[from][msg.sender] || ownerOf[tokenId] == msg.sender,
            "not approved"
        );
        ownerOf[tokenId] = to;
    }
}

contract InterlinkGatewayTest is Test {
    InterlinkGateway internal gateway;
    MockERC20 internal token;
    MockERC721 internal nft;

    address internal guardian = address(0xDAD);
    address internal user = address(0xBEEF);

    function setUp() public {
        gateway = new InterlinkGateway(guardian);
        token = new MockERC20();
        nft = new MockERC721();

        // Fund user with ERC-20 tokens
        token.mint(user, 1e18);
        vm.prank(user);
        token.approve(address(gateway), type(uint256).max);

        // Mint an NFT to user
        nft.mint(user, 1);
        vm.prank(user);
        nft.setApprovalForAll(address(gateway), true);
    }

    // ─────────────────────────────────────────────────────────────
    // sendCrossChainMessage
    // ─────────────────────────────────────────────────────────────

    function testSendCrossChainMessage_Native(uint64 destChain, uint128 amount) public {
        vm.assume(amount > 0 && amount < 1e24);
        vm.deal(user, amount);
        vm.prank(user);
        gateway.sendCrossChainMessage{value: amount}(destChain, address(0), amount, "");
    }

    function testSendCrossChainMessage_Token(uint64 destChain, uint128 amount) public {
        vm.assume(amount > 0 && amount <= 1e18);
        vm.prank(user);
        gateway.sendCrossChainMessage(destChain, address(token), amount, "hello");
        assertEq(token.balanceOf(address(gateway)), amount, "gateway should hold tokens");
    }

    function testSendCrossChainMessage_EmitsEvent() public {
        vm.deal(user, 1 ether);
        vm.prank(user);
        vm.expectEmit(true, false, false, false);
        emit InterlinkGateway.MessagePublished(0, 999, user, bytes32(0), "");
        gateway.sendCrossChainMessage{value: 1 ether}(999, address(0), 1 ether, "");
    }

    function testSendCrossChainMessage_IncrementsNonce() public {
        vm.deal(user, 3 ether);
        vm.startPrank(user);
        gateway.sendCrossChainMessage{value: 1 ether}(1, address(0), 1 ether, "");
        gateway.sendCrossChainMessage{value: 1 ether}(2, address(0), 1 ether, "");
        gateway.sendCrossChainMessage{value: 1 ether}(3, address(0), 1 ether, "");
        vm.stopPrank();
        assertEq(gateway.currentNonce(), 3);
    }

    function testSendCrossChainMessage_RevertsWhenPaused() public {
        vm.prank(guardian);
        gateway.pause();
        vm.deal(user, 1 ether);
        vm.prank(user);
        vm.expectRevert("Interlink: Gateway is paused");
        gateway.sendCrossChainMessage{value: 1 ether}(1, address(0), 1 ether, "");
    }

    function testSendCrossChainMessage_RevertsWrongNativeValue() public {
        vm.deal(user, 2 ether);
        vm.prank(user);
        vm.expectRevert("Interlink: Incorrect native value sent");
        gateway.sendCrossChainMessage{value: 2 ether}(1, address(0), 1 ether, "");
    }

    // ─────────────────────────────────────────────────────────────
    // initiateSwap
    // ─────────────────────────────────────────────────────────────

    function testInitiateSwap_Native(
        uint64 destChain,
        uint128 amountIn,
        uint128 minAmountOut
    ) public {
        vm.assume(amountIn > 0 && amountIn < 1e24);
        vm.deal(user, amountIn);
        vm.prank(user);
        gateway.initiateSwap{value: amountIn}(
            destChain, address(0), amountIn, address(0x1), minAmountOut, user, ""
        );
    }

    function testInitiateSwap_Token(uint128 amountIn, uint128 minAmountOut) public {
        vm.assume(amountIn > 0 && amountIn <= 1e18);
        vm.prank(user);
        gateway.initiateSwap(1, address(token), amountIn, address(0x1), minAmountOut, user, "");
        assertEq(token.balanceOf(address(gateway)), amountIn);
    }

    function testInitiateSwap_RevertsZeroAmount() public {
        vm.prank(user);
        vm.expectRevert("Interlink: zero amount");
        gateway.initiateSwap(1, address(token), 0, address(0x1), 0, user, "");
    }

    function testInitiateSwap_RevertsZeroRecipient() public {
        vm.prank(user);
        vm.expectRevert("Interlink: zero recipient");
        gateway.initiateSwap(1, address(token), 1e6, address(0x1), 0, address(0), "");
    }

    // ─────────────────────────────────────────────────────────────
    // lockNFT
    // ─────────────────────────────────────────────────────────────

    function testLockNFT_Success(uint64 destChain) public {
        vm.assume(destChain > 0);
        bytes32 recipient = bytes32(uint256(uint160(user)));
        vm.prank(user);
        gateway.lockNFT(address(nft), 1, destChain, recipient);
        assertEq(nft.ownerOf(1), address(gateway), "gateway should hold NFT");
    }

    function testLockNFT_RevertsZeroContract() public {
        vm.prank(user);
        vm.expectRevert("Interlink: zero NFT contract");
        gateway.lockNFT(address(0), 1, 1, bytes32(uint256(1)));
    }

    function testLockNFT_RevertsZeroRecipient() public {
        vm.prank(user);
        vm.expectRevert("Interlink: zero recipient");
        gateway.lockNFT(address(nft), 1, 1, bytes32(0));
    }

    // ─────────────────────────────────────────────────────────────
    // executeVerifiedMessage
    // ─────────────────────────────────────────────────────────────

    function testExecuteVerifiedMessage_RejectsWithoutVK() public {
        bytes memory proof = new bytes(256);
        vm.expectRevert("Interlink: VK not initialized");
        gateway.executeVerifiedMessage(address(this), 0, "", proof);
    }

    function testExecuteVerifiedMessage_RejectsShortProof() public {
        _setDummyVK();
        bytes memory shortProof = new bytes(100);
        vm.expectRevert("Interlink: Invalid Groth16 proof");
        gateway.executeVerifiedMessage(address(this), 0, "", shortProof);
    }

    function testExecuteVerifiedMessage_RejectsReplay() public {
        // Mark nonce 77 as executed via storage manipulation
        bytes32 slot = keccak256(abi.encode(uint64(77), uint256(1)));
        vm.store(address(gateway), slot, bytes32(uint256(1)));

        _setDummyVK();
        bytes memory proof = new bytes(256);
        vm.expectRevert("Interlink: Message already executed");
        gateway.executeVerifiedMessage(address(this), 77, "", proof);
    }

    function testExecuteVerifiedMessage_RejectsZeroTarget() public {
        _setDummyVK();
        bytes memory proof = new bytes(256);
        vm.expectRevert("Interlink: zero target");
        gateway.executeVerifiedMessage(address(0), 0, "", proof);
    }

    // ─────────────────────────────────────────────────────────────
    // setVerificationKey
    // ─────────────────────────────────────────────────────────────

    function testSetVK_Success() public {
        _setDummyVK();
        assertTrue(gateway.vkInitialized());
    }

    function testSetVK_RevertsNonGuardian() public {
        vm.prank(user);
        vm.expectRevert("Interlink: Unauthorized");
        gateway.setVerificationKey(
            [uint256(0), 0], [[uint256(0), 0], [uint256(0), 0]],
            [[uint256(0), 0], [uint256(0), 0]], [[uint256(0), 0], [uint256(0), 0]],
            [uint256(0), 0], [uint256(0), 0]
        );
    }

    // ─────────────────────────────────────────────────────────────
    // Admin
    // ─────────────────────────────────────────────────────────────

    function testPauseUnpause() public {
        vm.prank(guardian);
        gateway.pause();
        assertTrue(gateway.paused());

        vm.prank(guardian);
        gateway.unpause();
        assertFalse(gateway.paused());
    }

    function testPause_RevertsNonGuardian() public {
        vm.prank(user);
        vm.expectRevert("Interlink: Unauthorized");
        gateway.pause();
    }

    function testEmergencyWithdraw_ETH() public {
        vm.deal(address(gateway), 1 ether);
        uint256 before = guardian.balance;
        vm.prank(guardian);
        gateway.emergencyWithdraw(address(0), guardian, 1 ether);
        assertEq(guardian.balance - before, 1 ether);
    }

    function testEmergencyWithdraw_Token() public {
        token.mint(address(gateway), 500);
        vm.prank(guardian);
        gateway.emergencyWithdraw(address(token), guardian, 500);
        assertEq(token.balanceOf(guardian), 500);
    }

    // ─── Helpers ─────────────────────────────────────────────────

    function _setDummyVK() internal {
        vm.prank(guardian);
        gateway.setVerificationKey(
            [uint256(1), 2],
            [[uint256(1), 2], [uint256(3), 4]],
            [[uint256(1), 2], [uint256(3), 4]],
            [[uint256(1), 2], [uint256(3), 4]],
            [uint256(1), 2],
            [uint256(1), 2]
        );
    }

    receive() external payable {}
}
