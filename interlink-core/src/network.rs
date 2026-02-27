use crate::Result;
use ethers_providers::{Http, Middleware, Provider};
use std::convert::TryFrom;
use tracing::info;

/// A client for interacting with blockchain networks.
pub struct NetworkClient {
    pub url: String,
}

impl NetworkClient {
    /// Creates a new network client.
    pub fn new(url: String) -> Self {
        Self { url }
    }

    /// Connects to a remote node and validates the provider.
    pub async fn connect(&self) -> Result<()> {
        info!("Connecting to network at {}...", self.url);

        let provider = Provider::<Http>::try_from(&self.url)
            .map_err(|e| crate::InterlinkError::NetworkError(e.to_string()))?;

        let chain_id = provider
            .get_chainid()
            .await
            .map_err(|e| crate::InterlinkError::NetworkError(e.to_string()))?;

        info!("Connected successfully to Chain ID: {}", chain_id);
        Ok(())
    }

    /// Fetches a block by height using the provider.
    pub async fn get_block(&self, height: u64) -> Result<Vec<u8>> {
        info!("Fetching block at height {}...", height);

        let provider = Provider::<Http>::try_from(&self.url)
            .map_err(|e| crate::InterlinkError::NetworkError(e.to_string()))?;

        let block = provider
            .get_block(height)
            .await
            .map_err(|e| crate::InterlinkError::NetworkError(e.to_string()))?
            .ok_or_else(|| crate::InterlinkError::NetworkError("Block not found".to_string()))?;

        // Serialize block data for circuit witness
        let serialized = serde_json::to_vec(&block)
            .map_err(|_| crate::InterlinkError::NetworkError("Serialization failed".to_string()))?;

        Ok(serialized)
    }
}
