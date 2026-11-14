import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { createMint } from "@solana/spl-token";
import { InterlinkHub } from "../target/types/interlink_hub";
import { expect } from "chai";

const BPF_UPGRADEABLE_LOADER = new anchor.web3.PublicKey(
  "BPFLoaderUpgradeab1e11111111111111111111111"
);

describe("interlink-hub", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.interlinkHub as Program<InterlinkHub>;
  const admin = provider.wallet as anchor.Wallet;

  const pda = (...seeds: Buffer[]) =>
    anchor.web3.PublicKey.findProgramAddressSync(seeds, program.programId)[0];

  const statePda = pda(Buffer.from("state"));
  const vkPda = pda(Buffer.from("vk"));
  const stakePda = pda(Buffer.from("stake"), admin.publicKey.toBuffer());
  const [programData] = anchor.web3.PublicKey.findProgramAddressSync(
    [program.programId.toBuffer()],
    BPF_UPGRADEABLE_LOADER
  );

  let ilinkMint: anchor.web3.PublicKey;

  const sampleMessage = () => ({
    srcChain: new anchor.BN(1),
    srcGateway: new Array(32).fill(0x11),
    nonce: new anchor.BN(42),
    sender: new Array(32).fill(0x22),
    asset: new Array(32).fill(0x33),
    amount: new anchor.BN("1000000000000000000"),
    recipient: admin.publicKey,
    extraHash: new Array(32).fill(0x55),
  });

  const expectError = async (p: Promise<unknown>, ...names: string[]) => {
    try {
      await p;
      expect.fail("should have thrown");
    } catch (err) {
      const msg = String(err);
      expect(
        names.some((n) => msg.includes(n)),
        `expected one of ${names.join(", ")}, got: ${msg}`
      ).to.be.true;
    }
  };

  before(async () => {
    ilinkMint = await createMint(
      provider.connection,
      admin.payer,
      admin.publicKey,
      null,
      6
    );
  });

  it("initializes the hub (upgrade authority only)", async () => {
    await program.methods
      .initialize(10)
      .accountsPartial({
        stateRegistry: statePda,
        ilinkMint,
        admin: admin.publicKey,
        program: program.programId,
        programData,
      })
      .rpc();

    const state = await program.account.stateRegistry.fetch(statePda);
    expect(state.admin.toBase58()).to.equal(admin.publicKey.toBase58());
    expect(state.ilinkMint.toBase58()).to.equal(ilinkMint.toBase58());
    expect(state.feeRateBps).to.equal(10);
    expect(state.vkInitialized).to.equal(false);
  });

  it("rejects submit_proof before the VK is set", async () => {
    const msg = sampleMessage();
    const payloadHash = new Array(32).fill(0);
    await expectError(
      program.methods
        .submitProof(payloadHash, 1, msg, Buffer.alloc(256))
        .accountsPartial({
          stateRegistry: statePda,
          stakeAccount: stakePda,
          verificationKey: vkPda,
          messageRecord: pda(Buffer.from("message"), Buffer.from(payloadHash)),
          relayer: admin.publicKey,
        })
        .rpc(),
      "AccountNotInitialized",
      "InsufficientStake",
      "VKNotInitialized"
    );
  });

  it("sets the initial VK exactly once", async () => {
    // Shape-only VK; real proofs are covered by the Rust groth16_end_to_end test.
    const vk = Buffer.alloc(576, 1);
    await program.methods
      .setVerificationKey(vk)
      .accountsPartial({
        stateRegistry: statePda,
        verificationKey: vkPda,
        admin: admin.publicKey,
      })
      .rpc();

    // A second direct set must fail: rotation goes through the timelock.
    await expectError(
      program.methods
        .setVerificationKey(vk)
        .accountsPartial({
          stateRegistry: statePda,
          verificationKey: vkPda,
          admin: admin.publicKey,
        })
        .rpc(),
      "already in use"
    );
  });

  it("refuses to apply a proposed VK before the timelock", async () => {
    const pendingPda = pda(Buffer.from("pending_vk"));
    await program.methods
      .proposeVerificationKey(Buffer.alloc(576, 2))
      .accountsPartial({
        stateRegistry: statePda,
        pendingVerificationKey: pendingPda,
        admin: admin.publicKey,
      })
      .rpc();

    await expectError(
      program.methods
        .applyVerificationKey()
        .accountsPartial({
          stateRegistry: statePda,
          pendingVerificationKey: pendingPda,
          verificationKey: vkPda,
          admin: admin.publicKey,
        })
        .rpc(),
      "TimelockNotElapsed"
    );
  });

  it("rejects staking with a token other than ILINK", async () => {
    const fakeMint = await createMint(
      provider.connection,
      admin.payer,
      admin.publicKey,
      null,
      6
    );
    await expectError(
      program.methods
        .stake(new anchor.BN("100000000000"))
        .accountsPartial({
          stateRegistry: statePda,
          stakeAccount: stakePda,
          stakeVault: pda(Buffer.from("stake_vault"), admin.publicKey.toBuffer()),
          ilinkMint: fakeMint,
          relayerTokenAccount: admin.publicKey, // never reached
          relayer: admin.publicKey,
        })
        .rpc(),
      "InvalidMint",
      "ConstraintAddress"
    );
  });
});
