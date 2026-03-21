/// Wrapped asset standard for cross-chain canonical tokens
///
/// Manages the mapping of native assets (ETH, SOL, MATIC…) to their canonical
/// wrapped representations on each destination chain. On arrival the relayer
/// resolves the wrapped address and the gateway mints / releases the correct
/// token without user intervention.
///
/// Comparison:
///   - Stargate: native OFT wrapper per-chain, manual address management
///   - Wormhole: attested tokens per source chain, separate attestation tx required
///   - InterLink: deterministic canonical registry, automatic unwrap on destination
use std::collections::HashMap;

/// Chain identifier (matches ChainId in the SDK / fee.rs)
pub type ChainId = u32;

// ─── Well-known chain IDs ─────────────────────────────────────────────────────
pub const ETHEREUM: ChainId = 1;
pub const OPTIMISM: ChainId = 10;
pub const POLYGON: ChainId = 137;
pub const ARBITRUM_ONE: ChainId = 42161;
pub const BASE: ChainId = 8453;
pub const SOLANA: ChainId = 900;

// ─── Asset Descriptor ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AssetId {
    pub chain: ChainId,
    /// Hex address (EVM) or base58 mint address (Solana); "native" for gas tokens
    pub address: String,
}

impl AssetId {
    pub fn new(chain: ChainId, address: impl Into<String>) -> Self {
        AssetId {
            chain,
            address: address.into(),
        }
    }

    pub fn native(chain: ChainId) -> Self {
        AssetId {
            chain,
            address: "native".to_string(),
        }
    }

    pub fn is_native(&self) -> bool {
        self.address == "native"
    }
}

#[derive(Debug, Clone)]
pub struct AssetMeta {
    pub symbol: String,
    pub decimals: u8,
    pub name: String,
}

impl AssetMeta {
    pub fn new(symbol: impl Into<String>, decimals: u8, name: impl Into<String>) -> Self {
        AssetMeta {
            symbol: symbol.into(),
            decimals,
            name: name.into(),
        }
    }
}

// ─── Wrapped Registry ────────────────────────────────────────────────────────

/// Maps a canonical (source_chain, source_address) pair to its wrapped
/// equivalent on every supported destination chain.
pub struct WrappedRegistry {
    /// canonical_id → (dest_chain → wrapped_address)
    mappings: HashMap<AssetId, HashMap<ChainId, AssetId>>,
    /// address → metadata
    meta: HashMap<AssetId, AssetMeta>,
}

impl WrappedRegistry {
    pub fn new() -> Self {
        let mut reg = WrappedRegistry {
            mappings: HashMap::new(),
            meta: HashMap::new(),
        };
        reg.seed_defaults();
        reg
    }

    /// Register a canonical asset and its wrapped counterpart on a destination.
    pub fn register(
        &mut self,
        canonical: AssetId,
        canonical_meta: AssetMeta,
        dest_chain: ChainId,
        wrapped: AssetId,
        wrapped_meta: AssetMeta,
    ) {
        self.meta.insert(canonical.clone(), canonical_meta);
        self.meta.insert(wrapped.clone(), wrapped_meta);
        self.mappings
            .entry(canonical)
            .or_default()
            .insert(dest_chain, wrapped);
    }

    /// Look up the wrapped address for `asset` on `dest_chain`.
    ///
    /// Returns `Some(wrapped_id)` if a mapping exists, `None` if the asset is
    /// already native on that chain (i.e., canonical.chain == dest_chain), or
    /// `Err` if no mapping is registered.
    pub fn resolve(
        &self,
        canonical: &AssetId,
        dest_chain: ChainId,
    ) -> Result<Option<&AssetId>, WrappedError> {
        // Same chain: no wrapping needed
        if canonical.chain == dest_chain {
            return Ok(None);
        }
        let by_dest = self
            .mappings
            .get(canonical)
            .ok_or_else(|| WrappedError::NoMapping {
                asset: canonical.clone(),
            })?;
        let wrapped = by_dest
            .get(&dest_chain)
            .ok_or_else(|| WrappedError::NoDestinationChain {
                asset: canonical.clone(),
                dest: dest_chain,
            })?;
        Ok(Some(wrapped))
    }

