//! Validator threshold signature scheme for InterLink (Phase 4).
//!
//! Implements a t-of-n threshold signature scheme where `t` validators must
//! independently sign a proof submission before it is accepted by the Hub.
//!
//! # Initial configuration: 3-of-5
//!
//! Three out of five validators must co-sign any proof. This provides:
//!   - Byzantine fault tolerance for f = 1 validator failure (f < t/2 = 1.5)
//!   - Liveness with t=3 even if 2 validators go offline
//!
//! # Competitive comparison
//!   Wormhole:    19 guardians, 2/3 threshold (13-of-19)
//!   Stargate:    Multi-sig, 2-of-n (varies per chain)
//!   Across:      Optimistic + UMA oracle, no threshold sig
//!   InterLink:   3-of-5 initial, upgradeable via governance to 13-of-19
//!
//! # Future: upgrade path
//!   Phase 4:  3-of-5 (launch safety)
//!   Phase 9:  DAO vote to expand validator set (governance)
//!   Mainnet:  Target 13-of-19 (Wormhole parity) or 34-of-50 (higher security)
//!
//! # Signature scheme
//!
//! Validators sign the `proof_commitment`:
//!   commitment = keccak256(proof_bytes || sequence || source_chain_id || hub_program_id)
//!
//! Using Ed25519 signatures (matches Solana native key format).
//! Aggregated into a `MultiSigBundle` submitted alongside the ZK proof.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

/// Serde helper for serializing/deserializing [u8; 32] commitment as hex string.
mod hex_commitment {
    use super::*;

    pub fn serialize<S: Serializer>(bytes: &[u8; 32], s: S) -> Result<S::Ok, S::Error> {
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        s.serialize_str(&hex)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 32], D::Error> {
        let hex = String::deserialize(d)?;
        if hex.len() != 64 {
            return Err(serde::de::Error::custom("expected 64-char hex string for 32-byte commitment"));
        }
        let mut out = [0u8; 32];
        for i in 0..32 {
            out[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
                .map_err(|_| serde::de::Error::custom("invalid hex character"))?;
        }
        Ok(out)
    }
}

/// Serde helper for serializing/deserializing [u8; 64] as a hex string.
mod hex_sig {
    use super::*;

    pub fn serialize<S: Serializer>(bytes: &[u8; 64], s: S) -> Result<S::Ok, S::Error> {
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        s.serialize_str(&hex)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 64], D::Error> {
        let hex = String::deserialize(d)?;
        if hex.len() != 128 {
            return Err(serde::de::Error::custom("expected 128-char hex string for 64-byte signature"));
        }
        let mut out = [0u8; 64];
        for i in 0..64 {
            out[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
                .map_err(|_| serde::de::Error::custom("invalid hex character"))?;
        }
        Ok(out)
    }
}

// ─── Constants ────────────────────────────────────────────────────────────────

/// Default signing threshold (t): minimum signatures required.
pub const DEFAULT_THRESHOLD: usize = 3;
/// Default validator set size (n).
pub const DEFAULT_VALIDATOR_COUNT: usize = 5;
/// Maximum allowed validator set size.
pub const MAX_VALIDATORS: usize = 50;
/// Minimum signing threshold (must be > 50% of n to prevent 50/50 split).
pub const MIN_THRESHOLD_BPS_OF_N: u32 = 5_001; // >50%

/// Wormhole guardian threshold for comparison: 13-of-19.
pub const WORMHOLE_THRESHOLD: usize = 13;
pub const WORMHOLE_VALIDATOR_COUNT: usize = 19;

// ─── Types ────────────────────────────────────────────────────────────────────

/// A validator's public identity (Ed25519 public key, base58).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidatorId {
    pub pubkey_bytes: [u8; 32],
    pub index: usize,
    pub alias: String,
}

impl ValidatorId {
    pub fn new(pubkey_bytes: [u8; 32], index: usize, alias: impl Into<String>) -> Self {
        Self {
            pubkey_bytes,
            index,
            alias: alias.into(),
        }
    }
}

/// An individual validator's signature on a proof commitment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorSignature {
    /// Index of the signing validator in the set.
    pub validator_index: usize,
    /// Ed25519 signature bytes (64 bytes), stored as hex string for JSON compatibility.
    #[serde(with = "hex_sig")]
    pub signature: [u8; 64],
    /// Unix timestamp when the signature was produced.
    pub signed_at: u64,
}

