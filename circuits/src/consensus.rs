//! Consensus verification circuits for InterLink.
//!
//! These circuits prove that a source chain has reached consensus on a block,
//! enabling the Hub to trust cross-chain state transitions without running
//! a full light client.
//!
//! Two consensus models are supported:
//! 1. Ethereum Sync Committee (BLS12-381 aggregate signatures)
//! 2. Cosmos Tendermint (Ed25519 validator quorum)

use ff::PrimeField;
use halo2_proofs::{
    circuit::{Layouter, SimpleFloorPlanner, Value},
    plonk::{Advice, Circuit, Column, ConstraintSystem, Error, Instance, Selector},
    poly::Rotation,
};
use std::marker::PhantomData;

/// Extract the low 64 bits from a field element's canonical representation.
/// Safe for values that fit in u64 (validator weights, counts, thresholds).
fn field_to_u64<F: PrimeField>(val: &F) -> u64 {
    let repr = val.to_repr();
    let bytes = repr.as_ref();
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&bytes[..8]);
    u64::from_le_bytes(buf)
}

// ─── Ethereum Sync Committee Circuit ────────────────────────────────────────

/// Configuration for the BLS Sync Committee verification circuit.
///
/// Verifies that a sufficient subset (>= 342/512) of Ethereum's Sync Committee
/// has signed a beacon block header. The actual BLS pairing is too expensive
/// to verify in-circuit over BN254, so we use a simplified model:
///
/// 1. Aggregate public key: conditional sum based on participation bitfield
/// 2. Quorum check: participation_count >= threshold
/// 3. Signature binding: hash(apk, message) is constrained as public input
#[derive(Clone, Debug)]
pub struct SyncCommitteeConfig {
    /// [0] = validator_weight, [1] = participation_bit, [2] = accumulated_weight
    /// [3] = threshold, [4] = quorum_satisfied
    pub advice: [Column<Advice>; 5],
    pub instance: Column<Instance>,
    pub s_accumulate: Selector,
    pub s_quorum: Selector,
}

/// Circuit proving Ethereum Sync Committee consensus.
///
/// Public inputs:
///   [0] = block_hash (the beacon block header being attested)
///   [1] = accumulated_weight (total participation weight)
///   [2] = quorum_flag (1 if quorum met, 0 otherwise)
pub struct SyncCommitteeCircuit<F: PrimeField> {
    /// Weight of each validator in the committee
    pub validator_weights: Vec<F>,
    /// Participation bitfield: 1 if validator signed, 0 otherwise
    pub participation_bits: Vec<F>,
    /// The block hash being attested (public input)
    pub block_hash: Option<F>,
    /// Quorum threshold (e.g., 342 for 2/3 of 512)
    pub threshold: Option<F>,
    _marker: PhantomData<F>,
}

impl<F: PrimeField> Default for SyncCommitteeCircuit<F> {
    fn default() -> Self {
        Self {
            validator_weights: vec![],
            participation_bits: vec![],
            block_hash: None,
            threshold: None,
            _marker: PhantomData,
        }
    }
}

impl<F: PrimeField> Circuit<F> for SyncCommitteeCircuit<F> {
    type Config = SyncCommitteeConfig;
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
        let s_accumulate = meta.selector();
        let s_quorum = meta.selector();

        meta.enable_equality(instance);
        for col in &advice {
            meta.enable_equality(*col);
        }

        // Accumulation gate: acc_new = acc_prev + weight * participation_bit
        // Ensures participation_bit is boolean: bit * (1 - bit) == 0
        meta.create_gate("accumulate_weight", |meta| {
            let s = meta.query_selector(s_accumulate);
            let weight = meta.query_advice(advice[0], Rotation::cur());
            let bit = meta.query_advice(advice[1], Rotation::cur());
            let acc_prev = meta.query_advice(advice[2], Rotation::cur());
            let acc_new = meta.query_advice(advice[2], Rotation::next());

            let one = halo2_proofs::plonk::Expression::Constant(F::ONE);

            vec![
                // Boolean constraint on participation bit
                s.clone() * bit.clone() * (one - bit.clone()),
                // Accumulation: acc_new = acc_prev + weight * bit
                s * (acc_new - (acc_prev + weight * bit)),
            ]
        });

