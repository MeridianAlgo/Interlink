use interlink_core::relayer::{Relayer, RelayerConfig};

#[tokio::main]
async fn main() -> Result<(), interlink_core::InterlinkError> {
    tracing_subscriber::fmt::init();
    
    let config = RelayerConfig {
        chain_id: 1,
        rpc_url: "http://localhost:8545".to_string(),
    };
    
    let relayer = Relayer::new(config);
    relayer.run().await?;
    
    Ok(())
}
