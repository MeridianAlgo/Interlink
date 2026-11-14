// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

/**
 * @title InterlinkGateway
 * @dev Spoke gateway for EVM chains: custody, outbound messages, verified inbound messages.
 *
 * Canonical message (shared with the Solana hub and relayer):
 *
 *   payloadHash = keccak256(abi.encode(
 *       keccak256("InterlinkMessage(v1)"),
 *       uint64 srcChain,  bytes32 srcGateway,
 *       uint64 dstChain,  bytes32 dstGateway,
 *       uint64 nonce,     uint8   action,
 *       bytes32 sender,   bytes32 asset,
 *       uint256 amount,   bytes32 recipient,
 *       bytes32 extraHash))
 *
 * Both chain IDs and both gateway addresses are bound, so a message cannot be
 * replayed against another deployment or another chain. Recipients are
 * bytes32 so Solana addresses fit.
 *
 * Inbound proofs are Groth16 over BN254 with one public input:
 *   commitment = (payloadHash mod r + rc)^5 + nonce   (mod r)
 * which is the relation the relayer's circuit proves.
 *
 * Trust changes (verification key, remote gateways, guardian) are timelocked
 * or two-step; emergency withdrawal requires the gateway to be paused.
 *
 * NOTE: the circuit currently proves only knowledge of a preimage of the
 * commitment; it does NOT prove that the message was emitted on the source
 * chain. See docs/SECURITY_REVIEW.md before holding real value here.
 */

interface IERC20Balance {
    function balanceOf(address account) external view returns (uint256);
}

interface IERC721 {
    function transferFrom(address from, address to, uint256 tokenId) external;
    function ownerOf(uint256 tokenId) external view returns (address);
}

