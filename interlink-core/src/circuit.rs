use std::marker::PhantomData;
use halo2_proofs::{
    arithmetic::FieldExt,
    circuit::{AssignedCell, Layouter, SimpleFloorPlanner},
    plonk::{Advice, Circuit, Column, ConstraintSystem, Error, Fixed, Instance, Selector},
    poly::Rotation,
};

/// A custom chip for Poseidon hash operations within the circuit.
pub struct PoseidonChip<F: FieldExt> {
    config: PoseidonConfig,
    _marker: PhantomData<F>,
}

#[derive(Clone, Debug)]
pub struct PoseidonConfig {
    pub advice: [Column<Advice>; 3],
    pub instance: Column<Instance>,
    pub s_hash: Selector,
}

impl<F: FieldExt> PoseidonChip<F> {
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
        ];
        let instance = meta.instance_column();
        let s_hash = meta.selector();

        meta.enable_equality(instance);
        for column in &advice {
            meta.enable_equality(*column);
        }

        meta.create_gate("poseidon_round", |meta| {
            let s = meta.query_selector(s_hash);
            let a = meta.query_advice(advice[0], Rotation::cur());
            let b = meta.query_advice(advice[1], Rotation::cur());
            let out = meta.query_advice(advice[2], Rotation::cur());
            
            // Minimal constraint: out = (a + b)^2 (educational simplification)
            vec![s * (out - (a + b) * (a + b))]
        });

        PoseidonConfig { advice, instance, s_hash }
    }
}

/// The core InterLink circuit.
#[derive(Default)]
pub struct InterlinkCircuit<F: FieldExt> {
    pub a: Option<F>,
    pub b: Option<F>,
}

impl<F: FieldExt> Circuit<F> for InterlinkCircuit<F> {
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
        let chip = PoseidonChip::construct(config);

        layouter.assign_region(
            || "hash_round",
            |mut region| {
                chip.config.s_hash.enable(&mut region, 0)?;

                let a_cell = region.assign_advice(
                    || "a",
                    chip.config.advice[0],
                    0,
                    || self.a.ok_or(Error::Synthesis),
                )?;

                let b_cell = region.assign_advice(
                    || "b",
                    chip.config.advice[1],
                    0,
                    || self.b.ok_or(Error::Synthesis),
                )?;

                let _out_cell = region.assign_advice(
                    || "out",
                    chip.config.advice[2],
                    0,
                    || (self.a.unwrap() + self.b.unwrap()).square().into(),
                )?;

                Ok(())
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use halo2_proofs::{dev::MockProver, halo2curves::bn256::Fr};

    #[test]
    fn test_interlink_circuit() {
        let k = 4;
        let a = Fr::from(2);
        let b = Fr::from(3);
        let out = (a + b).square();

        let circuit = InterlinkCircuit {
            a: Some(a),
            b: Some(b),
        };

        let public_inputs = vec![]; // Simplified for now

        let prover = MockProver::run(k, &circuit, public_inputs).unwrap();
        prover.assert_satisfied();
    }
}
