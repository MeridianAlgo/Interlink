use interlink_core::relayer::{Relayer, RelayerConfig};

#[tokio::main]
async fn main() -> Result<(), interlink_core::InterlinkError> {
    // setup logging so we're not flying blind.
    tracing_subscriber::fmt::init();

    // load config from environment or use reasonable dev defaults.
    let config = RelayerConfig {
        chain_id: std::env::var("CHAIN_ID")
            .unwrap_or_else(|_| "1".to_string())
            .parse()
            .unwrap_or(1),
        rpc_url: std::env::var("EVM_RPC_URL").unwrap_or_else(|_| "ws://localhost:8545".to_string()),
        hub_url: std::env::var("SOLANA_RPC_URL")
            .unwrap_or_else(|_| "https://api.devnet.solana.com".to_string()),
        gateway_address: std::env::var("GATEWAY_ADDRESS")
            .unwrap_or_else(|_| "0x0000000000000000000000000000000000000000".to_string()),
        solana_program_id: std::env::var("HUB_PROGRAM_ID")
            .unwrap_or_else(|_| "AKzpc9tvxfhLjj5AantKizK2YZgSjoyhLmYqRZy6b8Lz".to_string()),
        keypair_path: std::env::var("KEYPAIR_PATH")
            .unwrap_or_else(|_| "~/.config/solana/id.json".to_string()),
    };

    // spin up the relayer.
    let relayer = Relayer::new(config);

    // run until it either works or crashes hard.
    relayer.run().await?;

    Ok(())
}
