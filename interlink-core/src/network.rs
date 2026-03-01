use crate::Result;
use ethers_providers::{Http, Middleware, Provider};
use std::convert::TryFrom;
use tracing::info;

/// simple client for talking to the chains.
pub struct NetworkClient {
    pub url: String,
}

impl NetworkClient {
    /// spin up a new network client.
    pub fn new(url: String) -> Self {
        Self { url }
    }

    /// try to connect and make sure the node is actually alive.
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

    /// grab a block by height. might be slow depending on the node.
    pub async fn get_block(&self, height: u64) -> Result<Vec<u8>> {
        info!("Fetching block at height {}...", height);

        let provider = Provider::<Http>::try_from(&self.url)
            .map_err(|e| crate::InterlinkError::NetworkError(e.to_string()))?;

        let block = provider
            .get_block(height)
            .await
            .map_err(|e| crate::InterlinkError::NetworkError(e.to_string()))?
            .ok_or_else(|| crate::InterlinkError::NetworkError("Block not found".to_string()))?;

        // serialize the block so we can shove it into the circuit witness.
        let serialized = serde_json::to_vec(&block)
            .map_err(|_| crate::InterlinkError::NetworkError("Serialization failed".to_string()))?;

        Ok(serialized)
    }
}