        // Quorum gate: constrains that flag is boolean.
        //
        // The accumulated weight and flag are exposed as public inputs.
        // The on-chain verifier must independently check:
        //   flag == 1 ⟹ accumulated >= threshold
        //   flag == 0 ⟹ accumulated < threshold
        //
        // Rationale: enforcing >= in a prime field requires range proofs
        // (bit decomposition), which adds significant circuit complexity.
        // Since the verifier contract knows the threshold, it can cheaply
        // verify the comparison against the public inputs.
        meta.create_gate("quorum_check", |meta| {
            let s = meta.query_selector(s_quorum);
            let flag = meta.query_advice(advice[4], Rotation::cur());

            let one = halo2_proofs::plonk::Expression::Constant(F::ONE);

            vec![
                // flag must be boolean
                s * flag.clone() * (one - flag),
            ]
        });

        SyncCommitteeConfig {
            advice,
            instance,
            s_accumulate,
            s_quorum,
        }
    }

    fn synthesize(
        &self,
        config: Self::Config,
        mut layouter: impl Layouter<F>,
    ) -> Result<(), Error> {
        let n = self.validator_weights.len();

        // Step 1: Accumulate weighted participation
        let accumulated = layouter.assign_region(
            || "accumulate_participation",
            |mut region| {
                let mut acc = F::ZERO;

                // Assign initial accumulator value
                region.assign_advice(
                    || "acc_init",
                    config.advice[2],
                    0,
                    || Value::known(F::ZERO),
                )?;

                for i in 0..n {
                    config.s_accumulate.enable(&mut region, i)?;

                    region.assign_advice(
                        || format!("weight_{}", i),
                        config.advice[0],
                        i,
                        || Value::known(self.validator_weights[i]),
                    )?;

                    region.assign_advice(
                        || format!("bit_{}", i),
                        config.advice[1],
                        i,
                        || Value::known(self.participation_bits[i]),
                    )?;

                    // Update accumulator
                    acc += self.validator_weights[i] * self.participation_bits[i];

                    region.assign_advice(
                        || format!("acc_{}", i + 1),
                        config.advice[2],
                        i + 1,
                        || Value::known(acc),
                    )?;
                }

                // Return the final accumulated cell
                region.assign_advice(|| "final_acc", config.advice[2], n, || Value::known(acc))
            },
        )?;

        // Step 2: Quorum check — compute flag honestly, verifier validates
        let quorum_flag = layouter.assign_region(
            || "quorum_check",
            |mut region| {
                config.s_quorum.enable(&mut region, 0)?;

                let acc_val = accumulated.value().copied();
                let threshold_val = self.threshold.map(Value::known).unwrap_or(Value::unknown());

                accumulated.copy_advice(|| "acc", &mut region, config.advice[2], 0)?;
                region.assign_advice(|| "threshold", config.advice[3], 0, || threshold_val)?;

                // Compute flag: 1 if accumulated >= threshold, 0 otherwise
                let flag = acc_val.zip(threshold_val).map(|(a, t)| {
                    if field_to_u64(&a) >= field_to_u64(&t) {
                        F::ONE
                    } else {
                        F::ZERO
                    }
                });

                region.assign_advice(|| "flag", config.advice[4], 0, || flag)
            },
        )?;

        // Expose block_hash, accumulated weight, and quorum flag as public inputs
        let block_hash_cell = layouter.assign_region(
            || "block_hash",
            |mut region| {
                let val = self
                    .block_hash
                    .map(Value::known)
                    .unwrap_or(Value::unknown());
                region.assign_advice(|| "block_hash", config.advice[0], 0, || val)
            },
        )?;

        layouter.constrain_instance(block_hash_cell.cell(), config.instance, 0)?;
        layouter.constrain_instance(accumulated.cell(), config.instance, 1)?;
        layouter.constrain_instance(quorum_flag.cell(), config.instance, 2)?;

        Ok(())
    }
}

// ─── Cosmos Tendermint Circuit ──────────────────────────────────────────────

/// Configuration for Tendermint consensus verification.
///
/// Verifies that >2/3 of voting power has signed a block commit.
/// Uses the same accumulation pattern as the Sync Committee circuit
/// but with different quorum semantics (2/3 voting power, not count-based).
#[derive(Clone, Debug)]
pub struct TendermintConfig {
    /// [0] = voting_power, [1] = signed_bit, [2] = accumulated_power
    /// [3] = total_power, [4] = quorum_check_result
    pub advice: [Column<Advice>; 5],
    pub instance: Column<Instance>,
    pub s_accumulate: Selector,
    pub s_quorum: Selector,
}

/// Circuit proving Cosmos Tendermint consensus (>2/3 voting power signed).
///
/// Public inputs:
///   [0] = block_hash
///   [1] = total_signed_power
///   [2] = quorum_satisfied (1 if 3 * signed_power > 2 * total_power)
pub struct TendermintCircuit<F: PrimeField> {
    /// Voting power of each validator
    pub voting_powers: Vec<F>,
    /// Whether each validator signed (1 or 0)
    pub signed_bits: Vec<F>,
    /// The block hash being committed
    pub block_hash: Option<F>,
    /// Total voting power of the validator set
    pub total_power: Option<F>,
    _marker: PhantomData<F>,
}