    pub fn meta(&self, asset: &AssetId) -> Option<&AssetMeta> {
        self.meta.get(asset)
    }

    /// All destination chains for which a canonical asset has a wrapped token.
    pub fn supported_destinations(&self, canonical: &AssetId) -> Vec<ChainId> {
        self.mappings
            .get(canonical)
            .map(|m| m.keys().copied().collect())
            .unwrap_or_default()
    }

    /// Seed well-known wrapped pairs (ETH ↔ wETH, SOL ↔ wSOL, MATIC ↔ wMATIC).
    fn seed_defaults(&mut self) {
        // ── wETH: Ethereum native ETH → wrapped on L2s & Solana ──
        let eth = AssetId::native(ETHEREUM);
        let eth_meta = AssetMeta::new("ETH", 18, "Ether");

        let weth_optimism = AssetId::new(OPTIMISM, "0x4200000000000000000000000000000000000006");
        let weth_arbitrum =
            AssetId::new(ARBITRUM_ONE, "0x82aF49447D8a07e3bd95BD0d56f35241523fBab1");
        let weth_base = AssetId::new(BASE, "0x4200000000000000000000000000000000000006");
        let weth_polygon = AssetId::new(POLYGON, "0x7ceB23fD6bC0adD59E62ac25578270cFf1b9f619");
        let weth_solana = AssetId::new(SOLANA, "7vfCXTUXx5WJV5JADk17DUJ4ksgau7utNKj4b963voxs");

        for (dest, weth) in [
            (OPTIMISM, weth_optimism),
            (ARBITRUM_ONE, weth_arbitrum),
            (BASE, weth_base),
            (POLYGON, weth_polygon),
            (SOLANA, weth_solana),
        ] {
            self.register(
                eth.clone(),
                eth_meta.clone(),
                dest,
                weth,
                AssetMeta::new("wETH", 18, "Wrapped Ether"),
            );
        }

        // ── wSOL: Solana native SOL → wrapped on EVM chains ──
        let sol = AssetId::native(SOLANA);
        let sol_meta = AssetMeta::new("SOL", 9, "Solana");

        let wsol_ethereum = AssetId::new(ETHEREUM, "0xD31a59c85aE9D8edEFeC411D448f90841571b89c");
        let wsol_polygon = AssetId::new(POLYGON, "0x7DfF46370823dDGf2Eac3DeD2e");
        let wsol_arbitrum =
            AssetId::new(ARBITRUM_ONE, "0x2bcC6D6CdBbDC0a4071e48bb3B969b06B3330c07");

        for (dest, wsol) in [
            (ETHEREUM, wsol_ethereum),
            (POLYGON, wsol_polygon),
            (ARBITRUM_ONE, wsol_arbitrum),
        ] {
            self.register(
                sol.clone(),
                sol_meta.clone(),
                dest,
                wsol,
                AssetMeta::new("wSOL", 9, "Wrapped SOL"),
            );
        }

        // ── wMATIC: Polygon native MATIC → wrapped on Ethereum / L2s ──
        let matic = AssetId::native(POLYGON);
        let matic_meta = AssetMeta::new("MATIC", 18, "Polygon");

        let wmatic_ethereum = AssetId::new(ETHEREUM, "0x7D1AfA7B718fb893dB30A3aBc0Cfc608AaCfeBB0");
        let wmatic_arbitrum =
            AssetId::new(ARBITRUM_ONE, "0x561877b6b3DD7651313794e5F2894B2F18bE0766");

        for (dest, wmatic) in [(ETHEREUM, wmatic_ethereum), (ARBITRUM_ONE, wmatic_arbitrum)] {
            self.register(
                matic.clone(),
                matic_meta.clone(),
                dest,
                wmatic,
                AssetMeta::new("wMATIC", 18, "Wrapped MATIC"),
            );
        }
    }
}

