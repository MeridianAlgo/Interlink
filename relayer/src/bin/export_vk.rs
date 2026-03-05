//! Export the Groth16 verification key as hex for on-chain deployment.
//!
//! Usage:
//!   cargo run --bin export-vk
//!
//! Outputs:
//!   - VK hex string (for EVM setVerificationKey / VK_HEX forge env var)
//!   - VK as JSON array of bytes (for Solana set_verification_key)
//!
//! The VK is 576 bytes and is deterministic for a given circuit.
//! In production, replace with keys from a trusted setup ceremony.

use relayer::prover::ProverEngine;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = ProverEngine::new(0);
    eprintln!("Running Groth16 trusted setup (this takes a few seconds)...");
    engine.initialize().await?;

    let vk_bytes = engine.export_vk().await?;
    assert_eq!(vk_bytes.len(), 576, "VK must be 576 bytes");

    // Hex (for EVM + forge script VK_HEX env var)
    let hex: String = vk_bytes.iter().map(|b| format!("{:02x}", b)).collect();
    println!("VK_HEX=0x{}", hex);

    // JSON byte array (for Solana anchor script)
    let json: Vec<u8> = vk_bytes.clone();
    let json_str = serde_json::to_string(&json)?;
    println!("VK_JSON={}", json_str);

    eprintln!("\nDone. VK is {} bytes.", vk_bytes.len());
    eprintln!("Use VK_HEX with `forge script script/DeployAndInit.s.sol`");
    eprintln!("Use VK_JSON with the Solana `set_verification_key` instruction");

    Ok(())
}
