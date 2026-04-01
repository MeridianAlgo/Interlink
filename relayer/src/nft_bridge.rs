/// NFT cross-chain bridging for InterLink (Phase 8)
///
/// Lock-mint-burn model for cross-chain NFT transfers with full
/// metadata preservation (on-chain attributes + IPFS/Arweave URIs).
///
/// Flow:
///   1. Lock NFT on source chain → emit NFTLocked event
///   2. Relayer generates ZK proof of lock
///   3. Mint wrapped NFT on destination with identical metadata
///   4. On return: burn wrapped NFT, unlock original on source
///
/// Metadata handling:
///   - On-chain attributes (name, description, traits) are bridged in proof payload
///   - IPFS/Arweave URIs are preserved as-is (content-addressable, chain-agnostic)
///   - SVG on-chain art: base64-encoded in proof payload
///   - Royalties: EIP-2981 royalty info forwarded to destination
///
/// Comparison:
///   Holograph:  mint-burn with protocol-owned contracts, no ZK
///   NFTBridge:  lock-mint on 5 chains, no metadata preservation guarantee
///   Wormhole:   NFT portal (deprecated), limited metadata
///   InterLink:  ZK-verified lock-mint with full metadata + royalty preservation

use std::collections::HashMap;

// ─── Constants ──────────────────────────────────────────────────────────────

/// Maximum metadata payload size in bytes (64 KB).
pub const MAX_METADATA_BYTES: usize = 65_536;
/// Maximum number of traits/attributes per NFT.
pub const MAX_ATTRIBUTES: usize = 100;
/// Royalty basis point cap (50% = 5000 bps).
pub const MAX_ROYALTY_BPS: u32 = 5_000;
/// Lock timeout: if mint doesn't happen within this window, unlock (24 hours).
pub const LOCK_TIMEOUT_SECS: u64 = 24 * 3600;

// ─── Types ──────────────────────────────────────────────────────────────────

/// Unique identifier for an NFT across chains.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NftId {
    /// Source chain ID where the NFT originates.
    pub origin_chain: u32,
    /// Contract address on the origin chain.
    pub contract_address: String,
    /// Token ID within the contract.
    pub token_id: String,
}

impl NftId {
    pub fn new(origin_chain: u32, contract: impl Into<String>, token_id: impl Into<String>) -> Self {
        NftId {
            origin_chain,
            contract_address: contract.into(),
            token_id: token_id.into(),
        }
    }

    /// Canonical string key for indexing.
    pub fn canonical_key(&self) -> String {
        format!("{}:{}:{}", self.origin_chain, self.contract_address, self.token_id)
    }
}

/// NFT metadata preserved during bridging.
#[derive(Debug, Clone)]
pub struct NftMetadata {
    /// Token name (e.g., "Bored Ape #1234").
    pub name: String,
    /// Token description.
    pub description: String,
    /// Image URI (IPFS, Arweave, HTTPS, or data: URI for on-chain SVG).
    pub image_uri: String,
    /// Animation URI (optional, for multimedia NFTs).
    pub animation_uri: Option<String>,
    /// External link.
    pub external_url: Option<String>,
    /// Attributes / traits.
    pub attributes: Vec<NftAttribute>,
    /// EIP-2981 royalty recipient address.
    pub royalty_recipient: Option<String>,
    /// EIP-2981 royalty in basis points.
    pub royalty_bps: Option<u32>,
}

/// A single NFT attribute (trait_type → value).
#[derive(Debug, Clone)]
pub struct NftAttribute {
    pub trait_type: String,
    pub value: String,
    /// Optional: display type (number, date, boost_percentage, etc.).
    pub display_type: Option<String>,
}

/// NFT bridge transfer state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeState {
    /// NFT locked on source chain, awaiting proof generation.
    Locked,
    /// ZK proof generated, awaiting mint on destination.
    ProofGenerated,
    /// Wrapped NFT minted on destination chain.
    Minted,
    /// Transfer complete (locked on source, minted on destination).
    Complete,
    /// Return journey: wrapped burned, original unlocked.
    Returned,
    /// Lock expired, original returned to owner.
    Expired,
    /// Transfer failed.
    Failed { reason: String },
}

