use interlink_core::relayer::{Relayer, RelayerConfig};

#[tokio::main]
async fn main() -> Result<(), interlink_core::InterlinkError> {
    // setup logging so we're not flying blind.
    tracing_subscriber::fmt::init();

    // todo: move these to a .env or config file. hardcoding is bad, mkay?
    let config = RelayerConfig {
        chain_id: 1,
        rpc_url: "ws://localhost:8545".to_string(),
        hub_url: "https://api.devnet.solana.com".to_string(),
        gateway_address: "0x0000000000000000000000000000000000000000".to_string(),
        solana_program_id: "Hub1111111111111111111111111111111111111111".to_string(),
        keypair_path: "~/.config/solana/id.json".to_string(),
    };

    // spin up the relayer.
    let relayer = Relayer::new(config);
    
    // run until it either works or crashes hard.
    relayer.run().await?;

    Ok(())
}
