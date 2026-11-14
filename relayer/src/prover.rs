//! Groth16 proof generation for InterLink cross-chain bridge.
//!
//! Generates BN254 Groth16 proofs that verify on both EVM and Solana.
//! Circuit relation: commitment = (message + round_constant)^5 + sequence
//!
//! SECURITY: this circuit only proves knowledge of a preimage of the public
//! commitment, which the verifier computes from the payload hash itself. It
//! does NOT prove that the message was emitted on the source chain, so anyone
//! holding the proving key can "prove" any message. The bridge's real trust
//! assumption is therefore "whoever holds the proving key", until the circuit
//! verifies source-chain consensus and receipt inclusion. See
//! docs/SECURITY_REVIEW.md.
//!
//! Proof format: 256 bytes = A (64-byte G1) + B (128-byte G2) + C (64-byte G1)
//! Serialization: big-endian, EVM/Solana precompile compatible.

use crate::events::{CanonicalMessage, GatewayEvent};
use ark_bn254::{Bn254, Fr, G1Affine, G2Affine};
use ark_ff::{BigInteger, Field, PrimeField};
use ark_groth16::{Groth16, PreparedVerifyingKey, ProvingKey, VerifyingKey};
use ark_relations::r1cs::{
    ConstraintSynthesizer, ConstraintSystemRef, LinearCombination, SynthesisError, Variable,
};
use ark_serialize::{CanonicalDeserialize, CanonicalSerialize};
use ark_snark::SNARK;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Domain salt — must match on-chain verifiers and EVM gateway.
const DOMAIN_SALT: &[u8] = b"interlink_v1_domain";

// ─── Circuit ────────────────────────────────────────────────────────────────

/// Compute the deterministic round constant from the domain salt.
fn compute_round_constant() -> Fr {
    let hash = crate::evm::keccak256(DOMAIN_SALT);
    Fr::from_be_bytes_mod_order(&hash)
}

/// Public input for a payload hash and sequence: (msg + rc)^5 + seq.
/// Must match `InterlinkGateway.commitment` and the hub's `compute_commitment`.
pub fn compute_commitment(payload_hash: &[u8; 32], sequence: u64) -> Fr {
    let w = Fr::from_be_bytes_mod_order(payload_hash) + compute_round_constant();
    w.square().square() * w + Fr::from(sequence)
}

/// The InterLink Groth16 circuit.
///
/// Proves knowledge of (message, sequence) such that:
///   commitment = (message + round_constant)^5 + sequence
///
/// - Public input:  commitment (1 field element)
/// - Private witness: message, sequence
/// - Round constant: deterministic from domain salt (hardcoded in circuit)
#[derive(Clone)]
pub struct InterlinkCircuit {
    pub message: Option<Fr>,
    pub sequence: Option<Fr>,
    pub round_constant: Fr,
}

impl Default for InterlinkCircuit {
    fn default() -> Self {
        Self {
            message: None,
            sequence: None,
            round_constant: compute_round_constant(),
        }
    }
}

impl ConstraintSynthesizer<Fr> for InterlinkCircuit {
    fn generate_constraints(self, cs: ConstraintSystemRef<Fr>) -> Result<(), SynthesisError> {
        let rc = self.round_constant;

        let msg =
            cs.new_witness_variable(|| self.message.ok_or(SynthesisError::AssignmentMissing))?;
        let seq =
            cs.new_witness_variable(|| self.sequence.ok_or(SynthesisError::AssignmentMissing))?;

        // w = msg + rc
        let w_val = self.message.map(|m| m + rc);
        let w = cs.new_witness_variable(|| w_val.ok_or(SynthesisError::AssignmentMissing))?;
        {
            let mut a = LinearCombination::zero();
            a += (ark_ff::One::one(), msg);
            a += (rc, Variable::One);
            cs.enforce_constraint(
                a,
                LinearCombination::from(Variable::One),
                LinearCombination::from(w),
            )?;
        }

        // v1 = w²
        let v1_val = w_val.map(|w| w.square());
        let v1 = cs.new_witness_variable(|| v1_val.ok_or(SynthesisError::AssignmentMissing))?;
        cs.enforce_constraint(
            LinearCombination::from(w),
            LinearCombination::from(w),
            LinearCombination::from(v1),
        )?;

        // v2 = v1² = w⁴
        let v2_val = v1_val.map(|v| v.square());
        let v2 = cs.new_witness_variable(|| v2_val.ok_or(SynthesisError::AssignmentMissing))?;
        cs.enforce_constraint(
            LinearCombination::from(v1),
            LinearCombination::from(v1),
            LinearCombination::from(v2),
        )?;

        // v3 = v2 * w = w⁵
        let v3_val = v2_val.zip(w_val).map(|(v2, w)| v2 * w);
        let v3 = cs.new_witness_variable(|| v3_val.ok_or(SynthesisError::AssignmentMissing))?;
        cs.enforce_constraint(
            LinearCombination::from(v2),
            LinearCombination::from(w),
            LinearCombination::from(v3),
        )?;

        // commitment = v3 + seq  (PUBLIC INPUT)
        let commitment_val = v3_val.zip(self.sequence).map(|(v3, s)| v3 + s);
        let commitment =
            cs.new_input_variable(|| commitment_val.ok_or(SynthesisError::AssignmentMissing))?;
        {
            let mut a = LinearCombination::zero();
            a += (ark_ff::One::one(), v3);
            a += (ark_ff::One::one(), seq);
            cs.enforce_constraint(
                a,
                LinearCombination::from(Variable::One),
                LinearCombination::from(commitment),
            )?;
        }

        Ok(())
    }
}

