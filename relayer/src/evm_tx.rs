//! EIP-1559 transaction encoding and signing (secp256k1 via `k256`).

use k256::ecdsa::SigningKey;

/// RLP-encode a byte string.
pub fn rlp_bytes(b: &[u8]) -> Vec<u8> {
    match b.len() {
        1 if b[0] < 0x80 => vec![b[0]],
        len if len <= 55 => [&[0x80 + len as u8][..], b].concat(),
        len => {
            let len_bytes = trim_leading_zeros(&(len as u64).to_be_bytes()).to_vec();
            [&[0xb7 + len_bytes.len() as u8][..], &len_bytes, b].concat()
        }
    }
}

/// RLP-encode an unsigned integer given as big-endian bytes (minimal form).
pub fn rlp_uint(be: &[u8]) -> Vec<u8> {
    rlp_bytes(trim_leading_zeros(be))
}

pub fn rlp_u128(v: u128) -> Vec<u8> {
    rlp_uint(&v.to_be_bytes())
}

/// RLP-encode a list of already-encoded items.
pub fn rlp_list(items: &[Vec<u8>]) -> Vec<u8> {
    let payload: Vec<u8> = items.concat();
    let mut out = match payload.len() {
        len if len <= 55 => vec![0xc0 + len as u8],
        len => {
            let len_bytes = trim_leading_zeros(&(len as u64).to_be_bytes()).to_vec();
            [&[0xf7 + len_bytes.len() as u8][..], &len_bytes].concat()
        }
    };
    out.extend_from_slice(&payload);
    out
}

fn trim_leading_zeros(b: &[u8]) -> &[u8] {
    let first = b.iter().position(|x| *x != 0).unwrap_or(b.len());
    &b[first..]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Eip1559Tx {
    pub chain_id: u64,
    pub nonce: u64,
    pub max_priority_fee_per_gas: u128,
    pub max_fee_per_gas: u128,
    pub gas_limit: u64,
    pub to: [u8; 20],
    pub value: u128,
    pub data: Vec<u8>,
}

impl Eip1559Tx {
    fn fields(&self) -> Vec<Vec<u8>> {
        vec![
            rlp_u128(self.chain_id as u128),
            rlp_u128(self.nonce as u128),
            rlp_u128(self.max_priority_fee_per_gas),
            rlp_u128(self.max_fee_per_gas),
            rlp_u128(self.gas_limit as u128),
            rlp_bytes(&self.to),
            rlp_u128(self.value),
            rlp_bytes(&self.data),
            rlp_list(&[]), // empty access list
        ]
    }

    /// keccak256(0x02 || rlp([chainId, nonce, ..., accessList]))
    pub fn signing_hash(&self) -> [u8; 32] {
        let mut buf = vec![0x02];
        buf.extend_from_slice(&rlp_list(&self.fields()));
        crate::evm::keccak256(&buf)
    }

    /// Signed raw transaction: 0x02 || rlp([..fields, yParity, r, s]).
    pub fn sign(&self, key: &SigningKey) -> Result<Vec<u8>, String> {
        let (sig, recovery) = key
            .sign_prehash_recoverable(&self.signing_hash())
            .map_err(|e| format!("signing failed: {e}"))?;
        let bytes = sig.to_bytes();
        let mut fields = self.fields();
        fields.push(rlp_u128(recovery.is_y_odd() as u128));
        fields.push(rlp_uint(&bytes[..32]));
        fields.push(rlp_uint(&bytes[32..]));
        let mut raw = vec![0x02];
        raw.extend_from_slice(&rlp_list(&fields));
        Ok(raw)
    }
}

/// Ethereum address of a signing key: keccak256(uncompressed pubkey)[12..].
pub fn address(key: &SigningKey) -> [u8; 20] {
    let point = key.verifying_key().to_encoded_point(false);
    crate::evm::keccak256(&point.as_bytes()[1..])[12..]
        .try_into()
        .unwrap()
}

pub fn signing_key_from_hex(hex: &str) -> Result<SigningKey, String> {
    let bytes = crate::evm::from_hex(hex.trim())?;
    SigningKey::from_slice(&bytes).map_err(|_| "invalid secp256k1 private key".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rlp_spec_vectors() {
        // Examples from the Ethereum RLP specification.
        assert_eq!(rlp_bytes(b"dog"), vec![0x83, b'd', b'o', b'g']);
        assert_eq!(
            rlp_list(&[rlp_bytes(b"cat"), rlp_bytes(b"dog")]),
            vec![0xc8, 0x83, b'c', b'a', b't', 0x83, b'd', b'o', b'g']
        );
        assert_eq!(rlp_bytes(b""), vec![0x80]);
        assert_eq!(rlp_list(&[]), vec![0xc0]);
        assert_eq!(rlp_u128(0), vec![0x80]);
        assert_eq!(rlp_u128(15), vec![0x0f]);
        assert_eq!(rlp_u128(1024), vec![0x82, 0x04, 0x00]);
        let lorem = b"Lorem ipsum dolor sit amet, consectetur adipisicing elit";
        assert_eq!(&rlp_bytes(lorem)[..2], &[0xb8, 0x38]);
    }

    #[test]
    fn known_private_key_address() {
        // Private key 1 → the well-known address of the secp256k1 generator.
        let mut k = [0u8; 32];
        k[31] = 1;
        let key = SigningKey::from_slice(&k).unwrap();
        assert_eq!(
            crate::evm::to_hex(&address(&key)),
            "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf"
        );
    }

    /// Cross-check encoding and signature against ethers-core (dev-dependency).
    #[test]
    fn matches_ethers_encoding_and_recovers_sender() {
        use ethers_core::types::transaction::eip2718::TypedTransaction;
        use ethers_core::types::{Eip1559TransactionRequest, Signature, H160, U256};

        let key = signing_key_from_hex(
            "0x4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318",
        )
        .unwrap();
        let tx = Eip1559Tx {
            chain_id: 1,
            nonce: 42,
            max_priority_fee_per_gas: 1_500_000_000,
            max_fee_per_gas: 80_000_000_000,
            gas_limit: 350_000,
            to: [0xAB; 20],
            value: 0,
            data: (0..300u16).map(|i| i as u8).collect(),
        };

        let ethers_tx: TypedTransaction = Eip1559TransactionRequest::new()
            .chain_id(1u64)
            .nonce(42u64)
            .max_priority_fee_per_gas(1_500_000_000u64)
            .max_fee_per_gas(80_000_000_000u64)
            .gas(350_000u64)
            .to(H160::from([0xAB; 20]))
            .value(0u64)
            .data(tx.data.clone())
            .into();
        assert_eq!(tx.signing_hash(), ethers_tx.sighash().0);

        let raw = tx.sign(&key).unwrap();
        let (sig, rec) = key.sign_prehash_recoverable(&tx.signing_hash()).unwrap();
        let b = sig.to_bytes();
        let ethers_sig = Signature {
            r: U256::from_big_endian(&b[..32]),
            s: U256::from_big_endian(&b[32..]),
            v: rec.is_y_odd() as u64,
        };
        assert_eq!(raw, ethers_tx.rlp_signed(&ethers_sig).to_vec());

        let recovered = ethers_sig.recover(ethers_tx.sighash()).unwrap();
        assert_eq!(recovered.0, address(&key));
    }
}
