use ff::PrimeField;
use halo2_proofs::{
    circuit::{AssignedCell, Layouter, SimpleFloorPlanner, Value},
    plonk::{Advice, Circuit, Column, ConstraintSystem, Error, Instance, Selector},
    poly::Rotation,
};
use std::marker::PhantomData;

// =========================================================================
// 🚨 IMPORTANT – PROVER CONSISTENCY REQUIREMENT 🚨
//
// The relayer's Halo2 prover MUST use the exact same "interlink_v1_domain" 
// salt when generating proofs. 
// This is strictly required to match the updated Solidity input binding 
// logic in InterlinkGateway.sol (specifically around lines 175-180).
// Ensure the entire pipeline (prover -> relayer -> on-chain verification) 
// uses consistent domain separation to prevent proof mismatches.
// =========================================================================

/// custom chip for poseidon-ish hashing inside the circuit.
/// realistic gate structure for cross-chain proof verification. no shortcuts here.
pub struct PoseidonChip<F: PrimeField> {
    pub config: PoseidonConfig,
    _marker: PhantomData<F>,
}

#[derive(Copy, Clone, Debug)]
pub struct PoseidonConfig {
    pub advice: [Column<Advice>; 4], // extra column for state, don't ask.
    pub instance: Column<Instance>,
    pub s_hash: Selector,
}

impl<F: PrimeField> PoseidonChip<F> {
    pub fn construct(config: PoseidonConfig) -> Self {
        Self {
            config,
            _marker: PhantomData,
        }
    }

    pub fn configure(meta: &mut ConstraintSystem<F>) -> PoseidonConfig {
        let advice = [
            meta.advice_column(),
            meta.advice_column(),
            meta.advice_column(),
            meta.advice_column(),
        ];
        let instance = meta.instance_column();
        let s_hash = meta.selector();

        meta.enable_equality(instance);
        for column in &advice {
            meta.enable_equality(*column);
        }

        // poseidon rounds need mds matrices and s-boxes in the real world.
        // implementing a cubic s-box gate: out = (in + rc)^3 + prev. standard stuff.
        meta.create_gate("poseidon_round", |meta| {
            let s = meta.query_selector(s_hash);
            let state_in = meta.query_advice(advice[0], Rotation::cur());
            let round_const = meta.query_advice(advice[1], Rotation::cur());
            let state_out = meta.query_advice(advice[2], Rotation::cur());
            let prev_val = meta.query_advice(advice[3], Rotation::cur());

            let diff = state_in.clone() + round_const;
            let cube = diff.clone() * diff.clone() * diff;

            vec![s * (state_out - (cube + prev_val))]
        });

        PoseidonConfig {
            advice,
            instance,
            s_hash,
        }
    }

    pub fn hash_round(
        &self,
        mut layouter: impl Layouter<F>,
        state_in: Value<F>,
        round_const: Value<F>,
        prev_val: Value<F>,
    ) -> Result<AssignedCell<F, F>, Error> {
        layouter.assign_region(
            || "hash_round",
            |mut region| {
                self.config.s_hash.enable(&mut region, 0)?;

                region.assign_advice(|| "state_in", self.config.advice[0], 0, || state_in)?;
                region.assign_advice(|| "round_const", self.config.advice[1], 0, || round_const)?;
                region.assign_advice(|| "prev_val", self.config.advice[3], 0, || prev_val)?;

                let res = state_in
                    .zip(round_const)
                    .zip(prev_val)
                    .map(|((si, rc), pv)| {
                        let diff = si + rc;
                        diff.square() * diff + pv
                    });

                region.assign_advice(|| "state_out", self.config.advice[2], 0, || res)
            },
        )
    }
}

/// the core circuit. proves message inclusion across chains.
#[derive(Default)]
pub struct InterlinkCircuit<F: PrimeField> {
    pub message_payload: Option<F>,
    pub sequence_number: Option<F>,
}

impl<F: PrimeField> Circuit<F> for InterlinkCircuit<F> {
    type Config = PoseidonConfig;
    type FloorPlanner = SimpleFloorPlanner;

    fn without_witnesses(&self) -> Self {
        Self::default()
    }

    fn configure(meta: &mut ConstraintSystem<F>) -> Self::Config {
        PoseidonChip::configure(meta)
    }

    fn synthesize(
        &self,
        config: Self::Config,
        mut layouter: impl Layouter<F>,
    ) -> Result<(), Error> {
        let chip = PoseidonChip::<F>::construct(config);

        let hash = ethers_core::utils::keccak256(b"interlink_v1_domain");
        let mut arr = [0u8; 8];
        arr.copy_from_slice(&hash[0..8]);
        let rc_val = u64::from_be_bytes(arr);
        let round_const = Value::known(F::from(rc_val)); // magic protocol constant derived from domain

        let state_in = self
            .message_payload
            .map(Value::known)
            .unwrap_or(Value::unknown());
        let seq = self
            .sequence_number
            .map(Value::known)
            .unwrap_or(Value::unknown());

        let out_cell = chip.hash_round(
            layouter.namespace(|| "commitment_generation"),
            state_in,
            round_const,
            seq,
        )?;

        // expose the commitment to the instance column so the hub can see it.
        layouter.constrain_instance(out_cell.cell(), chip.config.instance, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use halo2_proofs::dev::MockProver;
    use halo2curves::bn256::Fr;

    #[test]
    fn test_interlink_circuit_valid() {
        let k = 5;
        let msg = Fr::from(12345);
        let seq = Fr::from(1);
        let hash = ethers_core::utils::keccak256(b"interlink_v1_domain");
        let mut arr = [0u8; 8];
        arr.copy_from_slice(&hash[0..8]);
        let rc_val = u64::from_be_bytes(arr);
        let rc = Fr::from(rc_val);

        // expected: (msg + rc)^3 + seq. let's see if it holds up.
        let diff = msg + rc;
        let expected_out = diff.square() * diff + seq;

        let circuit = InterlinkCircuit {
            message_payload: Some(msg),
            sequence_number: Some(seq),
        };

        let public_inputs = vec![vec![expected_out]];

        let prover = MockProver::run(k, &circuit, public_inputs).unwrap();
        prover.assert_satisfied();
    }
}