/// A threshold multi-signature bundle: t-of-n signatures on a proof commitment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiSigBundle {
    /// The commitment that was signed: SHA-256(proof || sequence || chain_id || program_id).
    /// Stored as hex string for JSON compatibility.
    #[serde(with = "hex_commitment")]
    pub commitment: [u8; 32],
    /// Collected validator signatures (must be ≥ threshold to be valid).
    pub signatures: Vec<ValidatorSignature>,
    /// Total validator set size at time of signing.
    pub validator_count: usize,
    /// Required threshold.
    pub threshold: usize,
    /// Sequence number this bundle covers.
    pub sequence: u64,
    /// Source chain ID.
    pub source_chain_id: u64,
}

impl MultiSigBundle {
    /// Whether this bundle has enough signatures to meet the threshold.
    pub fn is_valid(&self) -> bool {
        self.signatures.len() >= self.threshold
    }

    /// Fraction of validators who signed (basis points of validator_count).
    pub fn participation_bps(&self) -> u32 {
        if self.validator_count == 0 {
            return 0;
        }
        (self.signatures.len() * 10_000 / self.validator_count) as u32
    }

    /// Indices of validators who signed.
    pub fn signers(&self) -> Vec<usize> {
        self.signatures.iter().map(|s| s.validator_index).collect()
    }
}

/// The validator set configuration for a given epoch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorSet {
    /// Ordered list of validators.
    pub validators: Vec<ValidatorId>,
    /// Signing threshold.
    pub threshold: usize,
    /// Epoch number (incremented on rotation).
    pub epoch: u64,
}

impl ValidatorSet {
    /// Create a new validator set with the given validators and threshold.
    ///
    /// Returns an error if the threshold is invalid.
    pub fn new(validators: Vec<ValidatorId>, threshold: usize) -> Result<Self, MultiSigError> {
        let n = validators.len();
        if n == 0 {
            return Err(MultiSigError::EmptyValidatorSet);
        }
        if n > MAX_VALIDATORS {
            return Err(MultiSigError::TooManyValidators { count: n, max: MAX_VALIDATORS });
        }
        if threshold == 0 {
            return Err(MultiSigError::InvalidThreshold { threshold, n });
        }
        if threshold > n {
            return Err(MultiSigError::InvalidThreshold { threshold, n });
        }
        // Enforce >50% threshold (prevents 50/50 deadlock)
        let threshold_bps = threshold * 10_000 / n;
        if threshold_bps < MIN_THRESHOLD_BPS_OF_N as usize {
            return Err(MultiSigError::ThresholdTooLow {
                threshold_bps: threshold_bps as u32,
                min_bps: MIN_THRESHOLD_BPS_OF_N,
            });
        }

        Ok(Self {
            validators,
            threshold,
            epoch: 0,
        })
    }

    /// Default 3-of-5 set with placeholder validator keys.
    pub fn default_3_of_5() -> Self {
        let validators: Vec<ValidatorId> = (0..5)
            .map(|i| {
                let mut key = [0u8; 32];
                key[0] = i as u8 + 1; // placeholder: validator i has key[0] = i+1
                ValidatorId::new(key, i, format!("validator-{i}"))
            })
            .collect();

        Self {
            validators,
            threshold: DEFAULT_THRESHOLD,
            epoch: 0,
        }
    }

    /// Get a validator by index.
    pub fn get(&self, index: usize) -> Option<&ValidatorId> {
        self.validators.get(index)
    }

    /// Number of validators.
    pub fn n(&self) -> usize {
        self.validators.len()
    }

    /// Byzantine fault tolerance: maximum faults tolerated while maintaining liveness.
    /// f < threshold (validators can still sign even with f offline).
    pub fn fault_tolerance(&self) -> usize {
        self.validators.len().saturating_sub(self.threshold)
    }
}

/// Errors from multi-sig operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultiSigError {
    EmptyValidatorSet,
    TooManyValidators { count: usize, max: usize },
    InvalidThreshold { threshold: usize, n: usize },
    ThresholdTooLow { threshold_bps: u32, min_bps: u32 },
    InsufficientSignatures { got: usize, need: usize },
    DuplicateValidator { index: usize },
    UnknownValidator { index: usize },
    InvalidSignatureLength { got: usize },
    CommitmentMismatch,
}

