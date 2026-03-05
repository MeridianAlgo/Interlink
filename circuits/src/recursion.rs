//! Recursive proof folding pipeline for InterLink.
//!
//! Implements the accumulation scheme described in the research paper:
//! multiple transaction proofs are folded into a single recursive proof
//! to achieve O(1) amortized verification cost on-chain.
//!
//! Architecture:
//! 1. Individual proofs are added to the pipeline
//! 2. When batch_size is reached, proofs are folded pairwise (tree-structured)
//! 3. The final accumulated proof is wrapped into a SNARK for on-chain submission

use ff::PrimeField;
use halo2_proofs::{
    circuit::{Layouter, SimpleFloorPlanner, Value},
    plonk::{Advice, Circuit, Column, ConstraintSystem, Error, Instance, Selector},
    poly::Rotation,
};
use std::marker::PhantomData;

/// Represents a proof's commitment and evaluation for folding
#[derive(Clone, Debug)]
pub struct ProofAccumulator<F: PrimeField> {
    /// Polynomial commitment (compressed representation)
    pub commitment: F,
    /// Evaluation at the challenge point
    pub evaluation: F,
    /// The public inputs this proof attests to
    pub public_input: F,
}

/// Configuration for the folding/accumulation circuit
#[derive(Clone, Debug)]
pub struct FoldingConfig {
    /// [0] = commitment_1, [1] = commitment_2, [2] = folded_commitment / folded_eval
    /// [3] = evaluation_1, [4] = evaluation_2
    pub advice: [Column<Advice>; 5],
    pub instance: Column<Instance>,
    pub s_fold: Selector,
    pub s_fold_eval: Selector,
}

/// Circuit that verifies two proofs have been correctly folded.
///
/// Folding rule (Fiat-Shamir):
///   alpha = H(C1, C2)  (challenge derived from both commitments)
///   C_new = C1 + alpha * C2
///   e_new = e1 + alpha * e2
///
/// We use the simplified hash: alpha = (C1 + C2)^5 for the Fiat-Shamir challenge.
pub struct FoldingCircuit<F: PrimeField> {
    pub proof_a: Option<ProofAccumulator<F>>,
    pub proof_b: Option<ProofAccumulator<F>>,
    _marker: PhantomData<F>,
}

impl<F: PrimeField> FoldingCircuit<F> {
    pub fn new(a: ProofAccumulator<F>, b: ProofAccumulator<F>) -> Self {
        Self {
            proof_a: Some(a),
            proof_b: Some(b),
            _marker: PhantomData,
        }
    }
}

impl<F: PrimeField> Default for FoldingCircuit<F> {
    fn default() -> Self {
        Self {
            proof_a: None,
            proof_b: None,
            _marker: PhantomData,
        }
    }
}

impl<F: PrimeField> Circuit<F> for FoldingCircuit<F> {
    type Config = FoldingConfig;
    type FloorPlanner = SimpleFloorPlanner;

    fn without_witnesses(&self) -> Self {
        Self::default()
    }

    fn configure(meta: &mut ConstraintSystem<F>) -> Self::Config {
        let advice = [
            meta.advice_column(),
            meta.advice_column(),
            meta.advice_column(),
            meta.advice_column(),
            meta.advice_column(),
        ];
        let instance = meta.instance_column();
        let s_fold = meta.selector();

        meta.enable_equality(instance);
        for col in &advice {
            meta.enable_equality(*col);
        }

        // Folding gate for commitments:
        // Given C1 (advice[0]), C2 (advice[1]), the folded result (advice[2]),
        // and alpha = (C1 + C2)^5:
        // Constraint: folded == C1 + alpha * C2
        meta.create_gate("fold_commitments", |meta| {
            let s = meta.query_selector(s_fold);
            let c1 = meta.query_advice(advice[0], Rotation::cur());
            let c2 = meta.query_advice(advice[1], Rotation::cur());
            let folded = meta.query_advice(advice[2], Rotation::cur());

            // alpha = (C1 + C2)^5 (quintic Fiat-Shamir challenge)
            let sum = c1.clone() + c2.clone();
            let sq = sum.clone() * sum.clone();
            let alpha = sq.clone() * sq * sum;

            // folded = C1 + alpha * C2
            let expected = c1 + alpha * c2;

            vec![s * (folded - expected)]
        });

        // Folding gate for evaluations (must be constrained, not just assigned):
        // e_new = e1 + alpha * e2, using the same alpha derived from commitments.
        // We reuse s_fold on the next row to constrain evaluations.
        let s_fold_eval = meta.selector();

        meta.create_gate("fold_evaluations", |meta| {
            let s = meta.query_selector(s_fold_eval);
            // Read commitments from row 0 to derive alpha
            let c1 = meta.query_advice(advice[0], Rotation::prev());
            let c2 = meta.query_advice(advice[1], Rotation::prev());
            // Read evaluations from current row
            let e1 = meta.query_advice(advice[3], Rotation::cur());
            let e2 = meta.query_advice(advice[4], Rotation::cur());
            let folded_eval = meta.query_advice(advice[2], Rotation::cur());

            let sum = c1 + c2;
            let sq = sum.clone() * sum.clone();
            let alpha = sq.clone() * sq * sum;

            let expected = e1 + alpha * e2;

            vec![s * (folded_eval - expected)]
        });

        FoldingConfig {
            advice,
            instance,
            s_fold,
            s_fold_eval,
        }
    }