// ─── Serialization helpers (ark ↔ EVM/Solana big-endian) ────────────────────

/// Serialize a G1Affine point to 64 bytes (x: 32 BE, y: 32 BE).
fn g1_to_bytes(p: &G1Affine) -> [u8; 64] {
    let mut out = [0u8; 64];
    if p.infinity {
        return out;
    }
    out[0..32].copy_from_slice(&p.x.into_bigint().to_bytes_be());
    out[32..64].copy_from_slice(&p.y.into_bigint().to_bytes_be());
    out
}

/// Serialize a G2Affine point to 128 bytes: (x_im, x_re, y_im, y_re), each 32 BE.
fn g2_to_bytes(p: &G2Affine) -> [u8; 128] {
    let mut out = [0u8; 128];
    if p.infinity {
        return out;
    }
    out[0..32].copy_from_slice(&p.x.c1.into_bigint().to_bytes_be());
    out[32..64].copy_from_slice(&p.x.c0.into_bigint().to_bytes_be());
    out[64..96].copy_from_slice(&p.y.c1.into_bigint().to_bytes_be());
    out[96..128].copy_from_slice(&p.y.c0.into_bigint().to_bytes_be());
    out
}

fn serialize_proof(proof: &ark_groth16::Proof<Bn254>) -> Vec<u8> {
    let mut out = Vec::with_capacity(256);
    out.extend_from_slice(&g1_to_bytes(&proof.a));
    out.extend_from_slice(&g2_to_bytes(&proof.b));
    out.extend_from_slice(&g1_to_bytes(&proof.c));
    out
}

/// Serialize a Groth16 verification key (576 bytes for 1 public input):
/// alpha_g1 (64) | beta_g2 (128) | gamma_g2 (128) | delta_g2 (128) | ic[0] (64) | ic[1] (64)
pub fn serialize_vk(vk: &VerifyingKey<Bn254>) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&g1_to_bytes(&vk.alpha_g1));
    out.extend_from_slice(&g2_to_bytes(&vk.beta_g2));
    out.extend_from_slice(&g2_to_bytes(&vk.gamma_g2));
    out.extend_from_slice(&g2_to_bytes(&vk.delta_g2));
    for ic in &vk.gamma_abc_g1 {
        out.extend_from_slice(&g1_to_bytes(ic));
    }
    out
}

// ─── Proof engine ───────────────────────────────────────────────────────────

struct ProverCache {
    pk: ProvingKey<Bn254>,
    vk: VerifyingKey<Bn254>,
    pvk: PreparedVerifyingKey<Bn254>,
    round_constant: Fr,
}

/// Groth16 proof generation engine with key caching. Cheaply cloneable.
#[derive(Clone)]
pub struct ProverEngine {
    cache: Arc<RwLock<Option<ProverCache>>>,
}

/// The result of proof generation.
#[derive(Debug, Clone)]
pub struct ProofPackage {
    /// 256-byte Groth16 proof: A (64) + B (128) + C (64)
    pub proof_bytes: Vec<u8>,
    /// Public input (commitment) as 32-byte big-endian field element
    pub public_inputs: Vec<u8>,
    /// Event sequence number
    pub sequence: u64,
    /// Canonical payload hash the proof is bound to
    pub payload_hash: [u8; 32],
    /// The canonical message (None for legacy events, which cannot be submitted)
    pub message: Option<CanonicalMessage>,
}

impl ProverEngine {
    pub fn new(_k: u32) -> Self {
        Self {
            cache: Arc::new(RwLock::new(None)),
        }
    }

    /// Run a fresh, throwaway Groth16 setup. **Tests and benchmarks only**:
    /// every call produces a different verification key, so proofs from one
    /// process never verify against a VK uploaded by another.
    pub async fn initialize(&self) -> Result<(), String> {
        let (pk, vk) = tokio::task::spawn_blocking(|| {
            let mut rng = ark_std::rand::rngs::OsRng;
            Groth16::<Bn254>::circuit_specific_setup(InterlinkCircuit::default(), &mut rng)
                .map_err(|e| format!("Groth16 setup failed: {}", e))
        })
        .await
        .map_err(|e| format!("setup task failed: {}", e))??;
        self.install(pk, vk).await
    }