/// A tracked NFT bridge transfer.
#[derive(Debug, Clone)]
pub struct NftTransfer {
    /// Unique transfer ID.
    pub transfer_id: String,
    /// The NFT being transferred.
    pub nft_id: NftId,
    /// Full metadata snapshot at time of lock.
    pub metadata: NftMetadata,
    /// Source chain ID.
    pub source_chain: u32,
    /// Destination chain ID.
    pub dest_chain: u32,
    /// Owner address on source chain.
    pub sender: String,
    /// Recipient address on destination chain.
    pub receiver: String,
    /// Current state.
    pub state: BridgeState,
    /// Lock timestamp.
    pub locked_at: u64,
    /// Wrapped contract address on destination (set after mint).
    pub wrapped_contract: Option<String>,
    /// Wrapped token ID on destination (set after mint).
    pub wrapped_token_id: Option<String>,
}

// ─── Bridge Registry ────────────────────────────────────────────────────────

/// Tracks all NFT bridge operations.
pub struct NftBridgeRegistry {
    /// Active and historical transfers by transfer_id.
    transfers: HashMap<String, NftTransfer>,
    /// Index: NftId canonical key → transfer_id (latest).
    by_nft: HashMap<String, String>,
    /// Index: sender → [transfer_ids].
    by_sender: HashMap<String, Vec<String>>,
    /// Wrapped contract mappings: (origin_chain, contract) → (dest_chain, wrapped_contract).
    wrapped_contracts: HashMap<(u32, String), HashMap<u32, String>>,
    /// Transfer counter for ID generation.
    next_id: u64,
}

impl NftBridgeRegistry {
    pub fn new() -> Self {
        NftBridgeRegistry {
            transfers: HashMap::new(),
            by_nft: HashMap::new(),
            by_sender: HashMap::new(),
            wrapped_contracts: HashMap::new(),
            next_id: 0,
        }
    }

    /// Register a wrapped contract mapping.
    pub fn register_wrapped_contract(
        &mut self,
        origin_chain: u32,
        origin_contract: impl Into<String>,
        dest_chain: u32,
        wrapped_contract: impl Into<String>,
    ) {
        self.wrapped_contracts
            .entry((origin_chain, origin_contract.into()))
            .or_default()
            .insert(dest_chain, wrapped_contract.into());
    }

    /// Look up the wrapped contract for an NFT on a destination chain.
    pub fn get_wrapped_contract(
        &self,
        origin_chain: u32,
        origin_contract: &str,
        dest_chain: u32,
    ) -> Option<&String> {
        self.wrapped_contracts
            .get(&(origin_chain, origin_contract.to_string()))?
            .get(&dest_chain)
    }

    /// Initiate an NFT bridge: lock on source chain.
    pub fn lock_nft(
        &mut self,
        nft_id: NftId,
        metadata: NftMetadata,
        dest_chain: u32,
        sender: impl Into<String>,
        receiver: impl Into<String>,
        now: u64,
    ) -> Result<String, NftBridgeError> {
        // Validate metadata
        self.validate_metadata(&metadata)?;

        // Check NFT isn't already locked
        let key = nft_id.canonical_key();
        if let Some(existing_id) = self.by_nft.get(&key) {
            if let Some(existing) = self.transfers.get(existing_id) {
                match &existing.state {
                    BridgeState::Locked | BridgeState::ProofGenerated | BridgeState::Minted => {
                        return Err(NftBridgeError::AlreadyLocked);
                    }
                    _ => {} // Previous transfer completed/expired/failed, OK to re-lock
                }
            }
        }

        let transfer_id = format!("nft_tx_{}", self.next_id);
        self.next_id += 1;

        let sender = sender.into();
        let source_chain = nft_id.origin_chain;

        let transfer = NftTransfer {
            transfer_id: transfer_id.clone(),
            nft_id: nft_id.clone(),
            metadata,
            source_chain,
            dest_chain,
            sender: sender.clone(),
            receiver: receiver.into(),
            state: BridgeState::Locked,
            locked_at: now,
            wrapped_contract: None,
            wrapped_token_id: None,
        };

        self.by_nft.insert(key, transfer_id.clone());
        self.by_sender.entry(sender).or_default().push(transfer_id.clone());
        self.transfers.insert(transfer_id.clone(), transfer);

        Ok(transfer_id)
    }

    /// Mark proof as generated for a transfer.
    pub fn mark_proof_generated(&mut self, transfer_id: &str) -> Result<(), NftBridgeError> {
        let transfer = self.transfers.get_mut(transfer_id).ok_or(NftBridgeError::TransferNotFound)?;
        if transfer.state != BridgeState::Locked {
            return Err(NftBridgeError::InvalidStateTransition);
        }
        transfer.state = BridgeState::ProofGenerated;
        Ok(())
    }