    fn synthesize(
        &self,
        config: Self::Config,
        mut layouter: impl Layouter<F>,
    ) -> Result<(), Error> {
        // Single region for both commitment and evaluation folding.
        // Row 0: commitment folding (s_fold enabled)
        // Row 1: evaluation folding (s_fold_eval enabled, reads commitments from row 0)
        let (folded_commitment, folded_eval) = layouter.assign_region(
            || "fold_proofs",
            |mut region| {
                config.s_fold.enable(&mut region, 0)?;
                config.s_fold_eval.enable(&mut region, 1)?;

                let c1 = self
                    .proof_a
                    .as_ref()
                    .map(|p| Value::known(p.commitment))
                    .unwrap_or(Value::unknown());
                let c2 = self
                    .proof_b
                    .as_ref()
                    .map(|p| Value::known(p.commitment))
                    .unwrap_or(Value::unknown());
                let e1 = self
                    .proof_a
                    .as_ref()
                    .map(|p| Value::known(p.evaluation))
                    .unwrap_or(Value::unknown());
                let e2 = self
                    .proof_b
                    .as_ref()
                    .map(|p| Value::known(p.evaluation))
                    .unwrap_or(Value::unknown());

                // Row 0: commitments
                region.assign_advice(|| "c1", config.advice[0], 0, || c1)?;
                region.assign_advice(|| "c2", config.advice[1], 0, || c2)?;

                // Compute alpha = (C1 + C2)^5
                let alpha = c1.zip(c2).map(|(a, b)| {
                    let sum = a + b;
                    let sq = sum.square();
                    sq * sq * sum
                });

                let folded_c = c1.zip(c2).zip(alpha).map(|((a, b), al)| a + al * b);
                let fc = region.assign_advice(|| "folded_c", config.advice[2], 0, || folded_c)?;

                // Row 1: evaluations (commitments from row 0 are read via Rotation::prev)
                region.assign_advice(|| "c1_ref", config.advice[0], 1, || c1)?;
                region.assign_advice(|| "c2_ref", config.advice[1], 1, || c2)?;
                region.assign_advice(|| "e1", config.advice[3], 1, || e1)?;
                region.assign_advice(|| "e2", config.advice[4], 1, || e2)?;

                let folded_e = e1.zip(e2).zip(alpha).map(|((ev1, ev2), al)| ev1 + al * ev2);
                let fe = region.assign_advice(|| "folded_e", config.advice[2], 1, || folded_e)?;

                Ok((fc, fe))
            },
        )?;

        // Expose folded commitment and evaluation as public instances
        layouter.constrain_instance(folded_commitment.cell(), config.instance, 0)?;
        layouter.constrain_instance(folded_eval.cell(), config.instance, 1)?;

        Ok(())
    }
}

/// Pipeline that accumulates proofs and folds them in tree structure.
///
/// Usage:
/// ```ignore
/// let mut pipeline = FoldingPipeline::new(4);
/// pipeline.add_proof(proof1);
/// pipeline.add_proof(proof2);
/// pipeline.add_proof(proof3);
/// pipeline.add_proof(proof4);
/// let final_proof = pipeline.finalize();
/// ```
pub struct FoldingPipeline<F: PrimeField> {
    pending: Vec<ProofAccumulator<F>>,
    batch_size: usize,
}

impl<F: PrimeField> FoldingPipeline<F> {
    pub fn new(batch_size: usize) -> Self {
        Self {
            pending: Vec::with_capacity(batch_size),
            batch_size,
        }
    }