    /// Load the proving key from `path`. If it does not exist and
    /// `allow_create` is set, run a local setup and save it there.
    ///
    /// The relayer and `export-vk` must use the same file, otherwise the
    /// on-chain VK will not match the proofs. A locally generated key is a
    /// single-party trusted setup: whoever ran it could keep the toxic waste.
    /// Production keys must come from a multi-party ceremony.
    pub async fn load_or_create(&self, path: &Path, allow_create: bool) -> Result<(), String> {
        let pk = if path.exists() {
            let bytes = std::fs::read(path)
                .map_err(|e| format!("read proving key {}: {}", path.display(), e))?;
            let pk = tokio::task::spawn_blocking(move || {
                ProvingKey::<Bn254>::deserialize_compressed(bytes.as_slice())
            })
            .await
            .map_err(|e| format!("deserialize task failed: {}", e))?
            .map_err(|e| format!("invalid proving key {}: {}", path.display(), e))?;
            info!(path = %path.display(), "loaded Groth16 proving key");
            pk
        } else if allow_create {
            warn!(
                path = %path.display(),
                "no proving key found; running a LOCAL single-party setup (dev only)"
            );
            let pk = tokio::task::spawn_blocking(|| {
                let mut rng = ark_std::rand::rngs::OsRng;
                Groth16::<Bn254>::circuit_specific_setup(InterlinkCircuit::default(), &mut rng)
                    .map(|(pk, _)| pk)
                    .map_err(|e| format!("Groth16 setup failed: {}", e))
            })
            .await
            .map_err(|e| format!("setup task failed: {}", e))??;
            let mut bytes = Vec::new();
            pk.serialize_compressed(&mut bytes)
                .map_err(|e| format!("serialize proving key: {}", e))?;
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)
                    .map_err(|e| format!("create {}: {}", dir.display(), e))?;
            }
            std::fs::write(path, bytes)
                .map_err(|e| format!("write proving key {}: {}", path.display(), e))?;
            pk
        } else {
            return Err(format!(
                "proving key {} not found; generate it with `cargo run --bin export-vk`",
                path.display()
            ));
        };
        let vk = pk.vk.clone();
        self.install(pk, vk).await
    }

    async fn install(&self, pk: ProvingKey<Bn254>, vk: VerifyingKey<Bn254>) -> Result<(), String> {
        let pvk = Groth16::<Bn254>::process_vk(&vk).map_err(|e| format!("VK processing: {}", e))?;
        info!(vk_size = serialize_vk(&vk).len(), "Groth16 keys ready");
        *self.cache.write().await = Some(ProverCache {
            pk,
            vk,
            pvk,
            round_constant: compute_round_constant(),
        });
        Ok(())
    }

    /// Export the verification key bytes for uploading to on-chain verifiers.
    pub async fn export_vk(&self) -> Result<Vec<u8>, String> {
        let cache = self.cache.read().await;
        let c = cache.as_ref().ok_or("prover not initialized")?;
        Ok(serialize_vk(&c.vk))
    }

    /// Generate a Groth16 proof for a gateway event.
    ///
    /// For current-format events the payload hash is recomputed from the
    /// event fields; a mismatch is refused rather than proven.
    pub async fn generate_proof(&self, event: &GatewayEvent) -> Result<ProofPackage, String> {
        match event {
            GatewayEvent::Message(_) => {
                let msg = event
                    .verified_message()
                    .ok_or("event payload hash does not match its fields; refusing to prove")?;
                self.prove_message(msg).await
            }
            _ => {
                self.prove_hash(event.payload_hash(), event.sequence(), None)
                    .await
            }
        }
    }

    /// Prove a canonical message in either direction (gateway → hub, or hub → gateway).
    pub async fn prove_message(&self, msg: &CanonicalMessage) -> Result<ProofPackage, String> {
        self.prove_hash(msg.hash(), msg.nonce, Some(msg.clone()))
            .await
    }

    async fn prove_hash(
        &self,
        payload_hash: [u8; 32],
        sequence: u64,
        message: Option<CanonicalMessage>,
    ) -> Result<ProofPackage, String> {
        let cache_guard = self.cache.read().await;
        let cache = cache_guard.as_ref().ok_or("prover not initialized")?;

        info!(sequence, "generating Groth16 proof");

        let msg_field = Fr::from_be_bytes_mod_order(&payload_hash);
        let seq_field = Fr::from(sequence);
        let rc = cache.round_constant;
        let commitment = compute_commitment(&payload_hash, sequence);

        let circuit = InterlinkCircuit {
            message: Some(msg_field),
            sequence: Some(seq_field),
            round_constant: rc,
        };

        let pk = cache.pk.clone();
        let pvk = cache.pvk.clone();

        let proof_bytes = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
            let mut rng = ark_std::rand::rngs::OsRng;
            let proof = Groth16::<Bn254>::prove(&pk, circuit, &mut rng)
                .map_err(|e| format!("proof generation failed: {}", e))?;

            let valid = Groth16::<Bn254>::verify_with_processed_vk(&pvk, &[commitment], &proof)
                .map_err(|e| format!("local verify error: {}", e))?;
            if !valid {
                return Err("locally generated proof failed verification — circuit bug".to_string());
            }
            Ok(serialize_proof(&proof))
        })
        .await
        .map_err(|e| format!("proof task panicked: {}", e))??;

        if proof_bytes.len() != 256 {
            return Err(format!(
                "proof has unexpected length: {}",
                proof_bytes.len()
            ));
        }

        info!(sequence, "Groth16 proof generated and locally verified");

        Ok(ProofPackage {
            proof_bytes,
            public_inputs: commitment.into_bigint().to_bytes_be(),
            sequence,
            payload_hash,
            message,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_circuit_satisfiability() {
        use ark_relations::r1cs::ConstraintSystem;

        let circuit = InterlinkCircuit {
            message: Some(Fr::from(42u64)),
            sequence: Some(Fr::from(1u64)),
            round_constant: compute_round_constant(),
        };
        let cs = ConstraintSystem::<Fr>::new_ref();
        circuit.generate_constraints(cs.clone()).unwrap();
        assert!(cs.is_satisfied().unwrap(), "circuit not satisfied");
    }

    #[tokio::test]
    async fn test_full_prove_verify() {
        let engine = ProverEngine::new(0);
        engine.initialize().await.unwrap();

        let event = GatewayEvent::Deposit(crate::events::DepositEvent {
            block_number: 100,
            tx_hash: [0u8; 32],
            sequence: 1,
            sender: [0u8; 20],
            recipient: vec![0u8; 20],
            amount: 1000,
            destination_chain: 2,
            payload_hash: [0xAB; 32],
        });

        let package = engine.generate_proof(&event).await.unwrap();
        assert_eq!(package.proof_bytes.len(), 256);
        assert_eq!(package.sequence, 1);
        assert!(package.message.is_none());
    }

    #[tokio::test]
    async fn test_refuses_message_with_wrong_hash() {
        use crate::events::{MessageEvent, ACTION_TRANSFER, HUB_CHAIN_ID};
        let engine = ProverEngine::new(0);
        engine.initialize().await.unwrap();

        let message = CanonicalMessage {
            src_chain: 1,
            src_gateway: [1; 32],
            dst_chain: HUB_CHAIN_ID,
            dst_gateway: [2; 32],
            nonce: 4,
            action: ACTION_TRANSFER,
            sender: [3; 32],
            asset: [0; 32],
            amount: 10,
            recipient: [4; 32],
            extra_hash: [5; 32],
        };
        let mut ev = MessageEvent {
            block_number: 1,
            block_hash: [0; 32],
            tx_hash: [0; 32],
            gateway: [0; 20],
            payload_hash: message.hash(),
            message,
            data: vec![],
        };
        let ok = engine
            .generate_proof(&GatewayEvent::Message(ev.clone()))
            .await
            .unwrap();
        assert_eq!(ok.payload_hash, ev.payload_hash);
        assert!(ok.message.is_some());

        ev.message.amount = 1_000_000;
        assert!(engine
            .generate_proof(&GatewayEvent::Message(ev))
            .await
            .is_err());
    }

    /// The whole point of persisting the key: two loads give the same VK.
    #[tokio::test]
    async fn test_proving_key_persists_across_restarts() {
        let dir = std::env::temp_dir().join(format!("interlink-pk-test-{}", std::process::id()));
        let path = dir.join("pk.bin");
        let _ = std::fs::remove_file(&path);

        let a = ProverEngine::new(0);
        assert!(
            a.load_or_create(&path, false).await.is_err(),
            "must not create silently"
        );
        a.load_or_create(&path, true).await.unwrap();
        let b = ProverEngine::new(0);
        b.load_or_create(&path, false).await.unwrap();
        assert_eq!(a.export_vk().await.unwrap(), b.export_vk().await.unwrap());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_vk_serialization() {
        use ark_std::rand::rngs::OsRng;
        let (_, vk) =
            Groth16::<Bn254>::circuit_specific_setup(InterlinkCircuit::default(), &mut OsRng)
                .unwrap();
        assert_eq!(serialize_vk(&vk).len(), 576);
    }
}