    /// Mark wrapped NFT as minted on destination.
    pub fn mark_minted(
        &mut self,
        transfer_id: &str,
        wrapped_contract: impl Into<String>,
        wrapped_token_id: impl Into<String>,
    ) -> Result<(), NftBridgeError> {
        let transfer = self.transfers.get_mut(transfer_id).ok_or(NftBridgeError::TransferNotFound)?;
        if transfer.state != BridgeState::ProofGenerated {
            return Err(NftBridgeError::InvalidStateTransition);
        }
        transfer.wrapped_contract = Some(wrapped_contract.into());
        transfer.wrapped_token_id = Some(wrapped_token_id.into());
        transfer.state = BridgeState::Complete;
        Ok(())
    }

    /// Process a return: burn wrapped, unlock original.
    pub fn process_return(&mut self, transfer_id: &str) -> Result<(), NftBridgeError> {
        let transfer = self.transfers.get_mut(transfer_id).ok_or(NftBridgeError::TransferNotFound)?;
        if transfer.state != BridgeState::Complete {
            return Err(NftBridgeError::InvalidStateTransition);
        }
        transfer.state = BridgeState::Returned;
        Ok(())
    }

    /// Expire locks that have timed out.
    pub fn expire_stale_locks(&mut self, now: u64) -> Vec<String> {
        let mut expired = Vec::new();
        for transfer in self.transfers.values_mut() {
            if transfer.state == BridgeState::Locked
                && now.saturating_sub(transfer.locked_at) >= LOCK_TIMEOUT_SECS
            {
                transfer.state = BridgeState::Expired;
                expired.push(transfer.transfer_id.clone());
            }
        }
        expired
    }

    /// Get transfer by ID.
    pub fn get_transfer(&self, transfer_id: &str) -> Option<&NftTransfer> {
        self.transfers.get(transfer_id)
    }

    /// Get all transfers by sender.
    pub fn get_by_sender(&self, sender: &str) -> Vec<&NftTransfer> {
        self.by_sender
            .get(sender)
            .map(|ids| ids.iter().filter_map(|id| self.transfers.get(id)).collect())
            .unwrap_or_default()
    }

    /// Total tracked transfers.
    pub fn total_transfers(&self) -> usize {
        self.transfers.len()
    }

    /// Validate NFT metadata.
    fn validate_metadata(&self, metadata: &NftMetadata) -> Result<(), NftBridgeError> {
        if metadata.name.is_empty() {
            return Err(NftBridgeError::InvalidMetadata("name is empty".into()));
        }
        if metadata.attributes.len() > MAX_ATTRIBUTES {
            return Err(NftBridgeError::InvalidMetadata(
                format!("too many attributes: {} > {MAX_ATTRIBUTES}", metadata.attributes.len()),
            ));
        }
        if let Some(bps) = metadata.royalty_bps {
            if bps > MAX_ROYALTY_BPS {
                return Err(NftBridgeError::InvalidMetadata(
                    format!("royalty {bps}bps exceeds max {MAX_ROYALTY_BPS}bps"),
                ));
            }
        }
        // Estimate total metadata size
        let estimated_size = metadata.name.len()
            + metadata.description.len()
            + metadata.image_uri.len()
            + metadata.attributes.iter().map(|a| a.trait_type.len() + a.value.len()).sum::<usize>();
        if estimated_size > MAX_METADATA_BYTES {
            return Err(NftBridgeError::MetadataTooLarge {
                size: estimated_size,
                max: MAX_METADATA_BYTES,
            });
        }
        Ok(())
    }

    /// Stats as JSON.
    pub fn stats_json(&self) -> serde_json::Value {
        let locked = self.transfers.values().filter(|t| t.state == BridgeState::Locked).count();
        let complete = self.transfers.values().filter(|t| t.state == BridgeState::Complete).count();
        let returned = self.transfers.values().filter(|t| t.state == BridgeState::Returned).count();
        let expired = self.transfers.values().filter(|t| t.state == BridgeState::Expired).count();

        serde_json::json!({
            "total_transfers": self.transfers.len(),
            "currently_locked": locked,
            "complete": complete,
            "returned": returned,
            "expired": expired,
            "wrapped_contracts": self.wrapped_contracts.len(),
        })
    }
}

