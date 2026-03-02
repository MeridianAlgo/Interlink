use interlink_core::relayer::{Relayer, RelayerConfig};

#[tokio::main]
async fn main() {
    // Setup tracing for logging and observability
    tracing_subscriber::fmt::init();

    let config = RelayerConfig {
        chain_id: 1, // Defaulting to Ethereum Mainnet ID
        rpc_url: "ws://localhost:8545".to_string(),
        hub_url: "https://api.devnet.solana.com".to_string(),
        gateway_address: "0x0000000000000000000000000000000000000000".to_string(),
        solana_program_id: "Hub1111111111111111111111111111111111111111".to_string(),
        keypair_path: "~/.config/solana/id.json".to_string(),
    };

    let relayer = Relayer::new(config);

    println!("--- InterLink Relayer Node ---");
    if let Err(e) = relayer.run().await {
        eprintln!("Relayer crashed with error: {:?}", e);
    }
}