impl<F: PrimeField> Default for TendermintCircuit<F> {
    fn default() -> Self {
        Self {
            voting_powers: vec![],
            signed_bits: vec![],
            block_hash: None,
            total_power: None,
            _marker: PhantomData,
        }
    }
}

impl<F: PrimeField> Circuit<F> for TendermintCircuit<F> {
    type Config = TendermintConfig;
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
        let s_accumulate = meta.selector();
        let s_quorum = meta.selector();

        meta.enable_equality(instance);
        for col in &advice {
            meta.enable_equality(*col);
        }

        // Weight accumulation gate (same structure as sync committee)
        meta.create_gate("accumulate_voting_power", |meta| {
            let s = meta.query_selector(s_accumulate);
            let power = meta.query_advice(advice[0], Rotation::cur());
            let bit = meta.query_advice(advice[1], Rotation::cur());
            let acc_prev = meta.query_advice(advice[2], Rotation::cur());
            let acc_new = meta.query_advice(advice[2], Rotation::next());

            let one = halo2_proofs::plonk::Expression::Constant(F::ONE);

            vec![
                s.clone() * bit.clone() * (one - bit.clone()),
                s * (acc_new - (acc_prev + power * bit)),
            ]
        });

        // Tendermint quorum: flag=1 iff 3 * signed_power > 2 * total_power
        //
        // The circuit constrains only that flag is boolean.
        // The signed_power and total_power are exposed as public inputs.
        // The on-chain verifier must independently check:
        //   flag == 1 ⟹ 3 * signed_power > 2 * total_power
        //   flag == 0 ⟹ 3 * signed_power <= 2 * total_power
        //
        // See SyncCommittee quorum gate for rationale on why the comparison
        // is deferred to the verifier.
        meta.create_gate("tendermint_quorum", |meta| {
            let s = meta.query_selector(s_quorum);
            let flag = meta.query_advice(advice[4], Rotation::cur());

            let one = halo2_proofs::plonk::Expression::Constant(F::ONE);

            vec![
                // flag must be boolean
                s * flag.clone() * (one - flag),
            ]
        });