    /// Add a proof to the pipeline. Returns the folded result if batch is full.
    pub fn add_proof(
        &mut self,
        proof: ProofAccumulator<F>,
    ) -> Option<ProofAccumulator<F>> {
        self.pending.push(proof);
        if self.pending.len() >= self.batch_size {
            Some(self.flush_batch())
        } else {
            None
        }
    }

    /// Fold all pending proofs into a single accumulator using tree-structured folding.
    /// O(log N) folding depth.
    pub fn flush_batch(&mut self) -> ProofAccumulator<F> {
        let mut current = std::mem::take(&mut self.pending);

        // Tree-structured pairwise folding
        while current.len() > 1 {
            let mut next = Vec::new();
            let mut i = 0;
            while i + 1 < current.len() {
                next.push(Self::fold_pair(&current[i], &current[i + 1]));
                i += 2;
            }
            // If odd number, carry the last one forward
            if i < current.len() {
                next.push(current[i].clone());
            }
            current = next;
        }

        current.into_iter().next().expect("pipeline cannot be empty")
    }

    /// Fold two proof accumulators into one.
    /// alpha = (C1 + C2)^5  (Fiat-Shamir challenge)
    /// C_new = C1 + alpha * C2
    /// e_new = e1 + alpha * e2
    pub fn fold_pair(
        a: &ProofAccumulator<F>,
        b: &ProofAccumulator<F>,
    ) -> ProofAccumulator<F> {
        let sum = a.commitment + b.commitment;
        let sq = sum.square();
        let alpha = sq * sq * sum;

        ProofAccumulator {
            commitment: a.commitment + alpha * b.commitment,
            evaluation: a.evaluation + alpha * b.evaluation,
            public_input: a.public_input + alpha * b.public_input,
        }
    }

    /// Number of pending proofs
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Whether the pipeline has enough proofs for a batch
    pub fn is_batch_ready(&self) -> bool {
        self.pending.len() >= self.batch_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use halo2_proofs::dev::MockProver;
    use halo2curves::bn256::Fr;

    fn make_proof(c: u64, e: u64, pi: u64) -> ProofAccumulator<Fr> {
        ProofAccumulator {
            commitment: Fr::from(c),
            evaluation: Fr::from(e),
            public_input: Fr::from(pi),
        }
    }

    #[test]
    fn test_folding_circuit() {
        let k = 6;
        let a = make_proof(10, 20, 100);
        let b = make_proof(30, 40, 200);

        // Compute expected folded values
        let sum = a.commitment + b.commitment;
        let sq = sum.square();
        let alpha = sq * sq * sum;
        let expected_commitment = a.commitment + alpha * b.commitment;
        let expected_eval = a.evaluation + alpha * b.evaluation;

        let circuit = FoldingCircuit::new(a, b);
        let public_inputs = vec![vec![expected_commitment, expected_eval]];

        let prover = MockProver::run(k, &circuit, public_inputs).unwrap();
        prover.assert_satisfied();
    }

    #[test]
    fn test_folding_pipeline_pair() {
        let a = make_proof(5, 10, 50);
        let b = make_proof(15, 20, 100);

        let folded = FoldingPipeline::fold_pair(&a, &b);

        let sum = a.commitment + b.commitment;
        let sq = sum.square();
        let alpha = sq * sq * sum;
        assert_eq!(folded.commitment, a.commitment + alpha * b.commitment);
        assert_eq!(folded.evaluation, a.evaluation + alpha * b.evaluation);
    }

    #[test]
    fn test_folding_pipeline_batch() {
        let mut pipeline = FoldingPipeline::<Fr>::new(4);

        assert!(pipeline.add_proof(make_proof(1, 2, 10)).is_none());
        assert!(pipeline.add_proof(make_proof(3, 4, 20)).is_none());
        assert!(pipeline.add_proof(make_proof(5, 6, 30)).is_none());

        // Fourth proof should trigger flush
        let result = pipeline.add_proof(make_proof(7, 8, 40));
        assert!(result.is_some());
        assert_eq!(pipeline.pending_count(), 0);
    }

    #[test]
    fn test_folding_pipeline_odd_count() {
        let mut pipeline = FoldingPipeline::<Fr>::new(8);
        for i in 0..5u64 {
            pipeline.add_proof(make_proof(i + 1, i + 10, i * 100));
        }
        // Force flush with odd number
        let result = pipeline.flush_batch();
        // Should produce a valid accumulator without panicking
        assert_ne!(result.commitment, Fr::from(0u64));
    }
}
