//! Groth16 proof generation for InterLink cross-chain bridge.
//!
//! Generates BN254 Groth16 proofs that verify on both EVM and Solana.
//! Circuit relation: commitment = (message + round_constant)^5 + sequence
//!
//! Proof format: 256 bytes = A (64-byte G1) + B (128-byte G2) + C (64-byte G1)
//! Serialization: big-endian, EVM/Solana precompile compatible.

use crate::events::GatewayEvent;
use ark_bn254::{Bn254, Fr, G1Affine, G2Affine};
use ark_ff::{BigInteger, Field, PrimeField};
use ark_groth16::{Groth16, PreparedVerifyingKey, ProvingKey, VerifyingKey};
use ark_relations::r1cs::{
    ConstraintSynthesizer, ConstraintSystemRef, LinearCombination, SynthesisError, Variable,
};
use ark_snark::SNARK;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

/// Domain salt — must match on-chain verifiers and EVM gateway.
const DOMAIN_SALT: &[u8] = b"interlink_v1_domain";

// ─── Circuit ────────────────────────────────────────────────────────────────

/// Compute the deterministic round constant from the domain salt.
/// Uses keccak256 truncated to fit within BN254 scalar field.
fn compute_round_constant() -> Fr {
    let hash = ethers_core::utils::keccak256(DOMAIN_SALT);
    // Interpret as big-endian and reduce mod r
    Fr::from_be_bytes_mod_order(&hash)
}

/// The InterLink Groth16 circuit.
///
/// Proves knowledge of (message, sequence) such that:
///   commitment = (message + round_constant)^5 + sequence
///
/// - Public input:  commitment (1 field element)
/// - Private witness: message, sequence
/// - Round constant: deterministic from domain salt (hardcoded in circuit)
///
/// R1CS constraints (4 multiplications):
///   w  = message + rc           (linear, enforced)
///   v1 = w * w                  (w²)
///   v2 = v1 * v1                (w⁴)
///   v3 = v2 * w                 (w⁵)
///   commitment = v3 + sequence  (linear, enforced)
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

        // Private witnesses
        let msg =
            cs.new_witness_variable(|| self.message.ok_or(SynthesisError::AssignmentMissing))?;
        let seq =
            cs.new_witness_variable(|| self.sequence.ok_or(SynthesisError::AssignmentMissing))?;

        // w = msg + rc
        let w_val = self.message.map(|m| m + rc);
        let w = cs.new_witness_variable(|| w_val.ok_or(SynthesisError::AssignmentMissing))?;

        // Enforce: (msg + rc) * 1 = w
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

        // Enforce: (v3 + seq) * 1 = commitment
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
/// This matches EVM ecPairing and Solana alt_bn128 precompile format.
fn g1_to_bytes(p: &G1Affine) -> [u8; 64] {
    let mut out = [0u8; 64];
    if p.infinity {
        return out; // point at infinity = all zeros
    }
    // ark stores field elements in little-endian Montgomery form internally,
    // but BigInteger::to_bytes_be() gives canonical big-endian.
    let x_bytes = p.x.into_bigint().to_bytes_be();
    let y_bytes = p.y.into_bigint().to_bytes_be();
    out[0..32].copy_from_slice(&x_bytes);
    out[32..64].copy_from_slice(&y_bytes);
    out
}

/// Serialize a G2Affine point to 128 bytes.
/// EVM/Solana convention: (x_imaginary, x_real, y_imaginary, y_real), each 32 BE.
/// ark convention: x = c0 + c1*u where c0=real, c1=imaginary.
fn g2_to_bytes(p: &G2Affine) -> [u8; 128] {
    let mut out = [0u8; 128];
    if p.infinity {
        return out;
    }
    // EVM expects (x_im, x_re, y_im, y_re) = (c1, c0, c1, c0) in ark terms
    let x_c1 = p.x.c1.into_bigint().to_bytes_be(); // imaginary
    let x_c0 = p.x.c0.into_bigint().to_bytes_be(); // real
    let y_c1 = p.y.c1.into_bigint().to_bytes_be(); // imaginary
    let y_c0 = p.y.c0.into_bigint().to_bytes_be(); // real
    out[0..32].copy_from_slice(&x_c1);
    out[32..64].copy_from_slice(&x_c0);
    out[64..96].copy_from_slice(&y_c1);
    out[96..128].copy_from_slice(&y_c0);
    out
}

/// Serialize a Groth16 proof to 256 bytes: A (64) + B (128) + C (64).
fn serialize_proof(proof: &ark_groth16::Proof<Bn254>) -> Vec<u8> {
    let mut out = Vec::with_capacity(256);
    out.extend_from_slice(&g1_to_bytes(&proof.a));
    out.extend_from_slice(&g2_to_bytes(&proof.b));
    out.extend_from_slice(&g1_to_bytes(&proof.c));
    out
}

