//! ZK proof generation module with verification key caching.
//!
//! Generates Halo2 proofs for cross-chain events. Caches the proving/verifying
//! keys to avoid expensive regeneration on every proof.

use crate::events::GatewayEvent;
use ff::PrimeField;
use halo2_proofs::{
    plonk::{create_proof, keygen_pk, keygen_vk, ProvingKey, VerifyingKey},
    poly::commitment::Params,
    transcript::{Blake2bWrite, Challenge255},
};
use halo2curves::bn256::{Bn256, Fr, G1Affine};
use interlink_core::circuit::InterlinkCircuit;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Cached proving artifacts to avoid regenerating keys per proof
struct ProverCache {
    params: Params<G1Affine>,
    pk: ProvingKey<G1Affine>,
    vk: VerifyingKey<G1Affine>,
}

/// Proof generation engine with key caching.
pub struct ProverEngine {
    cache: Arc<RwLock<Option<ProverCache>>>,
    k: u32,
}

/// The result of proof generation
#[derive(Debug, Clone)]
pub struct ProofPackage {
    pub proof_bytes: Vec<u8>,
    pub public_inputs: Vec<u8>,
    pub sequence: u64,
    pub payload_hash: [u8; 32],
}

impl ProverEngine {
    pub fn new(k: u32) -> Self {
        Self {
            cache: Arc::new(RwLock::new(None)),
            k,
        }
    }

    /// Initialize the prover cache (VK/PK generation). Call once at startup.
    pub async fn initialize(&self) -> Result<(), String> {
        info!(k = self.k, "initializing prover engine, generating keys");

        let k = self.k;
        let (params, pk, vk) = tokio::task::spawn_blocking(move || {
            let params = Params::<G1Affine>::new(k);
            let circuit = InterlinkCircuit::<Fr>::default();
            let vk = keygen_vk(&params, &circuit).expect("vk generation failed");
            let pk = keygen_pk(&params, vk.clone(), &circuit).expect("pk generation failed");
            (params, pk, vk)
        })
        .await
        .map_err(|e| format!("key generation task failed: {}", e))?;

        let mut cache = self.cache.write().await;
        *cache = Some(ProverCache { params, pk, vk });

        info!("prover engine initialized, keys cached");
        Ok(())
    }

    /// Generate a ZK proof for a gateway event.
    pub async fn generate_proof(&self, event: &GatewayEvent) -> Result<ProofPackage, String> {
        let cache_guard = self.cache.read().await;
        let cache = cache_guard
            .as_ref()
            .ok_or("prover not initialized")?;

        let sequence = event.sequence();
        let payload_hash = event.payload_hash();

        info!(sequence, "generating ZK proof");

        // Compute circuit inputs
        let hash = ethers_core::utils::keccak256(b"interlink_v1_domain");
        let mut arr = [0u8; 8];
        arr.copy_from_slice(&hash[0..8]);
        let rc_val = u64::from_be_bytes(arr);

        // Convert payload hash to field element
        let mut payload_arr = [0u8; 8];
        payload_arr.copy_from_slice(&payload_hash[0..8]);
        let payload_val = u64::from_be_bytes(payload_arr);

        let msg_field = Fr::from(payload_val);
        let seq_field = Fr::from(sequence);
        let rc_field = Fr::from(rc_val);

        // Compute public commitment: (msg + rc)^3 + seq
        let diff = msg_field + rc_field;
        let commitment = diff.square() * diff + seq_field;

        let circuit = InterlinkCircuit {
            message_payload: Some(msg_field),
            sequence_number: Some(seq_field),
        };

        // Clone params and pk for the blocking task
        // (In production, we'd use Arc<Params> to avoid cloning)
        let params = cache.params.clone();
        let pk = cache.pk.clone();

        let proof_bytes = tokio::task::spawn_blocking(move || {
            let mut transcript = Blake2bWrite::<_, G1Affine, Challenge255<_>>::init(vec![]);
            create_proof(
                &params,
                &pk,
                &[circuit],
                &[&[&[commitment]]],
                rand::rngs::OsRng,
                &mut transcript,
            )
            .expect("proof generation failed");
            transcript.finalize()
        })
        .await
        .map_err(|e| format!("proof generation task failed: {}", e))?;

        info!(
            sequence,
            proof_size = proof_bytes.len(),
            "proof generated successfully"
        );

        Ok(ProofPackage {
            proof_bytes,
            public_inputs: commitment.to_repr().as_ref().to_vec(),
            sequence,
            payload_hash,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_prover_initialization() {
        let engine = ProverEngine::new(6);
        engine.initialize().await.expect("init should succeed");

        // Verify cache is populated
        let cache = engine.cache.read().await;
        assert!(cache.is_some());
    }
}
