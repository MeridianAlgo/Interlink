use halo2_proofs::{
    circuit::{Layouter, SimpleFloorPlanner, Value},
    plonk::{Advice, Circuit, Column, ConstraintSystem, Error},
};
use ff::PrimeField;
use interlink_core::circuit::{PoseidonChip, PoseidonConfig};

#[derive(Clone, Debug)]
pub struct MerkleConfig {
    poseidon_config: PoseidonConfig,
    pub path_elements: [Column<Advice>; 2],
    pub path_indices: Column<Advice>,
}

pub struct MerkleCircuit<F: PrimeField> {
    pub leaf: Option<F>,
    pub path: Vec<F>,
    pub indices: Vec<F>,
}

impl<F: PrimeField> Circuit<F> for MerkleCircuit<F> {
    type Config = MerkleConfig;
    type FloorPlanner = SimpleFloorPlanner;

    fn without_witnesses(&self) -> Self {
        Self {
            leaf: None,
            path: vec![F::ZERO; self.path.len()],
            indices: vec![F::ZERO; self.indices.len()],
        }
    }

    fn configure(meta: &mut ConstraintSystem<F>) -> Self::Config {
        let poseidon_config = PoseidonChip::<F>::configure(meta);
        
        // We reuse poseidon_config.instance instead of creating a new one
        let path_elements = [meta.advice_column(), meta.advice_column()];
        let path_indices = meta.advice_column();

        for col in path_elements.iter() {
            meta.enable_equality(*col);
        }
        meta.enable_equality(path_indices);

        MerkleConfig {
            poseidon_config,
            path_elements,
            path_indices,
        }
    }

    fn synthesize(
        &self,
        config: Self::Config,
        mut layouter: impl Layouter<F>,
    ) -> Result<(), Error> {
        let chip = PoseidonChip::<F>::construct(config.poseidon_config);

        // 1. Assign the initial leaf
        let mut current_node = layouter.assign_region(
            || "init_leaf",
            |mut region| {
                region.assign_advice(
                    || "leaf",
                    chip.config.advice[0],
                    0,
                    || self.leaf.map(Value::known).unwrap_or(Value::unknown()),
                )
            },
        )?;

        // 2. Iterate through the path and hash at each level
        for i in 0..self.path.len() {
            let path_val = self.path[i];
            let index_val = self.indices[i];

            current_node = layouter.assign_region(
                || format!("merkle_level_{}", i),
                |mut region| {
                    chip.config.s_hash.enable(&mut region, 0)?;

                    // Assign current node as state_in
                    current_node.copy_advice(|| "current_node", &mut region, chip.config.advice[0], 0)?;
                    
                    // Assign path element as round_const
                    region.assign_advice(
                        || "path_element",
                        chip.config.advice[1],
                        0,
                        || Value::known(path_val),
                    )?;

                    // Assign index as prev_val (to influence the hash)
                    region.assign_advice(
                        || "index",
                        chip.config.advice[3],
                        0,
                        || Value::known(index_val),
                    )?;

                    // Calculate next node: (current_node + path_val)^3 + index
                    let next_val = current_node.value().map(|cn| {
                        let diff = *cn + path_val;
                        diff.square() * diff + index_val
                    });

                    region.assign_advice(|| "next_node", chip.config.advice[2], 0, || next_val)
                },
            )?;
        }

        // 3. Expose the final root as public input
        layouter.constrain_instance(current_node.cell(), config.poseidon_config.instance, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use halo2_proofs::dev::MockProver;
    use halo2curves::bn256::Fr;

    #[test]
    fn test_merkle_circuit_valid() {
        let k = 6;
        let leaf = Fr::from(100);
        let path = vec![Fr::from(200), Fr::from(300)];
        let indices = vec![Fr::from(0), Fr::from(1)];

        // Reconstruct root manually
        let mut root = leaf;
        for i in 0..path.len() {
            let diff = root + path[i];
            root = diff.square() * diff + indices[i];
        }

        let circuit = MerkleCircuit {
            leaf: Some(leaf),
            path,
            indices,
        };

        let public_inputs = vec![vec![root]];

        let prover = MockProver::run(k, &circuit, public_inputs).unwrap();
        prover.assert_satisfied();
    }
}