impl std::fmt::Display for MultiSigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MultiSigError::EmptyValidatorSet => write!(f, "validator set is empty"),
            MultiSigError::TooManyValidators { count, max } => {
                write!(f, "too many validators: {count} > {max}")
            }
            MultiSigError::InvalidThreshold { threshold, n } => {
                write!(f, "invalid threshold {threshold} for {n} validators")
            }
            MultiSigError::ThresholdTooLow { threshold_bps, min_bps } => {
                write!(
                    f,
                    "threshold {threshold_bps} bps below minimum {min_bps} bps (must be >50%)"
                )
            }
            MultiSigError::InsufficientSignatures { got, need } => {
                write!(f, "insufficient signatures: got {got}, need {need}")
            }
            MultiSigError::DuplicateValidator { index } => {
                write!(f, "duplicate validator signature at index {index}")
            }
            MultiSigError::UnknownValidator { index } => {
                write!(f, "unknown validator at index {index}")
            }
            MultiSigError::InvalidSignatureLength { got } => {
                write!(f, "invalid signature length: {got} (expected 64)")
            }
            MultiSigError::CommitmentMismatch => {
                write!(f, "bundle commitment does not match expected commitment")
            }
        }
    }
}

// ─── Commitment computation ────────────────────────────────────────────────────

/// Compute the proof commitment that validators sign.
///
/// commitment = SHA-256(proof_bytes || sequence.to_be_bytes() || chain_id.to_be_bytes() || program_id)
pub fn compute_commitment(
    proof_bytes: &[u8],
    sequence: u64,
    source_chain_id: u64,
    hub_program_id: &[u8; 32],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(proof_bytes);
    hasher.update(sequence.to_be_bytes());
    hasher.update(source_chain_id.to_be_bytes());
    hasher.update(hub_program_id);
    hasher.finalize().into()
}

// ─── Bundle construction ───────────────────────────────────────────────────────

/// Create an empty multi-sig bundle for collection.
pub fn create_bundle(
    proof_bytes: &[u8],
    sequence: u64,
    source_chain_id: u64,
    hub_program_id: &[u8; 32],
    validator_set: &ValidatorSet,
) -> MultiSigBundle {
    let commitment = compute_commitment(proof_bytes, sequence, source_chain_id, hub_program_id);
    MultiSigBundle {
        commitment,
        signatures: Vec::new(),
        validator_count: validator_set.n(),
        threshold: validator_set.threshold,
        sequence,
        source_chain_id,
    }
}

/// Add a validator signature to a bundle.
///
/// Returns an error if the validator index is unknown, signature is duplicate,
/// or the signature bytes are malformed.
pub fn add_signature(
    bundle: &mut MultiSigBundle,
    validator_index: usize,
    signature: [u8; 64],
    validator_set: &ValidatorSet,
    signed_at: u64,
) -> Result<(), MultiSigError> {
    // Validate validator index is in set
    if validator_index >= validator_set.n() {
        return Err(MultiSigError::UnknownValidator { index: validator_index });
    }

    // Check for duplicate
    if bundle
        .signatures
        .iter()
        .any(|s| s.validator_index == validator_index)
    {
        return Err(MultiSigError::DuplicateValidator { index: validator_index });
    }

    bundle.signatures.push(ValidatorSignature {
        validator_index,
        signature,
        signed_at,
    });

    Ok(())
}

/// Verify a completed multi-sig bundle is ready for submission.
///
/// Checks: threshold met, no duplicate validators, all validators in set.
/// Note: actual Ed25519 signature verification is done on-chain (Solana sysvar).
pub fn verify_bundle(
    bundle: &MultiSigBundle,
    validator_set: &ValidatorSet,
) -> Result<(), MultiSigError> {
    // Threshold check
    if bundle.signatures.len() < bundle.threshold {
        return Err(MultiSigError::InsufficientSignatures {
            got: bundle.signatures.len(),
            need: bundle.threshold,
        });
    }

    // No duplicate validators
    let mut seen = std::collections::HashSet::new();
    for sig in &bundle.signatures {
        if !seen.insert(sig.validator_index) {
            return Err(MultiSigError::DuplicateValidator {
                index: sig.validator_index,
            });
        }
        if sig.validator_index >= validator_set.n() {
            return Err(MultiSigError::UnknownValidator {
                index: sig.validator_index,
            });
        }
    }

    Ok(())
}

