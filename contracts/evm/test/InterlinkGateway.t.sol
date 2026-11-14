// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import "forge-std/Test.sol";
import "../src/InterlinkGateway.sol";

/// @dev Standard ERC-20 returning bool.
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

    function transferFrom(address from, address to, uint256 amount) external virtual returns (bool) {
        _move(from, to, amount);
        return true;
    }

    function transfer(address to, uint256 amount) external returns (bool) {
        require(balanceOf[msg.sender] >= amount, "insufficient balance");
        balanceOf[msg.sender] -= amount;
        balanceOf[to] += amount;
        return true;
    }

    function _move(address from, address to, uint256 amount) internal virtual {
        require(balanceOf[from] >= amount, "insufficient balance");
        require(allowance[from][msg.sender] >= amount, "insufficient allowance");
        balanceOf[from] -= amount;
        allowance[from][msg.sender] -= amount;
        balanceOf[to] += amount;
    }
}

/// @dev USDT-style token: transferFrom returns nothing. The old gateway reverted on these.
contract MockNoReturnToken is MockERC20 {
    function transferFrom(address from, address to, uint256 amount) external override returns (bool) {
        _move(from, to, amount);
        assembly {
            return(0, 0)
        }
    }
}

/// @dev Burns 1% on every transfer. The old gateway recorded the requested amount, not the received one.
contract MockFeeToken is MockERC20 {
    function _move(address from, address to, uint256 amount) internal override {
        require(balanceOf[from] >= amount, "insufficient balance");
        require(allowance[from][msg.sender] >= amount, "insufficient allowance");
        balanceOf[from] -= amount;
        allowance[from][msg.sender] -= amount;
        balanceOf[to] += amount - amount / 100;
    }
}

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
        require(isApprovedForAll[from][msg.sender] || from == msg.sender, "not approved");
        ownerOf[tokenId] = to;
    }
}