impl Default for NftBridgeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Errors ─────────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
pub enum NftBridgeError {
    AlreadyLocked,
    TransferNotFound,
    InvalidStateTransition,
    InvalidMetadata(String),
    MetadataTooLarge { size: usize, max: usize },
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_metadata() -> NftMetadata {
        NftMetadata {
            name: "CryptoPunk #1234".into(),
            description: "A rare punk".into(),
            image_uri: "ipfs://QmXyz123abc".into(),
            animation_uri: None,
            external_url: Some("https://cryptopunks.app/1234".into()),
            attributes: vec![
                NftAttribute { trait_type: "Background".into(), value: "Blue".into(), display_type: None },
                NftAttribute { trait_type: "Eyes".into(), value: "Laser".into(), display_type: None },
            ],
            royalty_recipient: Some("0xCreator".into()),
            royalty_bps: Some(500), // 5%
        }
    }

    fn sample_nft_id() -> NftId {
        NftId::new(1, "0xCryptoPunks", "1234")
    }

    #[test]
    fn test_lock_nft() {
        let mut reg = NftBridgeRegistry::new();
        let id = reg.lock_nft(sample_nft_id(), sample_metadata(), 900, "0xAlice", "SolBob", 1000).unwrap();
        assert!(id.starts_with("nft_tx_"));
        let transfer = reg.get_transfer(&id).unwrap();
        assert_eq!(transfer.state, BridgeState::Locked);
        assert_eq!(transfer.source_chain, 1);
        assert_eq!(transfer.dest_chain, 900);
    }

    #[test]
    fn test_double_lock_rejected() {
        let mut reg = NftBridgeRegistry::new();
        reg.lock_nft(sample_nft_id(), sample_metadata(), 900, "0xAlice", "SolBob", 1000).unwrap();
        let result = reg.lock_nft(sample_nft_id(), sample_metadata(), 900, "0xAlice", "SolBob", 1001);
        assert_eq!(result.unwrap_err(), NftBridgeError::AlreadyLocked);
    }

    #[test]
    fn test_full_bridge_lifecycle() {
        let mut reg = NftBridgeRegistry::new();
        let id = reg.lock_nft(sample_nft_id(), sample_metadata(), 900, "0xAlice", "SolBob", 1000).unwrap();

        reg.mark_proof_generated(&id).unwrap();
        assert_eq!(reg.get_transfer(&id).unwrap().state, BridgeState::ProofGenerated);

        reg.mark_minted(&id, "WrappedPunksSOL", "1234").unwrap();
        let t = reg.get_transfer(&id).unwrap();
        assert_eq!(t.state, BridgeState::Complete);
        assert_eq!(t.wrapped_contract.as_deref(), Some("WrappedPunksSOL"));
    }

    #[test]
    fn test_return_journey() {
        let mut reg = NftBridgeRegistry::new();
        let id = reg.lock_nft(sample_nft_id(), sample_metadata(), 900, "0xAlice", "SolBob", 1000).unwrap();
        reg.mark_proof_generated(&id).unwrap();
        reg.mark_minted(&id, "Wrapped", "1234").unwrap();
        reg.process_return(&id).unwrap();
        assert_eq!(reg.get_transfer(&id).unwrap().state, BridgeState::Returned);
    }

    #[test]
    fn test_invalid_state_transitions() {
        let mut reg = NftBridgeRegistry::new();
        let id = reg.lock_nft(sample_nft_id(), sample_metadata(), 900, "0xAlice", "SolBob", 1000).unwrap();
        // Can't mint before proof
        assert_eq!(reg.mark_minted(&id, "W", "1"), Err(NftBridgeError::InvalidStateTransition));
        // Can't return before complete
        assert_eq!(reg.process_return(&id), Err(NftBridgeError::InvalidStateTransition));
    }

    #[test]
    fn test_expire_stale_locks() {
        let mut reg = NftBridgeRegistry::new();
        let id = reg.lock_nft(sample_nft_id(), sample_metadata(), 900, "0xAlice", "SolBob", 1000).unwrap();
        // Not expired yet
        let expired = reg.expire_stale_locks(1000 + LOCK_TIMEOUT_SECS - 1);
        assert!(expired.is_empty());
        // Now expired
        let expired = reg.expire_stale_locks(1000 + LOCK_TIMEOUT_SECS);
        assert_eq!(expired, vec![id.clone()]);
        assert_eq!(reg.get_transfer(&id).unwrap().state, BridgeState::Expired);
    }

