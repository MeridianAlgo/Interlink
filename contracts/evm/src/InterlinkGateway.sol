// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

/**
 * @title InterlinkGateway
 * @dev Source chain gateway (spoke) for EVM chains.
 * Handles asset custody, event logging for relayers, and verified hub commands.
 *
 * Security:
 *  - CEI pattern in sendCrossChainMessage
 *  - Zero-address guards
 *  - emergencyWithdraw for untrapping funds
 *  - Standard Groth16 verification (4-pairing check)
 *  - Pinned to 0.8.28 for stability
 *
 * Groth16 verification uses stored verification key (VK) with the equation:
 *   e(-A, B) · e(α, β) · e(L, γ) · e(C, δ) = 1
 * where L = IC[0] + publicInput * IC[1]
 */

interface IERC20 {
    function transferFrom(address sender, address recipient, uint256 amount) external returns (bool);
    function transfer(address recipient, uint256 amount) external returns (bool);
}

interface IERC721 {
    function transferFrom(address from, address to, uint256 tokenId) external;
    function ownerOf(uint256 tokenId) external view returns (address);
}

contract InterlinkGateway {
    address public immutable daoGuardian;
    bool public paused;

    // Anti-replay. Don't execute a nonce more than once.
    mapping(uint64 => bool) public executedNonces;
    uint64 public currentNonce;

    // ─── Groth16 Verification Key ───────────────────────────────────────────
    // Stored on-chain after trusted setup. Set via setVerificationKey().
    bool public vkInitialized;

    // VK points (BN254, big-endian, EVM precompile format)
    uint256[2] public vk_alpha;     // G1: (x, y)
    uint256[2][2] public vk_beta;   // G2: (x_im, x_re, y_im, y_re) packed as [[x_im,x_re],[y_im,y_re]]
    uint256[2][2] public vk_gamma;  // G2
    uint256[2][2] public vk_delta;  // G2
    uint256[2] public vk_ic0;       // G1: IC base
    uint256[2] public vk_ic1;       // G1: IC for public input

    // BN254 scalar field modulus
    uint256 constant BN254_SCALAR_FIELD = 21888242871839275222246405745257275088548364400416034343698204186575808495617;
    // BN254 base field modulus (for point negation)
    uint256 constant BN254_BASE_FIELD = 21888242871839275222246405745257275088696311157297823662689037894645226208583;

    // ─── Events ─────────────────────────────────────────────────────────────

    event MessagePublished(
        uint64 indexed nonce,
        uint64 destinationChain,
        address sender,
        bytes32 payloadHash,
        bytes payload
    );

    event MessageExecuted(uint64 indexed nonce, bool success);

    event SwapInitiated(
        uint64 indexed nonce,
        address indexed sender,
        address recipient,
        uint256 amountIn,
        address tokenIn,
        address tokenOut,
        uint256 minAmountOut,
        uint64 destinationChain,
        bytes swapData,
        bytes32 payloadHash
    );

    event NFTLocked(
        uint64 indexed nonce,
        address indexed sender,
        address nftContract,
        uint256 tokenId,
        uint64 destinationChain,
        bytes32 destinationRecipient,
        bytes32 nftHash
    );

    event GatewayPaused();
    event GatewayUnpaused();
    event EmergencyWithdraw(address indexed token, address indexed to, uint256 amount);
    event VerificationKeyUpdated();

    // ─── Modifiers ──────────────────────────────────────────────────────────

    modifier onlyGuardian() {
        require(msg.sender == daoGuardian, "Interlink: Unauthorized");
        _;
    }

    modifier whenNotPaused() {
        require(!paused, "Interlink: Gateway is paused");
        _;
    }

    constructor(address _guardian) {
        require(_guardian != address(0), "Interlink: zero guardian");
        daoGuardian = _guardian;
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

    function emergencyWithdraw(address token, address to, uint256 amount) external onlyGuardian {
        require(to != address(0), "Interlink: zero recipient");
        if (token == address(0)) {
            (bool ok,) = to.call{value: amount}("");
            require(ok, "Interlink: ETH transfer failed");
        } else {
            require(IERC20(token).transfer(to, amount), "Interlink: token transfer failed");
        }
        emit EmergencyWithdraw(token, to, amount);
    }

    /**
     * @dev Set the Groth16 verification key. Must be called once after trusted
     * setup before any proofs can be verified. Guardian-only.
     *
     * @param alpha   G1 point (2 uint256: x, y)
     * @param beta    G2 point (2x2 uint256: [x_im,x_re], [y_im,y_re])
     * @param gamma   G2 point
     * @param delta   G2 point
     * @param ic0     G1 point — IC base
     * @param ic1     G1 point — IC for public input
     */
    function setVerificationKey(
        uint256[2] calldata alpha,
        uint256[2][2] calldata beta,
        uint256[2][2] calldata gamma,
        uint256[2][2] calldata delta,
        uint256[2] calldata ic0,
        uint256[2] calldata ic1
    ) external onlyGuardian {
        vk_alpha = alpha;
        vk_beta = beta;
        vk_gamma = gamma;
        vk_delta = delta;
        vk_ic0 = ic0;
        vk_ic1 = ic1;
        vkInitialized = true;
        emit VerificationKeyUpdated();
    }

    // ─── User-facing ────────────────────────────────────────────────────────

    function sendCrossChainMessage(
        uint64 destChain,
        address token,
        uint256 amount,
        bytes calldata payload
    ) external payable whenNotPaused {
        if (token == address(0)) {
            require(msg.value == amount, "Interlink: Incorrect native value sent");
        }

        uint64 nonce = currentNonce++;
        bytes32 payloadHash = keccak256(abi.encode(msg.sender, destChain, token, amount, payload));

        emit MessagePublished(nonce, destChain, msg.sender, payloadHash, payload);

        if (token != address(0)) {
            require(IERC20(token).transferFrom(msg.sender, address(this), amount), "Interlink: Transfer failed");
        }
    }

    function initiateSwap(
        uint64 destChain,
        address tokenIn,
        uint256 amountIn,
        address tokenOut,
        uint256 minAmountOut,
        address recipient,
        bytes calldata swapData
    ) external payable whenNotPaused {
        require(amountIn > 0, "Interlink: zero amount");
        require(recipient != address(0), "Interlink: zero recipient");

        if (tokenIn == address(0)) {
            require(msg.value == amountIn, "Interlink: Incorrect native value sent");
        }

        uint64 nonce = currentNonce++;
        bytes32 payloadHash = keccak256(abi.encode(
            msg.sender, destChain, tokenIn, amountIn, tokenOut, minAmountOut, recipient, swapData
        ));

        emit SwapInitiated(
            nonce, msg.sender, recipient, amountIn,
            tokenIn, tokenOut, minAmountOut, destChain, swapData, payloadHash
        );

        if (tokenIn != address(0)) {
            require(IERC20(tokenIn).transferFrom(msg.sender, address(this), amountIn), "Interlink: Transfer failed");
        }
    }

    function lockNFT(
        address nftContract,
        uint256 tokenId,
        uint64 destinationChain,
        bytes32 destinationRecipient
    ) external whenNotPaused {
        require(nftContract != address(0), "Interlink: zero NFT contract");
        require(destinationRecipient != bytes32(0), "Interlink: zero recipient");

        uint64 nonce = currentNonce++;
        bytes32 nftHash = keccak256(abi.encode(nftContract, tokenId, msg.sender, destinationChain));

        emit NFTLocked(
            nonce, msg.sender, nftContract, tokenId,
            destinationChain, destinationRecipient, nftHash
        );

        IERC721(nftContract).transferFrom(msg.sender, address(this), tokenId);
    }

    // ─── Relayer-facing ─────────────────────────────────────────────────────

    /**
     * @dev Execute a verified message from the hub. Uses standard Groth16
     * verification with stored VK.
     *
     * @param target     Destination contract for the payload.
     * @param nonce      Origin sequence ID — prevents replay.
     * @param payload    ABI-encoded call data.
     * @param snarkProof 256-byte Groth16 proof: A(64) + B(128) + C(64).
     */
    function executeVerifiedMessage(
        address target,
        uint64 nonce,
        bytes calldata payload,
        bytes calldata snarkProof
    ) external whenNotPaused {
        require(target != address(0), "Interlink: zero target");
        require(!executedNonces[nonce], "Interlink: Message already executed");
        require(vkInitialized, "Interlink: VK not initialized");

        // Derive public input: keccak256(target, nonce, payload) domain-separated
        bytes32 publicInputHash = keccak256(abi.encodePacked(target, nonce, payload));
        uint256 publicInput = uint256(
            keccak256(abi.encodePacked(publicInputHash, "interlink_v1_domain"))
        ) % BN254_SCALAR_FIELD;

        bool valid = _verifyGroth16(snarkProof, publicInput);
        require(valid, "Interlink: Invalid Groth16 proof");

        executedNonces[nonce] = true;

        (bool success,) = target.call(payload);
        require(success, "Interlink: Execution failed");

        emit MessageExecuted(nonce, success);
    }

    // ─── Groth16 Verification ───────────────────────────────────────────────

    /**
     * @dev Standard Groth16 verification using BN254 precompiles.
     *
     * Verification equation (4-pairing multi-check):
     *   e(-A, B) · e(α, β) · e(L, γ) · e(C, δ) = 1
     *
     * Where L = IC[0] + publicInput * IC[1]
     *
     * @param proof        256-byte proof: A(64) + B(128) + C(64)
     * @param publicInput  Scalar field element (the commitment)
     */
    function _verifyGroth16(bytes calldata proof, uint256 publicInput) internal view returns (bool) {
        if (proof.length != 256) return false;
        require(publicInput < BN254_SCALAR_FIELD, "Interlink: input >= scalar field");

        // Decode proof points
        (uint256 ax, uint256 ay) = abi.decode(proof[0:64], (uint256, uint256));
        (uint256 bx1, uint256 bx2, uint256 by1, uint256 by2) = abi.decode(proof[64:192], (uint256, uint256, uint256, uint256));
        (uint256 cx, uint256 cy) = abi.decode(proof[192:256], (uint256, uint256));

        // Compute L = IC[0] + publicInput * IC[1]
        // Step 1: ECMUL — publicInput * IC[1]
        uint256[3] memory mulInput;
        mulInput[0] = vk_ic1[0];
        mulInput[1] = vk_ic1[1];
        mulInput[2] = publicInput;
        uint256[2] memory scaledIC1;
        bool mulOk;
        assembly {
            mulOk := staticcall(gas(), 0x07, mulInput, 0x60, scaledIC1, 0x40)
        }
        require(mulOk, "Interlink: ECMUL failed");

        // Step 2: ECADD — IC[0] + scaledIC1
        uint256[4] memory addInput;
        addInput[0] = vk_ic0[0];
        addInput[1] = vk_ic0[1];
        addInput[2] = scaledIC1[0];
        addInput[3] = scaledIC1[1];
        uint256[2] memory L;
        bool addOk;
        assembly {
            addOk := staticcall(gas(), 0x06, addInput, 0x80, L, 0x40)
        }
        require(addOk, "Interlink: ECADD failed");

        // 4-pairing check: e(-A, B) · e(α, β) · e(L, γ) · e(C, δ) = 1
        // -A: negate y coordinate → (ax, BN254_BASE_FIELD - ay)
        uint256 neg_ay = (ay == 0) ? 0 : BN254_BASE_FIELD - ay;

        uint256[24] memory pairingInput;

        // Pair 0: (-A, B)
        pairingInput[0]  = ax;
        pairingInput[1]  = neg_ay;
        pairingInput[2]  = bx1;
        pairingInput[3]  = bx2;
        pairingInput[4]  = by1;
        pairingInput[5]  = by2;

        // Pair 1: (alpha, beta)
        pairingInput[6]  = vk_alpha[0];
        pairingInput[7]  = vk_alpha[1];
        pairingInput[8]  = vk_beta[0][0];
        pairingInput[9]  = vk_beta[0][1];
        pairingInput[10] = vk_beta[1][0];
        pairingInput[11] = vk_beta[1][1];

        // Pair 2: (L, gamma)
        pairingInput[12] = L[0];
        pairingInput[13] = L[1];
        pairingInput[14] = vk_gamma[0][0];
        pairingInput[15] = vk_gamma[0][1];
        pairingInput[16] = vk_gamma[1][0];
        pairingInput[17] = vk_gamma[1][1];

        // Pair 3: (C, delta)
        pairingInput[18] = cx;
        pairingInput[19] = cy;
        pairingInput[20] = vk_delta[0][0];
        pairingInput[21] = vk_delta[0][1];
        pairingInput[22] = vk_delta[1][0];
        pairingInput[23] = vk_delta[1][1];

        // Call ecPairing precompile (0x08): returns 1 if the pairing product == 1
        uint256[1] memory result;
        bool pairingOk;
        assembly {
            pairingOk := staticcall(gas(), 0x08, pairingInput, 768, result, 0x20)
        }

        return pairingOk && result[0] == 1;
    }

    /// @dev Accept plain ETH sends.
    receive() external payable {}
}
