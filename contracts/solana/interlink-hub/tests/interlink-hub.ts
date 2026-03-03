import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { InterlinkHub } from "../target/types/interlink_hub";
import { expect } from "chai";

describe("interlink-hub", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.interlinkHub as Program<InterlinkHub>;
  const admin = provider.wallet;

  let stateRegistryPda: anchor.web3.PublicKey;
  let stateRegistryBump: number;

  before(async () => {
    [stateRegistryPda, stateRegistryBump] =
      anchor.web3.PublicKey.findProgramAddressSync(
        [Buffer.from("state")],
        program.programId
      );
  });

  it("initializes the hub with fee rate", async () => {
    const feeRateBps = 10; // 0.1%

    const tx = await program.methods
      .initialize(feeRateBps)
      .accounts({
        stateRegistry: stateRegistryPda,
        admin: admin.publicKey,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .rpc();

    console.log("Initialize tx:", tx);

    // Verify state
    const state = await program.account.stateRegistry.fetch(stateRegistryPda);
    expect(state.admin.toString()).to.equal(admin.publicKey.toString());
    expect(state.feeRateBps).to.equal(feeRateBps);
    expect(state.processedSequences.toNumber()).to.equal(0);
    expect(state.totalStaked.toNumber()).to.equal(0);
    expect(state.totalBurned.toNumber()).to.equal(0);
  });

  it("rejects submit_proof with invalid proof length", async () => {
    const invalidProof = Buffer.alloc(100); // Wrong length (should be 256)
    const payloadHash = Buffer.alloc(32);
    const commitmentInput = Buffer.alloc(32);

    try {
      await program.methods
        .submitProof(
          new anchor.BN(1), // source_chain
          new anchor.BN(1), // sequence
          invalidProof,
          Array.from(payloadHash),
          Array.from(commitmentInput)
        )
        .accounts({
          stateRegistry: stateRegistryPda,
          relayer: admin.publicKey,
        })
        .rpc();
      expect.fail("should have thrown");
    } catch (err) {
      // Expected: InvalidProof error due to wrong proof length
      expect(err.toString()).to.include("InvalidProof");
    }
  });

  it("rejects duplicate sequence numbers", async () => {
    // This test verifies anti-replay protection.
    // Since we can't generate a valid BN254 proof in TypeScript easily,
    // we verify the sequence check happens before proof verification
    // by submitting sequence 0 (which is <= processed_sequences = 0).
    const proof = Buffer.alloc(256);
    const payloadHash = Buffer.alloc(32);
    const commitmentInput = Buffer.alloc(32);

    try {
      await program.methods
        .submitProof(
          new anchor.BN(1),
          new anchor.BN(0), // sequence 0 <= processed_sequences (0)
          proof,
          Array.from(payloadHash),
          Array.from(commitmentInput)
        )
        .accounts({
          stateRegistry: stateRegistryPda,
          relayer: admin.publicKey,
        })
        .rpc();
      expect.fail("should have thrown");
    } catch (err) {
      expect(err.toString()).to.include("SequenceAlreadyProcessed");
    }
  });
});