impl Default for WrappedRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
pub enum WrappedError {
    NoMapping { asset: AssetId },
    NoDestinationChain { asset: AssetId, dest: ChainId },
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn reg() -> WrappedRegistry {
        WrappedRegistry::new()
    }

    #[test]
    fn test_eth_to_weth_optimism() {
        let r = reg();
        let eth = AssetId::native(ETHEREUM);
        let wrapped = r.resolve(&eth, OPTIMISM).unwrap().unwrap();
        assert_eq!(wrapped.chain, OPTIMISM);
        assert!(wrapped.address.starts_with("0x420"));
    }

    #[test]
    fn test_eth_to_weth_solana() {
        let r = reg();
        let eth = AssetId::native(ETHEREUM);
        let wrapped = r.resolve(&eth, SOLANA).unwrap().unwrap();
        assert_eq!(wrapped.chain, SOLANA);
    }

    #[test]
    fn test_same_chain_returns_none() {
        let r = reg();
        let eth = AssetId::native(ETHEREUM);
        let result = r.resolve(&eth, ETHEREUM).unwrap();
        assert!(result.is_none(), "same chain should return None (no wrap)");
    }

    #[test]
    fn test_unknown_asset_error() {
        let r = reg();
        let unknown = AssetId::new(ETHEREUM, "0xdeadbeef");
        let err = r.resolve(&unknown, SOLANA).unwrap_err();
        assert!(matches!(err, WrappedError::NoMapping { .. }));
    }

    #[test]
    fn test_unknown_dest_chain_error() {
        let r = reg();
        let eth = AssetId::native(ETHEREUM);
        // Chain 9999 is not registered
        let err = r.resolve(&eth, 9999).unwrap_err();
        assert!(matches!(err, WrappedError::NoDestinationChain { .. }));
    }

    #[test]
    fn test_sol_to_wsol_ethereum() {
        let r = reg();
        let sol = AssetId::native(SOLANA);
        let wrapped = r.resolve(&sol, ETHEREUM).unwrap().unwrap();
        assert_eq!(wrapped.chain, ETHEREUM);
    }

    #[test]
    fn test_meta_lookup() {
        let r = reg();
        let eth = AssetId::native(ETHEREUM);
        let meta = r.meta(&eth).unwrap();
        assert_eq!(meta.symbol, "ETH");
        assert_eq!(meta.decimals, 18);
    }

    #[test]
    fn test_supported_destinations_eth() {
        let r = reg();
        let eth = AssetId::native(ETHEREUM);
        let dests = r.supported_destinations(&eth);
        assert!(dests.contains(&OPTIMISM));
        assert!(dests.contains(&ARBITRUM_ONE));
        assert!(dests.contains(&SOLANA));
        assert!(dests.contains(&POLYGON));
        assert!(dests.contains(&BASE));
    }

    #[test]
    fn test_custom_registration() {
        let mut r = reg();
        let my_token = AssetId::new(ETHEREUM, "0xTOKEN");
        let my_wrapped = AssetId::new(POLYGON, "0xWTOKEN");
        r.register(
            my_token.clone(),
            AssetMeta::new("TKN", 18, "My Token"),
            POLYGON,
            my_wrapped.clone(),
            AssetMeta::new("wTKN", 18, "Wrapped My Token"),
        );
        let resolved = r.resolve(&my_token, POLYGON).unwrap().unwrap();
        assert_eq!(resolved.address, "0xWTOKEN");
    }

    #[test]
    fn test_asset_id_is_native() {
        let n = AssetId::native(ETHEREUM);
        assert!(n.is_native());
        let t = AssetId::new(ETHEREUM, "0xabc");
        assert!(!t.is_native());
    }
}