    #[test]
    fn test_relock_after_expiry() {
        let mut reg = NftBridgeRegistry::new();
        reg.lock_nft(sample_nft_id(), sample_metadata(), 900, "0xAlice", "SolBob", 1000).unwrap();
        reg.expire_stale_locks(1000 + LOCK_TIMEOUT_SECS);
        // Can re-lock after expiry
        let id2 = reg.lock_nft(sample_nft_id(), sample_metadata(), 900, "0xAlice", "SolBob", 200_000).unwrap();
        assert_eq!(reg.get_transfer(&id2).unwrap().state, BridgeState::Locked);
    }

    #[test]
    fn test_metadata_preserved() {
        let mut reg = NftBridgeRegistry::new();
        let meta = sample_metadata();
        let id = reg.lock_nft(sample_nft_id(), meta.clone(), 900, "0xAlice", "SolBob", 1000).unwrap();
        let t = reg.get_transfer(&id).unwrap();
        assert_eq!(t.metadata.name, "CryptoPunk #1234");
        assert_eq!(t.metadata.attributes.len(), 2);
        assert_eq!(t.metadata.royalty_bps, Some(500));
        assert_eq!(t.metadata.image_uri, "ipfs://QmXyz123abc");
    }

    #[test]
    fn test_invalid_metadata_empty_name() {
        let mut reg = NftBridgeRegistry::new();
        let mut meta = sample_metadata();
        meta.name = String::new();
        let result = reg.lock_nft(sample_nft_id(), meta, 900, "A", "B", 1000);
        assert!(matches!(result, Err(NftBridgeError::InvalidMetadata(_))));
    }

    #[test]
    fn test_invalid_metadata_royalty_too_high() {
        let mut reg = NftBridgeRegistry::new();
        let mut meta = sample_metadata();
        meta.royalty_bps = Some(6000); // 60% > max 50%
        let result = reg.lock_nft(sample_nft_id(), meta, 900, "A", "B", 1000);
        assert!(matches!(result, Err(NftBridgeError::InvalidMetadata(_))));
    }

    #[test]
    fn test_too_many_attributes() {
        let mut reg = NftBridgeRegistry::new();
        let mut meta = sample_metadata();
        meta.attributes = (0..MAX_ATTRIBUTES + 1)
            .map(|i| NftAttribute { trait_type: format!("t{i}"), value: "v".into(), display_type: None })
            .collect();
        let result = reg.lock_nft(sample_nft_id(), meta, 900, "A", "B", 1000);
        assert!(matches!(result, Err(NftBridgeError::InvalidMetadata(_))));
    }

    #[test]
    fn test_wrapped_contract_registry() {
        let mut reg = NftBridgeRegistry::new();
        reg.register_wrapped_contract(1, "0xPunks", 900, "WrappedPunksSOL");
        reg.register_wrapped_contract(1, "0xPunks", 10, "WrappedPunksOP");
        assert_eq!(reg.get_wrapped_contract(1, "0xPunks", 900).unwrap(), "WrappedPunksSOL");
        assert_eq!(reg.get_wrapped_contract(1, "0xPunks", 10).unwrap(), "WrappedPunksOP");
        assert!(reg.get_wrapped_contract(1, "0xPunks", 42161).is_none());
    }

    #[test]
    fn test_get_by_sender() {
        let mut reg = NftBridgeRegistry::new();
        reg.lock_nft(NftId::new(1, "0xA", "1"), sample_metadata(), 900, "0xAlice", "B", 1000).unwrap();
        reg.lock_nft(NftId::new(1, "0xA", "2"), sample_metadata(), 900, "0xAlice", "B", 1001).unwrap();
        reg.lock_nft(NftId::new(1, "0xB", "1"), sample_metadata(), 900, "0xBob", "C", 1002).unwrap();
        assert_eq!(reg.get_by_sender("0xAlice").len(), 2);
        assert_eq!(reg.get_by_sender("0xBob").len(), 1);
    }

    #[test]
    fn test_nft_id_canonical_key() {
        let id = NftId::new(1, "0xABC", "42");
        assert_eq!(id.canonical_key(), "1:0xABC:42");
    }

    #[test]
    fn test_stats_json() {
        let mut reg = NftBridgeRegistry::new();
        reg.lock_nft(sample_nft_id(), sample_metadata(), 900, "A", "B", 1000).unwrap();
        let j = reg.stats_json();
        assert_eq!(j["total_transfers"], 1);
        assert_eq!(j["currently_locked"], 1);
    }
}
