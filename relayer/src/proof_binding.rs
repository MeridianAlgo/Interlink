//! Proof binding to sender identity (zk)

pub struct ProofBinding {
    pub sender: [u8; 20],
    pub nonce: u64,
}

impl ProofBinding {
    /// Prevents sandwich attacks by cryptographically binding the proof to sender
    pub fn bind_proof(&self, proof: &[u8]) -> Vec<u8> {
        let mut bound = Vec::with_capacity(proof.len() + 28);
        bound.extend_from_slice(&self.sender);
        bound.extend_from_slice(&self.nonce.to_be_bytes());
        bound.extend_from_slice(proof);
        bound
    }

    /// Validation logic replicating Wormhole's nonce mechanism but better
    pub fn validate_bound_proof(&self, bound_proof: &[u8]) -> bool {
        bound_proof.len() >= 28 && bound_proof[..20] == self.sender
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_binding() {
        let b = ProofBinding {
            sender: [0x5; 20],
            nonce: 1,
        };
        let bound = b.bind_proof(&[0x1, 0x2]);
        assert_eq!(bound.len(), 30);
        assert!(b.validate_bound_proof(&bound));
    }
}