contract InterlinkGateway {
    // ─── Constants ──────────────────────────────────────────────────────────

    /// BN254 scalar field r
    uint256 internal constant R =
        21888242871839275222246405745257275088548364400416034343698204186575808495617;
    /// BN254 base field p
    uint256 internal constant P =
        21888242871839275222246405745257275088696311157297823662689037894645226208583;
    /// Circuit round constant: keccak256("interlink_v1_domain") mod r
    uint256 internal constant ROUND_CONSTANT = uint256(keccak256("interlink_v1_domain")) % R;

    bytes32 public constant MESSAGE_TYPEHASH = keccak256("InterlinkMessage(v1)");
    uint256 public constant TIMELOCK = 2 days;

    uint8 public constant ACTION_TRANSFER = 1;
    uint8 public constant ACTION_SWAP = 2;
    uint8 public constant ACTION_NFT = 3;
    uint8 public constant ACTION_CALL = 4;

    // ─── Types ──────────────────────────────────────────────────────────────

    struct Message {
        uint64 srcChain;
        bytes32 srcGateway;
        uint64 dstChain;
        bytes32 dstGateway;
        uint64 nonce;
        uint8 action;
        bytes32 sender;
        bytes32 asset;
        uint256 amount;
        bytes32 recipient;
        bytes32 extraHash;
    }

    /// Inbound message fields supplied by the caller. Source gateway and
    /// destination are not caller-controlled: they come from config and this contract.
    struct InboundMessage {
        uint64 srcChain;
        uint64 nonce;
        bytes32 sender;
        bytes32 asset;
        uint256 amount;
        bytes32 recipient;
        bytes32 extraHash;
    }

    struct PendingRemote {
        bytes32 gateway;
        uint256 eta;
    }

    // ─── State ──────────────────────────────────────────────────────────────

    address public guardian;
    address public pendingGuardian;
    bool public paused;
    uint256 private _reentrancy = 1;

    uint64 public currentNonce;
    /// Inbound replay protection, keyed by canonical payload hash.
    mapping(bytes32 => bool) public executedMessages;
    /// Trusted gateway per remote chain. Doubles as the destination allowlist.
    mapping(uint64 => bytes32) public remoteGateways;
    mapping(uint64 => bool) public remoteEverConfigured;
    mapping(uint64 => PendingRemote) public pendingRemoteGateways;
    /// Amount of each asset backing bridged supply. Releases can never exceed it.
    mapping(address => uint256) public totalLocked;
    /// Assets the gateway custodies. `executeVerifiedMessage` may never call them.
    mapping(address => bool) public isCustodiedAsset;
    /// No destination mints or returns NFTs yet, so locking is off until one does.
    bool public nftBridgingEnabled;

    /// Verification key as 18 words:
    /// [0..1] alpha | [2..5] beta | [6..9] gamma | [10..13] delta | [14..15] ic0 | [16..17] ic1
    /// G2 words are (x_im, x_re, y_im, y_re), the EIP-197 precompile order.
    uint256[18] internal _vk;
    bool public vkInitialized;
    uint256[18] internal _pendingVk;
    uint256 public pendingVkEta;

    // ─── Events ─────────────────────────────────────────────────────────────

    event MessagePublished(
        bytes32 indexed payloadHash,
        uint64 indexed nonce,
        uint64 dstChain,
        bytes32 dstGateway,
        uint8 action,
        bytes32 sender,
        bytes32 asset,
        uint256 amount,
        bytes32 recipient,
        bytes32 extraHash,
        bytes data
    );
    event MessageExecuted(bytes32 indexed payloadHash, uint64 indexed srcChain, uint64 nonce);

    event GatewayPaused();
    event GatewayUnpaused();
    event EmergencyWithdraw(address indexed token, address indexed to, uint256 amount);
    event GuardianProposed(address indexed pendingGuardian);
    event GuardianTransferred(address indexed previousGuardian, address indexed newGuardian);
    event RemoteGatewaySet(uint64 indexed chainId, bytes32 gateway);
    event RemoteGatewayProposed(uint64 indexed chainId, bytes32 gateway, uint256 eta);
    event VerificationKeyProposed(uint256 eta);
    event VerificationKeyUpdated();
    event NftBridgingSet(bool enabled);

    // ─── Modifiers ──────────────────────────────────────────────────────────

    modifier onlyGuardian() {
        require(msg.sender == guardian, "Interlink: Unauthorized");
        _;
    }

    modifier whenNotPaused() {
        require(!paused, "Interlink: Gateway is paused");
        _;
    }

    modifier nonReentrant() {
        require(_reentrancy == 1, "Interlink: reentrant call");
        _reentrancy = 2;
        _;
        _reentrancy = 1;
    }

    constructor(address _guardian) {
        require(_guardian != address(0), "Interlink: zero guardian");
        guardian = _guardian;
    }

    // ─── Admin ──────────────────────────────────────────────────────────────

    function pause() external onlyGuardian {
        paused = true;
        emit GatewayPaused();
    }

    function unpause() external onlyGuardian {
        paused = false;
        emit GatewayUnpaused();
    }

    /// @dev Step 1 of 2. The guardian should be a multisig/timelock, not an EOA.
    function transferGuardian(address newGuardian) external onlyGuardian {
        pendingGuardian = newGuardian;
        emit GuardianProposed(newGuardian);
    }

    /// @dev Step 2 of 2: the nominee proves it controls the address.
    function acceptGuardian() external {
        require(msg.sender == pendingGuardian, "Interlink: not pending guardian");
        emit GuardianTransferred(guardian, msg.sender);
        guardian = msg.sender;
        pendingGuardian = address(0);
    }

    /// @dev Only while paused, so funds cannot be pulled silently during normal operation.
    function emergencyWithdraw(address token, address to, uint256 amount) external onlyGuardian {
        require(paused, "Interlink: pause first");
        require(to != address(0), "Interlink: zero recipient");
        uint256 locked = totalLocked[token];
        totalLocked[token] = amount > locked ? 0 : locked - amount;
        _send(token, to, amount);
        emit EmergencyWithdraw(token, to, amount);
    }

    /// @dev Initial configuration of a remote chain takes effect immediately.
    /// Any later change must go through propose/apply with the timelock.
    function setRemoteGateway(uint64 chainId, bytes32 gateway) external onlyGuardian {
        require(gateway != bytes32(0), "Interlink: zero gateway");
        require(!remoteEverConfigured[chainId], "Interlink: use proposeRemoteGateway");
        remoteEverConfigured[chainId] = true;
        remoteGateways[chainId] = gateway;
        emit RemoteGatewaySet(chainId, gateway);
    }

    function proposeRemoteGateway(uint64 chainId, bytes32 gateway) external onlyGuardian {
        require(gateway != bytes32(0), "Interlink: zero gateway");
        uint256 eta = block.timestamp + TIMELOCK;
        pendingRemoteGateways[chainId] = PendingRemote(gateway, eta);
        emit RemoteGatewayProposed(chainId, gateway, eta);
    }

    function applyRemoteGateway(uint64 chainId) external onlyGuardian {
        PendingRemote memory p = pendingRemoteGateways[chainId];
        require(p.gateway != bytes32(0), "Interlink: nothing pending");
        require(block.timestamp >= p.eta, "Interlink: timelock");
        delete pendingRemoteGateways[chainId];
        remoteEverConfigured[chainId] = true;
        remoteGateways[chainId] = p.gateway;
        emit RemoteGatewaySet(chainId, p.gateway);
    }

    /// @dev Disabling a route reduces trust, so it is immediate.
    function disableRemoteGateway(uint64 chainId) external onlyGuardian {
        delete remoteGateways[chainId];
        emit RemoteGatewaySet(chainId, bytes32(0));
    }

    /// @dev Enable only once a destination can mint and return bridged NFTs.
    function setNftBridgingEnabled(bool enabled) external onlyGuardian {
        nftBridgingEnabled = enabled;
        emit NftBridgingSet(enabled);
    }

    /// @notice Set the initial Groth16 verification key (576 bytes, as produced by
    /// `cargo run --bin export-vk`). Replacement keys go through proposeVerificationKey.
    function setVerificationKey(bytes calldata vkData) external onlyGuardian {
        require(!vkInitialized, "Interlink: use proposeVerificationKey");
        _vk = _decodeVk(vkData);
        vkInitialized = true;
        emit VerificationKeyUpdated();
    }

    function proposeVerificationKey(bytes calldata vkData) external onlyGuardian {
        _pendingVk = _decodeVk(vkData);
        pendingVkEta = block.timestamp + TIMELOCK;
        emit VerificationKeyProposed(pendingVkEta);
    }

    function applyVerificationKey() external onlyGuardian {
        require(pendingVkEta != 0, "Interlink: nothing pending");
        require(block.timestamp >= pendingVkEta, "Interlink: timelock");
        _vk = _pendingVk;
        delete _pendingVk;
        pendingVkEta = 0;
        vkInitialized = true;
        emit VerificationKeyUpdated();
    }

    function verificationKey() external view returns (uint256[18] memory) {
        return _vk;
    }

    // ─── Outbound ───────────────────────────────────────────────────────────

    /// @notice Lock `amount` of `token` (address(0) = native) and send it to `recipient` on `dstChain`.
    function sendCrossChainMessage(
        uint64 dstChain,
        address token,
        uint256 amount,
        bytes32 recipient,
        bytes calldata payload
    ) external payable whenNotPaused nonReentrant returns (bytes32 payloadHash) {
        require(recipient != bytes32(0), "Interlink: zero recipient");
        uint256 received = _collect(token, amount);
        payloadHash = _publish(
            dstChain, ACTION_TRANSFER, _toBytes32(token), received, recipient, keccak256(payload), payload
        );
    }

    /// @notice Lock `amountIn` and request a swap into `tokenOut` on `dstChain`.
    /// @param tokenOut     Destination-chain token (bytes32 so Solana mints fit).
    /// @param minAmountOut Minimum output in destination token units (≤ uint64 max for Solana).
    function initiateSwap(
        uint64 dstChain,
        address tokenIn,
        uint256 amountIn,
        bytes32 tokenOut,
        uint256 minAmountOut,
        bytes32 recipient,
        bytes calldata swapData
    ) external payable whenNotPaused nonReentrant returns (bytes32 payloadHash) {
        require(recipient != bytes32(0), "Interlink: zero recipient");
        require(tokenOut != bytes32(0), "Interlink: zero tokenOut");
        require(minAmountOut <= type(uint64).max, "Interlink: minAmountOut too large");
        uint256 received = _collect(tokenIn, amountIn);
        bytes32 extraHash = keccak256(abi.encode(tokenOut, minAmountOut, keccak256(swapData)));
        payloadHash = _publish(
            dstChain,
            ACTION_SWAP,
            _toBytes32(tokenIn),
            received,
            recipient,
            extraHash,
            abi.encode(tokenOut, minAmountOut, swapData)
        );
    }

    /// @notice Lock an NFT for bridging. Disabled by default (`nftBridgingEnabled`):
    /// no destination mints or returns NFTs yet, so a locked NFT would be stranded.
    function lockNFT(
        address nftContract,
        uint256 tokenId,
        uint64 dstChain,
        bytes32 recipient
    ) external whenNotPaused nonReentrant returns (bytes32 payloadHash) {
        require(nftBridgingEnabled, "Interlink: NFT bridging disabled");
        require(nftContract.code.length > 0, "Interlink: zero NFT contract");
        require(recipient != bytes32(0), "Interlink: zero recipient");

        IERC721(nftContract).transferFrom(msg.sender, address(this), tokenId);
        require(IERC721(nftContract).ownerOf(tokenId) == address(this), "Interlink: NFT not received");
        isCustodiedAsset[nftContract] = true;

        payloadHash = _publish(
            dstChain,
            ACTION_NFT,
            _toBytes32(nftContract),
            1,
            recipient,
            keccak256(abi.encode(tokenId)),
            abi.encode(tokenId)
        );
    }

    // ─── Inbound ────────────────────────────────────────────────────────────

    /// @notice Release locked tokens for a proven inbound transfer.
    function releaseTokens(InboundMessage calldata m, bytes calldata proof)
        external
        whenNotPaused
        nonReentrant
    {
        bytes32 h = _consume(ACTION_TRANSFER, m, proof);
        address token = _toAddress(m.asset);
        address to = _toAddress(m.recipient);
        require(to != address(0), "Interlink: zero recipient");

        // Reverts on underflow: the gateway never releases more than it locked.
        totalLocked[token] -= m.amount;
        _send(token, to, m.amount);

        emit MessageExecuted(h, m.srcChain, m.nonce);
    }

    /// @notice Execute a proven generic call. The target may not be this
    /// gateway or any asset it custodies: a verified message must never be
    /// able to move custody (the Poly Network failure mode).
    function executeVerifiedMessage(
        InboundMessage calldata m,
        bytes calldata payload,
        bytes calldata proof
    ) external whenNotPaused nonReentrant {
        require(m.asset == bytes32(0) && m.amount == 0, "Interlink: calls carry no value");
        require(m.extraHash == keccak256(payload), "Interlink: payload mismatch");
        address target = _toAddress(m.recipient);
        require(target.code.length > 0, "Interlink: target has no code");
        require(target != address(this), "Interlink: cannot target gateway");
        require(!isCustodiedAsset[target], "Interlink: cannot target custodied asset");

        bytes32 h = _consume(ACTION_CALL, m, proof);

        (bool success,) = target.call(payload);
        require(success, "Interlink: Execution failed");

        emit MessageExecuted(h, m.srcChain, m.nonce);
    }

    // ─── Hashing ────────────────────────────────────────────────────────────

    function hashMessage(Message memory m) public pure returns (bytes32) {
        return keccak256(abi.encode(MESSAGE_TYPEHASH, m));
    }

    /// @notice Circuit public input for a payload hash and nonce.
    function commitment(bytes32 payloadHash, uint64 nonce) public pure returns (uint256) {
        uint256 w = addmod(uint256(payloadHash) % R, ROUND_CONSTANT, R);
        uint256 w2 = mulmod(w, w, R);
        uint256 w4 = mulmod(w2, w2, R);
        return addmod(mulmod(w4, w, R), uint256(nonce), R);
    }

    // ─── Internals ──────────────────────────────────────────────────────────

    function _publish(
        uint64 dstChain,
        uint8 action,
        bytes32 asset,
        uint256 amount,
        bytes32 recipient,
        bytes32 extraHash,
        bytes memory data
    ) internal returns (bytes32 h) {
        bytes32 dstGateway = remoteGateways[dstChain];
        require(dstGateway != bytes32(0), "Interlink: unsupported destination");

        uint64 nonce = currentNonce++;
        bytes32 sender = _toBytes32(msg.sender);
        h = hashMessage(
            Message({
                srcChain: uint64(block.chainid),
                srcGateway: _toBytes32(address(this)),
                dstChain: dstChain,
                dstGateway: dstGateway,
                nonce: nonce,
                action: action,
                sender: sender,
                asset: asset,
                amount: amount,
                recipient: recipient,
                extraHash: extraHash
            })
        );
        emit MessagePublished(
            h, nonce, dstChain, dstGateway, action, sender, asset, amount, recipient, extraHash, data
        );
    }

    /// Verify and mark an inbound message as executed. Returns its payload hash.
    function _consume(uint8 action, InboundMessage calldata m, bytes calldata proof)
        internal
        returns (bytes32 h)
    {
        bytes32 srcGateway = remoteGateways[m.srcChain];
        require(srcGateway != bytes32(0), "Interlink: unknown source");
        require(vkInitialized, "Interlink: VK not initialized");

        h = hashMessage(
            Message({
                srcChain: m.srcChain,
                srcGateway: srcGateway,
                dstChain: uint64(block.chainid),
                dstGateway: _toBytes32(address(this)),
                nonce: m.nonce,
                action: action,
                sender: m.sender,
                asset: m.asset,
                amount: m.amount,
                recipient: m.recipient,
                extraHash: m.extraHash
            })
        );
        require(!executedMessages[h], "Interlink: Message already executed");
        require(_verifyGroth16(proof, commitment(h, m.nonce)), "Interlink: Invalid Groth16 proof");
        executedMessages[h] = true;
    }

    /// Pull funds in and return the amount actually received (fee-on-transfer safe).
    function _collect(address token, uint256 amount) internal returns (uint256 received) {
        require(amount > 0, "Interlink: zero amount");
        if (token == address(0)) {
            require(msg.value == amount, "Interlink: Incorrect native value sent");
            received = amount;
        } else {
            require(msg.value == 0, "Interlink: unexpected native value");
            require(token.code.length > 0, "Interlink: token has no code");
            uint256 before = IERC20Balance(token).balanceOf(address(this));
            _safeTransferFrom(token, msg.sender, address(this), amount);
            received = IERC20Balance(token).balanceOf(address(this)) - before;
            require(received > 0, "Interlink: nothing received");
            isCustodiedAsset[token] = true;
        }
        // Solana amounts are bounded; keep every message representable everywhere.
        require(received <= type(uint128).max, "Interlink: amount too large");
        totalLocked[token] += received;
    }

    function _send(address token, address to, uint256 amount) internal {
        if (token == address(0)) {
            (bool ok,) = to.call{value: amount}("");
            require(ok, "Interlink: ETH transfer failed");
        } else {
            _safeTransfer(token, to, amount);
        }
    }

    /// Accepts tokens that return nothing (USDT) as well as those returning bool.
    function _safeTransfer(address token, address to, uint256 amount) private {
        (bool ok, bytes memory data) = token.call(abi.encodeWithSelector(0xa9059cbb, to, amount));
        require(ok && (data.length == 0 || abi.decode(data, (bool))), "Interlink: token transfer failed");
    }

    function _safeTransferFrom(address token, address from, address to, uint256 amount) private {
        (bool ok, bytes memory data) =
            token.call(abi.encodeWithSelector(0x23b872dd, from, to, amount));
        require(ok && (data.length == 0 || abi.decode(data, (bool))), "Interlink: Transfer failed");
    }

    function _toBytes32(address a) internal pure returns (bytes32) {
        return bytes32(uint256(uint160(a)));
    }

    function _toAddress(bytes32 b) internal pure returns (address) {
        require(uint256(b) >> 160 == 0, "Interlink: not an EVM address");
        return address(uint160(uint256(b)));
    }

    function _decodeVk(bytes calldata vkData) internal pure returns (uint256[18] memory words) {
        require(vkData.length == 576, "Interlink: VK must be 576 bytes");
        words = abi.decode(vkData, (uint256[18]));
        for (uint256 i; i < 18; ++i) {
            require(words[i] < P, "Interlink: VK coordinate out of range");
        }
    }

    // ─── Groth16 ────────────────────────────────────────────────────────────

    /// e(−A, B) · e(α, β) · e(L, γ) · e(C, δ) = 1,  L = IC0 + input · IC1
    /// proof = A (x, y) | B (x_im, x_re, y_im, y_re) | C (x, y), 32-byte big-endian words.
    function _verifyGroth16(bytes calldata proof, uint256 input) internal view returns (bool) {
        if (proof.length != 256 || input >= R) return false;
        uint256[8] memory p = abi.decode(proof, (uint256[8]));
        for (uint256 i; i < 8; ++i) {
            if (p[i] >= P) return false;
        }
        uint256[18] memory vk = _vk;

        uint256[3] memory mulIn = [vk[16], vk[17], input];
        uint256[2] memory scaled;
        bool ok;
        assembly {
            ok := staticcall(gas(), 0x07, mulIn, 0x60, scaled, 0x40)
        }
        if (!ok) return false;

        uint256[4] memory addIn = [vk[14], vk[15], scaled[0], scaled[1]];
        uint256[2] memory l;
        assembly {
            ok := staticcall(gas(), 0x06, addIn, 0x80, l, 0x40)
        }
        if (!ok) return false;

        uint256[24] memory input24 = [
            p[0], p[1] == 0 ? 0 : P - p[1], p[2], p[3], p[4], p[5],
            vk[0], vk[1], vk[2], vk[3], vk[4], vk[5],
            l[0], l[1], vk[6], vk[7], vk[8], vk[9],
            p[6], p[7], vk[10], vk[11], vk[12], vk[13]
        ];
        uint256[1] memory out;
        assembly {
            ok := staticcall(gas(), 0x08, input24, 768, out, 0x20)
        }
        return ok && out[0] == 1;
    }
}
