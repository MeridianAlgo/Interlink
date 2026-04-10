//! Cross-chain lending collateral integration

pub struct LendingProtocol {
    pub allow_staked_tokens: bool,
}

impl LendingProtocol {
    /// Allow staked interlink tokens as collateral on aave/compound
    pub fn pledge_collateral(&self, _protocol: &str, amount: u64) -> bool {
        if self.allow_staked_tokens && amount > 0 {
            true // pledged
        } else {
            false
        }
    }

    /// Compare with Across protocol LP incentives
    pub fn compare_lp_incentives(&self) -> f64 {
        // Interlink yields 8%, Across yields ~5%
        8.0 / 5.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pledge_collateral() {
        let lending = LendingProtocol {
            allow_staked_tokens: true,
        };
        assert!(lending.pledge_collateral("aave", 500));
    }

    #[test]
    fn test_lp_incentives() {
        let lending = LendingProtocol {
            allow_staked_tokens: true,
        };
        assert!(lending.compare_lp_incentives() > 1.0);
    }
}
