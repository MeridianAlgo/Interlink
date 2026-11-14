//! Export the Groth16 verification key for on-chain deployment.
//!
//! Usage:
//!   PROVING_KEY_PATH=keys/interlink_pk.bin cargo run --bin export-vk
//!
//! Reads the proving key the relayer uses (creating it on first run), so the
//! uploaded VK always matches the relayer's proofs. Previously this binary ran
//! its own random setup, so its VK never matched a running relayer.
//!
//! Outputs:
//!   - VK hex (for EVM `setVerificationKey(bytes)` / VK_HEX forge env var)
//!   - VK as JSON byte array (for Solana `set_verification_key`)
//!
//! WARNING: a locally created key is a single-party trusted setup. Use keys
//! from a multi-party ceremony for anything holding real value.

use relayer::prover::ProverEngine;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::var("PROVING_KEY_PATH").unwrap_or_else(|_| "keys/interlink_pk.bin".into());
    let path = std::path::Path::new(&path);

    let engine = ProverEngine::new(0);
    if !path.exists() {
        eprintln!(
            "No proving key at {}; running a LOCAL single-party setup (dev only)...",
            path.display()
        );
    }
    engine.load_or_create(path, true).await?;

    let vk_bytes = engine.export_vk().await?;
    assert_eq!(vk_bytes.len(), 576, "VK must be 576 bytes");

    let hex: String = vk_bytes.iter().map(|b| format!("{:02x}", b)).collect();
    println!("VK_HEX=0x{}", hex);
    println!("VK_JSON={}", serde_json::to_string(&vk_bytes)?);

    eprintln!(
        "\nProving key: {} (keep it with the relayer; never commit it)",
        path.display()
    );
    eprintln!("Use VK_HEX with `forge script script/DeployAndInit.s.sol`");
    eprintln!("Use VK_JSON with the Solana `set_verification_key` instruction");
    Ok(())
}