/// Serialise a bundle for on-chain submission.
///
/// Format: threshold (1 byte) || n_sigs (1 byte) || commitment (32 bytes)
///         || (validator_index (1 byte) || sig (64 bytes)) × n_sigs
pub fn serialize_bundle(bundle: &MultiSigBundle) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + 32 + bundle.signatures.len() * 65);
    out.push(bundle.threshold as u8);
    out.push(bundle.signatures.len() as u8);
    out.extend_from_slice(&bundle.commitment);
    for sig in &bundle.signatures {
        out.push(sig.validator_index as u8);
        out.extend_from_slice(&sig.signature);
    }
    out
}

/// Deserialise a multi-sig bundle from bytes.
pub fn deserialize_bundle(
    bytes: &[u8],
    validator_count: usize,
    sequence: u64,
    source_chain_id: u64,
) -> Result<MultiSigBundle, MultiSigError> {
    if bytes.len() < 34 {
        return Err(MultiSigError::InvalidSignatureLength { got: bytes.len() });
    }
    let threshold = bytes[0] as usize;
    let n_sigs = bytes[1] as usize;
    let mut commitment = [0u8; 32];
    commitment.copy_from_slice(&bytes[2..34]);

    let expected_len = 34 + n_sigs * 65;
    if bytes.len() < expected_len {
        return Err(MultiSigError::InvalidSignatureLength { got: bytes.len() });
    }

    let mut signatures = Vec::with_capacity(n_sigs);
    for i in 0..n_sigs {
        let offset = 34 + i * 65;
        let validator_index = bytes[offset] as usize;
        if validator_index >= validator_count {
            return Err(MultiSigError::UnknownValidator { index: validator_index });
        }
        let mut sig = [0u8; 64];
        sig.copy_from_slice(&bytes[offset + 1..offset + 65]);
        signatures.push(ValidatorSignature {
            validator_index,
            signature: sig,
            signed_at: 0,
        });
    }

    Ok(MultiSigBundle {
        commitment,
        signatures,
        validator_count,
        threshold,
        sequence,
        source_chain_id,
    })
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_sig(val: u8) -> [u8; 64] {
        let mut s = [0u8; 64];
        s[0] = val;
        s
    }

    fn dummy_program_id() -> [u8; 32] {
        let mut id = [0u8; 32];
        id[0] = 0xAB;
        id
    }

    #[test]
    fn test_default_3_of_5_set() {
        let set = ValidatorSet::default_3_of_5();
        assert_eq!(set.n(), 5);
        assert_eq!(set.threshold, 3);
        assert_eq!(set.fault_tolerance(), 2);
    }

    #[test]
    fn test_invalid_threshold_too_low() {
        let validators: Vec<ValidatorId> = (0..5)
            .map(|i| ValidatorId::new([i as u8; 32], i, format!("v{i}")))
            .collect();
        // 2-of-5 = 40% < 50% → should fail
        let err = ValidatorSet::new(validators, 2).unwrap_err();
        assert!(matches!(err, MultiSigError::ThresholdTooLow { .. }));
    }

    #[test]
    fn test_valid_3_of_5_threshold() {
        let validators: Vec<ValidatorId> = (0..5)
            .map(|i| ValidatorId::new([i as u8; 32], i, format!("v{i}")))
            .collect();
        // 3-of-5 = 60% > 50% → valid
        assert!(ValidatorSet::new(validators, 3).is_ok());
    }

    #[test]
    fn test_bundle_creation_and_signing() {
        let set = ValidatorSet::default_3_of_5();
        let proof = vec![0xAA; 256];
        let program_id = dummy_program_id();

        let mut bundle = create_bundle(&proof, 1, 1, &program_id, &set);
        assert!(!bundle.is_valid()); // 0 signatures

        add_signature(&mut bundle, 0, dummy_sig(1), &set, 1_000_000).unwrap();
        add_signature(&mut bundle, 1, dummy_sig(2), &set, 1_000_001).unwrap();
        add_signature(&mut bundle, 2, dummy_sig(3), &set, 1_000_002).unwrap();

        assert!(bundle.is_valid()); // 3 of 3 required
        assert_eq!(bundle.participation_bps(), 6_000); // 3/5 = 60%
        assert_eq!(bundle.signers(), vec![0, 1, 2]);
    }

    #[test]
    fn test_duplicate_validator_rejected() {
        let set = ValidatorSet::default_3_of_5();
        let proof = vec![0xAA; 256];
        let program_id = dummy_program_id();
        let mut bundle = create_bundle(&proof, 1, 1, &program_id, &set);

        add_signature(&mut bundle, 0, dummy_sig(1), &set, 0).unwrap();
        let err = add_signature(&mut bundle, 0, dummy_sig(2), &set, 0).unwrap_err();
        assert!(matches!(err, MultiSigError::DuplicateValidator { index: 0 }));
    }

    #[test]
    fn test_unknown_validator_rejected() {
        let set = ValidatorSet::default_3_of_5();
        let proof = vec![0xAA; 256];
        let program_id = dummy_program_id();
        let mut bundle = create_bundle(&proof, 1, 1, &program_id, &set);

        let err = add_signature(&mut bundle, 99, dummy_sig(1), &set, 0).unwrap_err();
        assert!(matches!(err, MultiSigError::UnknownValidator { index: 99 }));
    }

    #[test]
    fn test_verify_bundle_insufficient() {
        let set = ValidatorSet::default_3_of_5();
        let proof = vec![0xAA; 256];
        let program_id = dummy_program_id();
        let mut bundle = create_bundle(&proof, 1, 1, &program_id, &set);

        add_signature(&mut bundle, 0, dummy_sig(1), &set, 0).unwrap();
        add_signature(&mut bundle, 1, dummy_sig(2), &set, 0).unwrap();
        // Only 2 sigs, need 3
        let err = verify_bundle(&bundle, &set).unwrap_err();
        assert!(matches!(
            err,
            MultiSigError::InsufficientSignatures { got: 2, need: 3 }
        ));
    }

    #[test]
    fn test_verify_bundle_success() {
        let set = ValidatorSet::default_3_of_5();
        let proof = vec![0xAA; 256];
        let program_id = dummy_program_id();
        let mut bundle = create_bundle(&proof, 1, 1, &program_id, &set);

        for i in 0..3 {
            add_signature(&mut bundle, i, dummy_sig(i as u8 + 1), &set, 0).unwrap();
        }
        assert!(verify_bundle(&bundle, &set).is_ok());
    }

    #[test]
    fn test_serialize_deserialize_roundtrip() {
        let set = ValidatorSet::default_3_of_5();
        let proof = vec![0xAA; 256];
        let program_id = dummy_program_id();
        let mut bundle = create_bundle(&proof, 42, 1, &program_id, &set);

        for i in 0..3 {
            add_signature(&mut bundle, i, dummy_sig(i as u8 + 10), &set, 0).unwrap();
        }

        let bytes = serialize_bundle(&bundle);
        let restored = deserialize_bundle(&bytes, set.n(), 42, 1).unwrap();

        assert_eq!(restored.commitment, bundle.commitment);
        assert_eq!(restored.threshold, 3);
        assert_eq!(restored.signatures.len(), 3);
        assert_eq!(restored.sequence, 42);
    }

    #[test]
    fn test_commitment_deterministic() {
        let proof = vec![0xFF; 256];
        let program_id = dummy_program_id();
        let c1 = compute_commitment(&proof, 5, 1, &program_id);
        let c2 = compute_commitment(&proof, 5, 1, &program_id);
        assert_eq!(c1, c2);
    }

    #[test]
    fn test_commitment_differs_on_sequence() {
        let proof = vec![0xFF; 256];
        let program_id = dummy_program_id();
        let c1 = compute_commitment(&proof, 5, 1, &program_id);
        let c2 = compute_commitment(&proof, 6, 1, &program_id);
        assert_ne!(c1, c2);
    }

    #[test]
    fn test_wormhole_comparison() {
        // InterLink 3-of-5 has fault tolerance of 2
        let set = ValidatorSet::default_3_of_5();
        assert_eq!(set.fault_tolerance(), 2);

        // Wormhole 13-of-19 has fault tolerance of 6
        let wormhole_tolerance = WORMHOLE_VALIDATOR_COUNT - WORMHOLE_THRESHOLD;
        assert_eq!(wormhole_tolerance, 6);

        // At launch, InterLink has lower fault tolerance but is still BFT for small validator sets
        assert!(set.fault_tolerance() > 0, "must tolerate at least 1 failure");
    }
}