contract InterlinkGatewayTest is Test {
    uint64 internal constant HUB = 2;
    bytes32 internal constant HUB_PROGRAM = bytes32(uint256(0x7B7B7B7B7B7B7B7B7B7B7B7B7B7B7B7B7B7B7B7B7B7B7B7B7B7B7B7B7B7B7B7B));
    bytes32 internal constant SOL_RECIPIENT = bytes32(uint256(0x4444));

    InterlinkGateway internal gateway;
    MockERC20 internal token;
    MockERC721 internal nft;

    address internal guardian = address(0xDAD);
    address internal user = address(0xBEEF0);

    function setUp() public {
        gateway = new InterlinkGateway(guardian);
        token = new MockERC20();
        nft = new MockERC721();

        vm.prank(guardian);
        gateway.setRemoteGateway(HUB, HUB_PROGRAM);

        token.mint(user, 1e18);
        vm.prank(user);
        token.approve(address(gateway), type(uint256).max);

        nft.mint(user, 1);
        vm.prank(user);
        nft.setApprovalForAll(address(gateway), true);
    }

    // ─── sendCrossChainMessage ──────────────────────────────────────────────

    function testSend_Native(uint128 amount) public {
        vm.assume(amount > 0 && amount < 1e24);
        vm.deal(user, amount);
        vm.prank(user);
        gateway.sendCrossChainMessage{value: amount}(HUB, address(0), amount, SOL_RECIPIENT, "");
        assertEq(gateway.totalLocked(address(0)), amount);
    }

    function testSend_Token(uint128 amount) public {
        vm.assume(amount > 0 && amount <= 1e18);
        vm.prank(user);
        gateway.sendCrossChainMessage(HUB, address(token), amount, SOL_RECIPIENT, "hello");
        assertEq(token.balanceOf(address(gateway)), amount);
        assertEq(gateway.totalLocked(address(token)), amount);
        assertTrue(gateway.isCustodiedAsset(address(token)));
    }

    function testSend_EmitsCanonicalHash() public {
        vm.deal(user, 1 ether);
        bytes32 expected = gateway.hashMessage(
            InterlinkGateway.Message({
                srcChain: uint64(block.chainid),
                srcGateway: bytes32(uint256(uint160(address(gateway)))),
                dstChain: HUB,
                dstGateway: HUB_PROGRAM,
                nonce: 0,
                action: 1,
                sender: bytes32(uint256(uint160(user))),
                asset: bytes32(0),
                amount: 1 ether,
                recipient: SOL_RECIPIENT,
                extraHash: keccak256("")
            })
        );
        vm.expectEmit(true, true, false, true);
        emit InterlinkGateway.MessagePublished(
            expected, 0, HUB, HUB_PROGRAM, 1, bytes32(uint256(uint160(user))), bytes32(0),
            1 ether, SOL_RECIPIENT, keccak256(""), ""
        );
        vm.prank(user);
        bytes32 h = gateway.sendCrossChainMessage{value: 1 ether}(HUB, address(0), 1 ether, SOL_RECIPIENT, "");
        assertEq(h, expected);
    }

    function testSend_IncrementsNonce() public {
        vm.deal(user, 3 ether);
        vm.startPrank(user);
        for (uint256 i; i < 3; ++i) {
            gateway.sendCrossChainMessage{value: 1 ether}(HUB, address(0), 1 ether, SOL_RECIPIENT, "");
        }
        vm.stopPrank();
        assertEq(gateway.currentNonce(), 3);
    }

    function testSend_RevertsUnsupportedDestination() public {
        vm.deal(user, 1 ether);
        vm.prank(user);
        vm.expectRevert("Interlink: unsupported destination");
        gateway.sendCrossChainMessage{value: 1 ether}(999, address(0), 1 ether, SOL_RECIPIENT, "");
    }

    function testSend_RevertsZeroRecipient() public {
        vm.deal(user, 1 ether);
        vm.prank(user);
        vm.expectRevert("Interlink: zero recipient");
        gateway.sendCrossChainMessage{value: 1 ether}(HUB, address(0), 1 ether, bytes32(0), "");
    }

    function testSend_RevertsWhenPaused() public {
        vm.prank(guardian);
        gateway.pause();
        vm.deal(user, 1 ether);
        vm.prank(user);
        vm.expectRevert("Interlink: Gateway is paused");
        gateway.sendCrossChainMessage{value: 1 ether}(HUB, address(0), 1 ether, SOL_RECIPIENT, "");
    }

    function testSend_RevertsWrongNativeValue() public {
        vm.deal(user, 2 ether);
        vm.prank(user);
        vm.expectRevert("Interlink: Incorrect native value sent");
        gateway.sendCrossChainMessage{value: 2 ether}(HUB, address(0), 1 ether, SOL_RECIPIENT, "");
    }

    /// The old gateway silently kept ETH sent alongside an ERC-20 deposit.
    function testSend_RevertsNativeWithToken() public {
        vm.deal(user, 1 ether);
        vm.prank(user);
        vm.expectRevert("Interlink: unexpected native value");
        gateway.sendCrossChainMessage{value: 1 ether}(HUB, address(token), 1e6, SOL_RECIPIENT, "");
    }

    function testSend_NoReturnTokenWorks() public {
        MockNoReturnToken usdt = new MockNoReturnToken();
        usdt.mint(user, 1e6);
        vm.startPrank(user);
        usdt.approve(address(gateway), type(uint256).max);
        gateway.sendCrossChainMessage(HUB, address(usdt), 1e6, SOL_RECIPIENT, "");
        vm.stopPrank();
        assertEq(usdt.balanceOf(address(gateway)), 1e6);
    }

    function testSend_FeeOnTransferRecordsReceived() public {
        MockFeeToken fot = new MockFeeToken();
        fot.mint(user, 1000);
        vm.startPrank(user);
        fot.approve(address(gateway), type(uint256).max);
        gateway.sendCrossChainMessage(HUB, address(fot), 1000, SOL_RECIPIENT, "");
        vm.stopPrank();
        // 1% burned in transit: only 990 arrived, so only 990 may be minted remotely.
        assertEq(gateway.totalLocked(address(fot)), 990);
    }

    // ─── initiateSwap ───────────────────────────────────────────────────────

    function testInitiateSwap_Native(uint128 amountIn, uint64 minAmountOut) public {
        vm.assume(amountIn > 0 && amountIn < 1e24);
        vm.deal(user, amountIn);
        vm.prank(user);
        gateway.initiateSwap{value: amountIn}(
            HUB, address(0), amountIn, bytes32(uint256(1)), minAmountOut, SOL_RECIPIENT, ""
        );
    }

    function testInitiateSwap_Token(uint128 amountIn) public {
        vm.assume(amountIn > 0 && amountIn <= 1e18);
        vm.prank(user);
        gateway.initiateSwap(HUB, address(token), amountIn, bytes32(uint256(1)), 0, SOL_RECIPIENT, "");
        assertEq(token.balanceOf(address(gateway)), amountIn);
    }

    function testInitiateSwap_RevertsZeroAmount() public {
        vm.prank(user);
        vm.expectRevert("Interlink: zero amount");
        gateway.initiateSwap(HUB, address(token), 0, bytes32(uint256(1)), 0, SOL_RECIPIENT, "");
    }

    function testInitiateSwap_RevertsZeroRecipient() public {
        vm.prank(user);
        vm.expectRevert("Interlink: zero recipient");
        gateway.initiateSwap(HUB, address(token), 1e6, bytes32(uint256(1)), 0, bytes32(0), "");
    }

    /// Solana's min_amount_out is a u64; a larger value could never be executed there.
    function testInitiateSwap_RevertsMinOutTooLarge() public {
        vm.prank(user);
        vm.expectRevert("Interlink: minAmountOut too large");
        gateway.initiateSwap(
            HUB, address(token), 1e6, bytes32(uint256(1)), uint256(type(uint64).max) + 1, SOL_RECIPIENT, ""
        );
    }

    // ─── lockNFT ────────────────────────────────────────────────────────────

    function _enableNfts() internal {
        vm.prank(guardian);
        gateway.setNftBridgingEnabled(true);
    }

    /// No destination mints NFTs yet, so locking must be refused by default.
    function testLockNFT_DisabledByDefault() public {
        vm.prank(user);
        vm.expectRevert("Interlink: NFT bridging disabled");
        gateway.lockNFT(address(nft), 1, HUB, SOL_RECIPIENT);
    }

    function testLockNFT_Success() public {
        _enableNfts();
        vm.prank(user);
        gateway.lockNFT(address(nft), 1, HUB, SOL_RECIPIENT);
        assertEq(nft.ownerOf(1), address(gateway));
        assertTrue(gateway.isCustodiedAsset(address(nft)));
    }

    function testLockNFT_RevertsZeroContract() public {
        _enableNfts();
        vm.prank(user);
        vm.expectRevert("Interlink: zero NFT contract");
        gateway.lockNFT(address(0), 1, HUB, SOL_RECIPIENT);
    }

    function testLockNFT_RevertsZeroRecipient() public {
        _enableNfts();
        vm.prank(user);
        vm.expectRevert("Interlink: zero recipient");
        gateway.lockNFT(address(nft), 1, HUB, bytes32(0));
    }

    // ─── Inbound ────────────────────────────────────────────────────────────

    function _inbound(uint64 srcChain, uint64 nonce, uint256 amount, bytes32 recipient)
        internal
        pure
        returns (InterlinkGateway.InboundMessage memory)
    {
        return InterlinkGateway.InboundMessage({
            srcChain: srcChain,
            nonce: nonce,
            sender: bytes32(uint256(0x2222222222222222222222222222222222222222222222222222222222222222)),
            asset: bytes32(0),
            amount: amount,
            recipient: recipient,
            extraHash: bytes32(0)
        });
    }

    function testRelease_RejectsWithoutVK() public {
        vm.expectRevert("Interlink: VK not initialized");
        gateway.releaseTokens(_inbound(HUB, 0, 1, bytes32(uint256(1))), new bytes(256));
    }

    function testRelease_RejectsUnknownSource() public {
        vm.expectRevert("Interlink: unknown source");
        gateway.releaseTokens(_inbound(777, 0, 1, bytes32(uint256(1))), new bytes(256));
    }

    function testRelease_RejectsShortProof() public {
        _setVK();
        vm.expectRevert("Interlink: Invalid Groth16 proof");
        gateway.releaseTokens(_inbound(HUB, 0, 1, bytes32(uint256(1))), new bytes(100));
    }

    function testExecute_RejectsGatewayTarget() public {
        InterlinkGateway.InboundMessage memory m = _inbound(HUB, 0, 0, bytes32(uint256(uint160(address(gateway)))));
        m.extraHash = keccak256("");
        vm.expectRevert("Interlink: cannot target gateway");
        gateway.executeVerifiedMessage(m, "", new bytes(256));
    }

    /// The Poly Network pattern: a verified message calling transfer() on a token the gateway holds.
    function testExecute_RejectsCustodiedAssetTarget() public {
        vm.prank(user);
        gateway.sendCrossChainMessage(HUB, address(token), 1e6, SOL_RECIPIENT, "");

        bytes memory drain = abi.encodeWithSignature("transfer(address,uint256)", user, 1e6);
        InterlinkGateway.InboundMessage memory m = _inbound(HUB, 0, 0, bytes32(uint256(uint160(address(token)))));
        m.extraHash = keccak256(drain);
        vm.expectRevert("Interlink: cannot target custodied asset");
        gateway.executeVerifiedMessage(m, drain, new bytes(256));
    }

    function testExecute_RejectsPayloadMismatch() public {
        InterlinkGateway.InboundMessage memory m = _inbound(HUB, 0, 0, bytes32(uint256(uint160(address(nft)))));
        vm.expectRevert("Interlink: payload mismatch");
        gateway.executeVerifiedMessage(m, "abc", new bytes(256));
    }

    // ─── Real Groth16 proof (vectors generated with arkworks, same circuit as the relayer) ──

    address internal constant FIXED_GATEWAY = address(0xBEEF);
    address internal constant ALICE = address(bytes20(hex"a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1"));

    bytes internal constant VECTOR_VK =
        hex"0c979f59e5895b10449268398f0db786395c5ea0f1639a4105d2da89c7a698d31c26c5ea66e22593ca04900a58f911e02cb8120ff8d33f5d6a5f09f0fe80f92226008bcf4712e4ab5fd29d1d2632f5ba0bfbbcd3ecb1771bd112d4a1032a083e237a26cb82383692eb3549cc2d68742b77640dbc85a96326372f331748754bb81b2d67f7335c72b71bcb733b640ede3a4c6717547044e2327cbd54cf48a0b4c905beccc090ee25c17faf35a961abd4b6a7d838218b9beffde40508b76ea89d5319b5573a88d18b7ddbafc40e6e599564b095753f6705dfb31977a444366444fc0f1a58c5d2e95f98adbd5b8d48ab566741090b45622857b4bdc7b6970ce9214727a16870423faf5043c301cc910b5b25cf099d5ca81077b7b7b6a2ccb274688f29aaa08b9f4215b34868c2f46f3be1c2127eb3ca20bfb3320126a65b6586df960805e000d90aa564a32ba87ff512414ae1d21a4030b88e83c0eb63945c1d9132289cd2569be6afe09adce2da6a1c3756b20c19e02e40fde823d1acaa14fd0c220fde4483949def9e453f6fc6d92fd2310de0c0c14fdbfc0732588921d43ceaa81dfadabc69a3ccc1324950584809b578303b93de910ea913eb6ddeacb499cc7d093248e2a1bb61fe40204550b442301cef82927fcdf4666214b788b6b125a0f920ee1dd515db33cf0bbe53fe1a5497d6f2c118c77fd9d1a1122ad630227a176d26e020ca711f929004761bab615cd9c49c99555cdca2f7729cd67863573ee8902f4f49f31ac94ec1622e485395ae960d379b2712518f07243487b520f5db5bee";
    bytes internal constant VECTOR_PROOF =
        hex"0fed83419f7f06715c3e7801df6367567e91ff5bdd1de06451fa90ae3bbf20642a8d58599f50ef49aa10f9479c3e8a29681a5b376ea9e9db80d3df8d8d519a363054c78659383754e249470acca978d435840c9f14fec9fc04bfd7325fadeb951b283c66be1b817ef3c0c82ac5fa22456dc4400471986d4cb3fdebfe48f632081984b0fc46c9061b069e63da6aed37f5a3983320fde262ba0bdfdb5f6bd6ba892a8bc753b802634b6bc326beca87b9bc528744881f89e819420b6c09fb86406320aba599a62f1bca3505a30f0b2c09038672263d9d3cc3d3de3e69e16d2af297269fa19aa435b065b07f196064bd0eecef162b48030c14827ac96dbf34f475b5";
    bytes32 internal constant VECTOR_HASH = 0x07b0f8b8ce5c6f82e0bb552073f1c35df9b0987c6e2c598c52ed5b59ee1c09bd;

    function _fixedGateway() internal returns (InterlinkGateway g) {
        vm.chainId(31337);
        deployCodeTo("InterlinkGateway.sol:InterlinkGateway", abi.encode(guardian), FIXED_GATEWAY);
        g = InterlinkGateway(FIXED_GATEWAY);
        vm.startPrank(guardian);
        g.setRemoteGateway(HUB, HUB_PROGRAM);
        g.setVerificationKey(VECTOR_VK);
        vm.stopPrank();
        // Lock 5 ETH so there is custody to release.
        vm.deal(user, 5 ether);
        vm.prank(user);
        g.sendCrossChainMessage{value: 5 ether}(HUB, address(0), 5 ether, SOL_RECIPIENT, "");
    }

    function testRealProof_ReleasesExactlyOnce() public {
        InterlinkGateway g = _fixedGateway();
        InterlinkGateway.InboundMessage memory m = _inbound(HUB, 7, 2 ether, bytes32(uint256(uint160(ALICE))));

        g.releaseTokens(m, VECTOR_PROOF);
        assertEq(ALICE.balance, 2 ether);
        assertEq(g.totalLocked(address(0)), 3 ether);
        assertTrue(g.executedMessages(VECTOR_HASH));

        vm.expectRevert("Interlink: Message already executed");
        g.releaseTokens(m, VECTOR_PROOF);
    }

    function testRealProof_RejectsTamperedAmount() public {
        InterlinkGateway g = _fixedGateway();
        InterlinkGateway.InboundMessage memory m = _inbound(HUB, 7, 3 ether, bytes32(uint256(uint160(ALICE))));
        vm.expectRevert("Interlink: Invalid Groth16 proof");
        g.releaseTokens(m, VECTOR_PROOF);
    }

    function testRealProof_RejectsTamperedRecipient() public {
        InterlinkGateway g = _fixedGateway();
        InterlinkGateway.InboundMessage memory m = _inbound(HUB, 7, 2 ether, bytes32(uint256(uint160(user))));
        vm.expectRevert("Interlink: Invalid Groth16 proof");
        g.releaseTokens(m, VECTOR_PROOF);
    }

    // ─── Verification key ───────────────────────────────────────────────────

    function testSetVK_Success() public {
        _setVK();
        assertTrue(gateway.vkInitialized());
    }

    function testSetVK_OnlyOnce() public {
        _setVK();
        vm.prank(guardian);
        vm.expectRevert("Interlink: use proposeVerificationKey");
        gateway.setVerificationKey(VECTOR_VK);
    }

    function testSetVK_RevertsNonGuardian() public {
        vm.prank(user);
        vm.expectRevert("Interlink: Unauthorized");
        gateway.setVerificationKey(VECTOR_VK);
    }

    function testSetVK_RevertsWrongLength() public {
        vm.prank(guardian);
        vm.expectRevert("Interlink: VK must be 576 bytes");
        gateway.setVerificationKey(new bytes(100));
    }

    function testVKRotation_Timelocked() public {
        _setVK();
        vm.startPrank(guardian);
        gateway.proposeVerificationKey(VECTOR_VK);
        vm.expectRevert("Interlink: timelock");
        gateway.applyVerificationKey();
        vm.warp(block.timestamp + gateway.TIMELOCK());
        gateway.applyVerificationKey();
        vm.stopPrank();
    }

    // ─── Remote gateways ────────────────────────────────────────────────────

    function testRemote_ChangeRequiresTimelock() public {
        vm.startPrank(guardian);
        vm.expectRevert("Interlink: use proposeRemoteGateway");
        gateway.setRemoteGateway(HUB, bytes32(uint256(1)));

        gateway.proposeRemoteGateway(HUB, bytes32(uint256(1)));
        vm.expectRevert("Interlink: timelock");
        gateway.applyRemoteGateway(HUB);
        vm.warp(block.timestamp + gateway.TIMELOCK());
        gateway.applyRemoteGateway(HUB);
        vm.stopPrank();
        assertEq(gateway.remoteGateways(HUB), bytes32(uint256(1)));
    }

    /// Disable-then-set must not bypass the timelock.
    function testRemote_DisableThenSetStillTimelocked() public {
        vm.startPrank(guardian);
        gateway.disableRemoteGateway(HUB);
        vm.expectRevert("Interlink: use proposeRemoteGateway");
        gateway.setRemoteGateway(HUB, bytes32(uint256(1)));
        vm.stopPrank();
    }

    // ─── Guardian / admin ───────────────────────────────────────────────────

    function testGuardian_TwoStepTransfer() public {
        address next = address(0x1234);
        vm.prank(guardian);
        gateway.transferGuardian(next);
        assertEq(gateway.guardian(), guardian);

        vm.prank(user);
        vm.expectRevert("Interlink: not pending guardian");
        gateway.acceptGuardian();

        vm.prank(next);
        gateway.acceptGuardian();
        assertEq(gateway.guardian(), next);
    }

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

    function testEmergencyWithdraw_RequiresPause() public {
        vm.deal(address(gateway), 1 ether);
        vm.prank(guardian);
        vm.expectRevert("Interlink: pause first");
        gateway.emergencyWithdraw(address(0), guardian, 1 ether);
    }

    function testEmergencyWithdraw_ETH() public {
        vm.deal(address(gateway), 1 ether);
        vm.startPrank(guardian);
        gateway.pause();
        uint256 before = guardian.balance;
        gateway.emergencyWithdraw(address(0), guardian, 1 ether);
        vm.stopPrank();
        assertEq(guardian.balance - before, 1 ether);
    }

    function testEmergencyWithdraw_Token() public {
        token.mint(address(gateway), 500);
        vm.startPrank(guardian);
        gateway.pause();
        gateway.emergencyWithdraw(address(token), guardian, 500);
        vm.stopPrank();
        assertEq(token.balanceOf(guardian), 500);
    }

    // ─── Helpers ────────────────────────────────────────────────────────────

    function _setVK() internal {
        vm.prank(guardian);
        gateway.setVerificationKey(VECTOR_VK);
    }
}
