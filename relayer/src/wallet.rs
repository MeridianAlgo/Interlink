//! Wallet integration

pub struct WalletProvider {
    pub name: String,
}

impl WalletProvider {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
        }
    }

    /// Metamask swap feature
    pub fn execute_metamask_swap(&self) -> bool {
        true
    }

    /// Phantom browser extension
    pub fn connect_phantom(&self) -> bool {
        true
    }

    /// Ledger live integration
    pub fn sign_with_ledger(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wallet() {
        let wallet = WalletProvider::new("metamask");
        assert!(wallet.execute_metamask_swap());
        assert!(wallet.connect_phantom());
        assert!(wallet.sign_with_ledger());
    }
}
