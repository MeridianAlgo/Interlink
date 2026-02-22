use interlink_core::relayer::{Relayer, RelayerConfig};

#[tokio::main]
async fn main() {
    // Initialize tracing for logging
    tracing_subscriber::fmt::init();

    let config = RelayerConfig {
        chain_id: 1, // Ethereum Mainnet placeholder
        rpc_url: "https://eth-mainnet.g.alchemy.com/v2/your-api-key".to_string(),
    };

    let relayer = Relayer::new(config);

    println!("--- InterLink Relayer Node ---");
    if let Err(e) = relayer.run().await {
        eprintln!("Relayer crashed with error: {:?}", e);
    }
}