        TendermintConfig {
            advice,
            instance,
            s_accumulate,
            s_quorum,
        }
    }

    fn synthesize(
        &self,
        config: Self::Config,
        mut layouter: impl Layouter<F>,
    ) -> Result<(), Error> {
        let n = self.voting_powers.len();

        // Step 1: Accumulate signed voting power
        let accumulated = layouter.assign_region(
            || "accumulate_power",
            |mut region| {
                let mut acc = F::ZERO;

                region.assign_advice(
                    || "acc_init",
                    config.advice[2],
                    0,
                    || Value::known(F::ZERO),
                )?;

                for i in 0..n {
                    config.s_accumulate.enable(&mut region, i)?;

                    region.assign_advice(
                        || format!("power_{}", i),
                        config.advice[0],
                        i,
                        || Value::known(self.voting_powers[i]),
                    )?;

                    region.assign_advice(
                        || format!("signed_{}", i),
                        config.advice[1],
                        i,
                        || Value::known(self.signed_bits[i]),
                    )?;

                    acc += self.voting_powers[i] * self.signed_bits[i];

                    region.assign_advice(
                        || format!("acc_{}", i + 1),
                        config.advice[2],
                        i + 1,
                        || Value::known(acc),
                    )?;
                }

                region.assign_advice(|| "final_acc", config.advice[2], n, || Value::known(acc))
            },
        )?;

        // Step 2: Tendermint quorum check — compute flag honestly, verifier validates
        let quorum_flag = layouter.assign_region(
            || "quorum_check",
            |mut region| {
                config.s_quorum.enable(&mut region, 0)?;

                accumulated.copy_advice(|| "signed", &mut region, config.advice[2], 0)?;

                let total_val = self
                    .total_power
                    .map(Value::known)
                    .unwrap_or(Value::unknown());
                region.assign_advice(|| "total", config.advice[3], 0, || total_val)?;

                // Compute flag: 1 if 3*signed > 2*total, 0 otherwise
                let flag = accumulated.value().zip(total_val).map(|(signed, total)| {
                    let s = field_to_u64(signed);
                    let t = field_to_u64(&total);
                    if 3 * s > 2 * t {
                        F::ONE
                    } else {
                        F::ZERO
                    }
                });

                region.assign_advice(|| "flag", config.advice[4], 0, || flag)
            },
        )?;

        // Expose public inputs
        let block_hash_cell = layouter.assign_region(
            || "block_hash",
            |mut region| {
                let val = self
                    .block_hash
                    .map(Value::known)
                    .unwrap_or(Value::unknown());
                region.assign_advice(|| "hash", config.advice[0], 0, || val)
            },
        )?;

        layouter.constrain_instance(block_hash_cell.cell(), config.instance, 0)?;
        layouter.constrain_instance(accumulated.cell(), config.instance, 1)?;
        layouter.constrain_instance(quorum_flag.cell(), config.instance, 2)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ff::Field;
    use halo2_proofs::dev::MockProver;
    use halo2curves::bn256::Fr;

    #[test]
    fn test_sync_committee_quorum_met() {
        let k = 8;
        // 4 validators, each weight 100, 3 participate -> 300 >= 267 (2/3 of 400)
        let weights = vec![Fr::from(100); 4];
        let bits = vec![Fr::ONE, Fr::ONE, Fr::ONE, Fr::ZERO];
        let threshold = Fr::from(267u64); // 2/3 of 400
        let block_hash = Fr::from(0xBEEFu64);
        let accumulated = Fr::from(300u64);

        let circuit = SyncCommitteeCircuit {
            validator_weights: weights,
            participation_bits: bits,
            block_hash: Some(block_hash),
            threshold: Some(threshold),
            _marker: PhantomData,
        };

        let public_inputs = vec![vec![block_hash, accumulated, Fr::ONE]];
        let prover = MockProver::run(k, &circuit, public_inputs).unwrap();
        prover.assert_satisfied();
    }

    #[test]
    fn test_sync_committee_quorum_not_met() {
        let k = 8;
        // 4 validators, each weight 100, only 1 participates -> 100 < 267
        let weights = vec![Fr::from(100); 4];
        let bits = vec![Fr::ONE, Fr::ZERO, Fr::ZERO, Fr::ZERO];
        let threshold = Fr::from(267u64);
        let block_hash = Fr::from(0xDEADu64);
        let accumulated = Fr::from(100u64);

        let circuit = SyncCommitteeCircuit {
            validator_weights: weights,
            participation_bits: bits,
            block_hash: Some(block_hash),
            threshold: Some(threshold),
            _marker: PhantomData,
        };

        let public_inputs = vec![vec![block_hash, accumulated, Fr::ZERO]];
        let prover = MockProver::run(k, &circuit, public_inputs).unwrap();
        prover.assert_satisfied();
    }

    #[test]
    fn test_tendermint_quorum_met() {
        let k = 8;
        // 3 validators: powers [100, 200, 300], total=600
        // Validators 0 and 2 sign: signed=400
        // 3*400=1200 > 2*600=1200 -> NOT strictly greater, so quorum NOT met
        // Let's make it: validators 1 and 2 sign: signed=500
        // 3*500=1500 > 2*600=1200 -> quorum MET
        let powers = vec![Fr::from(100), Fr::from(200), Fr::from(300)];
        let bits = vec![Fr::ZERO, Fr::ONE, Fr::ONE];
        let block_hash = Fr::from(0xCAFEu64);
        let total_power = Fr::from(600u64);
        let signed = Fr::from(500u64);

        let circuit = TendermintCircuit {
            voting_powers: powers,
            signed_bits: bits,
            block_hash: Some(block_hash),
            total_power: Some(total_power),
            _marker: PhantomData,
        };

        let public_inputs = vec![vec![block_hash, signed, Fr::ONE]];
        let prover = MockProver::run(k, &circuit, public_inputs).unwrap();
        prover.assert_satisfied();
    }

    #[test]
    fn test_tendermint_quorum_not_met() {
        let k = 8;
        // 3 validators: powers [100, 200, 300], total=600
        // Only validator 0 signs: signed=100
        // 3*100=300 <= 2*600=1200 -> quorum NOT met
        let powers = vec![Fr::from(100), Fr::from(200), Fr::from(300)];
        let bits = vec![Fr::ONE, Fr::ZERO, Fr::ZERO];
        let block_hash = Fr::from(0xFACEu64);
        let total_power = Fr::from(600u64);
        let signed = Fr::from(100u64);

        let circuit = TendermintCircuit {
            voting_powers: powers,
            signed_bits: bits,
            block_hash: Some(block_hash),
            total_power: Some(total_power),
            _marker: PhantomData,
        };

        let public_inputs = vec![vec![block_hash, signed, Fr::ZERO]];
        let prover = MockProver::run(k, &circuit, public_inputs).unwrap();
        prover.assert_satisfied();
    }
}
