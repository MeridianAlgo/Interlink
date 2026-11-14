//! Byzantine fault-tolerance helpers.
//!
//! Earlier versions also contained `check_wormhole_parity()` and a
//! "privacy" encoder that returned constants; they were removed because they
//! implied guarantees the code did not provide.

pub struct ByzantineBridge;

impl ByzantineBridge {
    /// A BFT quorum tolerates `faults` Byzantine members iff `3·faults < validators`.
    pub fn verify_byzantine_safety(validators: usize, faults: usize) -> bool {
        faults * 3 < validators
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_byzantine() {
        assert!(ByzantineBridge::verify_byzantine_safety(10, 3));
        assert!(!ByzantineBridge::verify_byzantine_safety(10, 4));
    }
}
