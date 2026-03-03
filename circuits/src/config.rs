//! Shared circuit configuration for InterLink's Halo2 proving system.
//!
//! Defines the column layout and gate structure used across all InterLink circuits:
//! transaction inclusion, consensus verification, and recursive aggregation.

use ff::PrimeField;
use halo2_proofs::plonk::{Advice, Column, ConstraintSystem, Fixed, Instance, Selector};

/// Unified circuit configuration for InterLink proof generation.
///
/// Layout:
/// - 5 advice columns: state_in, round_const, state_out, prev_val, auxiliary
/// - 1 instance column: public inputs (roots, commitments, sequence numbers)
/// - 2 fixed columns: lookup tables and constant values
/// - 2 selectors: hash gate and verify gate
#[derive(Clone, Debug)]
pub struct InterlinkConfig {
    /// Advice columns for witness data
    /// [0] = state_in / left_input
    /// [1] = round_const / right_input
    /// [2] = state_out / hash_output
    /// [3] = prev_val / index / accumulator
    /// [4] = auxiliary (batch index, chain_id, flags)
    pub advice: [Column<Advice>; 5],

    /// Public inputs column: exposed values verified on-chain
    pub instance: Column<Instance>,

    /// Fixed columns for precomputed values (round constants, MDS matrix entries)
    pub fixed: [Column<Fixed>; 2],

    /// Selector for Poseidon-style hash rounds: out = (in + rc)^5 + prev
    pub s_hash: Selector,

    /// Selector for verification constraints (signature checks, pairing gates)
    pub s_verify: Selector,
}

impl InterlinkConfig {
    /// Configure the constraint system with the standard InterLink column layout.
    pub fn configure<F: PrimeField>(meta: &mut ConstraintSystem<F>) -> Self {
        let advice = [
            meta.advice_column(),
            meta.advice_column(),
            meta.advice_column(),
            meta.advice_column(),
            meta.advice_column(),
        ];

        let instance = meta.instance_column();
        let fixed = [meta.fixed_column(), meta.fixed_column()];
        let s_hash = meta.selector();
        let s_verify = meta.selector();

        // Enable equality constraints on all advice and instance columns
        meta.enable_equality(instance);
        for col in &advice {
            meta.enable_equality(*col);
        }

        // Hash gate: out = (state_in + round_const)^5 + prev_val
        // Quintic S-box: gcd(5, p-1) = 1 for BN254, ensuring bijectivity.
        meta.create_gate("interlink_hash", |meta| {
            let s = meta.query_selector(s_hash);
            let state_in = meta.query_advice(advice[0], halo2_proofs::poly::Rotation::cur());
            let round_const = meta.query_advice(advice[1], halo2_proofs::poly::Rotation::cur());
            let state_out = meta.query_advice(advice[2], halo2_proofs::poly::Rotation::cur());
            let prev_val = meta.query_advice(advice[3], halo2_proofs::poly::Rotation::cur());

            let diff = state_in + round_const;
            let sq = diff.clone() * diff.clone();
            let quint = sq.clone() * sq * diff;

            vec![s * (state_out - (quint + prev_val))]
        });

        // Verify gate: checks that auxiliary == state_in * state_out (used for
        // multiplication-based constraints in signature/pairing verification)
        meta.create_gate("interlink_verify", |meta| {
            let s = meta.query_selector(s_verify);
            let a = meta.query_advice(advice[0], halo2_proofs::poly::Rotation::cur());
            let b = meta.query_advice(advice[2], halo2_proofs::poly::Rotation::cur());
            let c = meta.query_advice(advice[4], halo2_proofs::poly::Rotation::cur());

            vec![s * (c - a * b)]
        });

        InterlinkConfig {
            advice,
            instance,
            fixed,
            s_hash,
            s_verify,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use halo2_proofs::{
        circuit::{Layouter, SimpleFloorPlanner, Value},
        plonk::{Circuit, Error},
    };

    /// Minimal circuit to validate that InterlinkConfig compiles and configures correctly
    struct ConfigTestCircuit;

    impl<F: PrimeField> Circuit<F> for ConfigTestCircuit {
        type Config = InterlinkConfig;
        type FloorPlanner = SimpleFloorPlanner;

        fn without_witnesses(&self) -> Self {
            ConfigTestCircuit
        }

        fn configure(meta: &mut ConstraintSystem<F>) -> Self::Config {
            InterlinkConfig::configure(meta)
        }

        fn synthesize(
            &self,
            config: Self::Config,
            mut layouter: impl Layouter<F>,
        ) -> Result<(), Error> {
            // Test hash gate: (3 + 2)^5 + 1 = 3125 + 1 = 3126
            layouter.assign_region(
                || "test_hash",
                |mut region| {
                    config.s_hash.enable(&mut region, 0)?;
                    region.assign_advice(
                        || "state_in",
                        config.advice[0],
                        0,
                        || Value::known(F::from(3u64)),
                    )?;
                    region.assign_advice(
                        || "round_const",
                        config.advice[1],
                        0,
                        || Value::known(F::from(2u64)),
                    )?;
                    region.assign_advice(
                        || "state_out",
                        config.advice[2],
                        0,
                        || Value::known(F::from(3126u64)),
                    )?;
                    region.assign_advice(
                        || "prev_val",
                        config.advice[3],
                        0,
                        || Value::known(F::from(1u64)),
                    )?;
                    Ok(())
                },
            )?;
            Ok(())
        }
    }

    #[test]
    fn test_config_compiles_and_satisfies() {
        use halo2_proofs::dev::MockProver;
        use halo2curves::bn256::Fr;

        let k = 5;
        let circuit = ConfigTestCircuit;
        let prover = MockProver::<Fr>::run(k, &circuit, vec![vec![]]).unwrap();
        prover.assert_satisfied();
    }
}