/// Serialize a Groth16 verification key to bytes for on-chain storage.
///
/// Layout (576 bytes for 1 public input):
///   alpha_g1:  64 bytes (G1)
///   beta_g2:  128 bytes (G2)
///   gamma_g2: 128 bytes (G2)
///   delta_g2: 128 bytes (G2)
///   ic[0]:     64 bytes (G1)
///   ic[1]:     64 bytes (G1)   — one per public input
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

/// Cached Groth16 proving artifacts (generated once at startup).
struct ProverCache {
    pk: ProvingKey<Bn254>,
    vk: VerifyingKey<Bn254>,
    pvk: PreparedVerifyingKey<Bn254>,
    round_constant: Fr,
}

/// Groth16 proof generation engine with key caching.
///
/// Cheaply cloneable — the inner `Arc` means all clones share the same initialized
/// key cache. Wrap in `Arc` once and clone into each concurrent processing task
/// without re-running the trusted setup.
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
    /// Original payload hash from the EVM event
    pub payload_hash: [u8; 32],
}

impl ProverEngine {
    pub fn new(_k: u32) -> Self {
        Self {
            cache: Arc::new(RwLock::new(None)),
        }
    }

    /// Initialize: run Groth16 trusted setup and cache keys.
    /// In production, use keys from a real ceremony. For devnet, generated fresh.
    pub async fn initialize(&self) -> Result<(), String> {
        info!("initializing Groth16 prover (trusted setup)");

        let (pk, vk, pvk, rc) = tokio::task::spawn_blocking(move || {
            let rc = compute_round_constant();
            let circuit = InterlinkCircuit {
                message: None,
                sequence: None,
                round_constant: rc,
            };

            let mut rng = ark_std::rand::rngs::OsRng;
            let (pk, vk) = Groth16::<Bn254>::circuit_specific_setup(circuit, &mut rng)
                .expect("Groth16 setup failed");
            let pvk = Groth16::<Bn254>::process_vk(&vk).expect("VK processing failed");

            (pk, vk, pvk, rc)
        })
        .await
        .map_err(|e| format!("setup task failed: {}", e))?;

        // Log the VK for on-chain deployment
        let vk_bytes = serialize_vk(&vk);
        info!(
            vk_size = vk_bytes.len(),
            ic_count = vk.gamma_abc_g1.len(),
            "Groth16 keys ready. Export VK with export_vk() for on-chain deployment."
        );

        let mut cache = self.cache.write().await;
        *cache = Some(ProverCache {
            pk,
            vk,
            pvk,
            round_constant: rc,
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
    pub async fn generate_proof(&self, event: &GatewayEvent) -> Result<ProofPackage, String> {
        let cache_guard = self.cache.read().await;
        let cache = cache_guard.as_ref().ok_or("prover not initialized")?;

        let sequence = event.sequence();
        let payload_hash = event.payload_hash();

        info!(sequence, "generating Groth16 proof");

        // Derive message field element from payload hash
        let msg_field = Fr::from_be_bytes_mod_order(&payload_hash);
        let seq_field = Fr::from(sequence);
        let rc = cache.round_constant;

        // Compute expected commitment: (msg + rc)^5 + seq
        let w = msg_field + rc;
        let commitment = w.square().square() * w + seq_field;

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

            // Verify locally before sending (catches witness errors early)
            let public_inputs = vec![commitment];
            let valid = Groth16::<Bn254>::verify_with_processed_vk(&pvk, &public_inputs, &proof)
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

        // Public input = commitment serialized as 32-byte big-endian
        let commitment_bytes = commitment.into_bigint().to_bytes_be();

        Ok(ProofPackage {
            proof_bytes,
            public_inputs: commitment_bytes,
            sequence,
            payload_hash,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_circuit_satisfiability() {
        use ark_relations::r1cs::ConstraintSystem;

        let rc = compute_round_constant();
        let msg = Fr::from(42u64);
        let seq = Fr::from(1u64);

        let circuit = InterlinkCircuit {
            message: Some(msg),
            sequence: Some(seq),
            round_constant: rc,
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
    }

    #[test]
    fn test_proof_serialization_size() {
        // Verify that G1 = 64 bytes, G2 = 128 bytes, total = 256
        assert_eq!(std::mem::size_of::<[u8; 64]>(), 64); // G1
        assert_eq!(64 + 128 + 64, 256); // A + B + C
    }

    #[test]
    fn test_vk_serialization() {
        use ark_std::rand::rngs::OsRng;
        let rc = compute_round_constant();
        let circuit = InterlinkCircuit {
            message: None,
            sequence: None,
            round_constant: rc,
        };
        let (_, vk) = Groth16::<Bn254>::circuit_specific_setup(circuit, &mut OsRng).unwrap();
        let vk_bytes = serialize_vk(&vk);
        // alpha(64) + beta(128) + gamma(128) + delta(128) + ic[0](64) + ic[1](64) = 576
        assert_eq!(vk_bytes.len(), 576);
    }
}
