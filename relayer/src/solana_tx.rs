//! Minimal Solana transaction building: account metas, legacy message
//! compilation, signing, PDA / ATA derivation and Anchor discriminators.
//!
//! Replaces hand-assembled byte layouts so every hub instruction goes
//! through one tested compiler.

use ed25519_dalek::Signer;
use sha2::{Digest, Sha256};

pub type Pubkey = [u8; 32];

pub const SYSTEM_PROGRAM: Pubkey = [0u8; 32];

pub fn pubkey(b58: &str) -> Result<Pubkey, String> {
    bs58::decode(b58)
        .into_vec()
        .map_err(|e| format!("invalid base58 pubkey {b58}: {e}"))?
        .try_into()
        .map_err(|_| format!("pubkey {b58} is not 32 bytes"))
}

pub fn token_program() -> Pubkey {
    pubkey("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap()
}

pub fn associated_token_program() -> Pubkey {
    pubkey("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL").unwrap()
}

pub fn compute_budget_program() -> Pubkey {
    pubkey("ComputeBudget111111111111111111111111111111").unwrap()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountMeta {
    pub pubkey: Pubkey,
    pub is_signer: bool,
    pub is_writable: bool,
}

impl AccountMeta {
    pub fn signer(pubkey: Pubkey) -> Self {
        Self {
            pubkey,
            is_signer: true,
            is_writable: true,
        }
    }
    pub fn writable(pubkey: Pubkey) -> Self {
        Self {
            pubkey,
            is_signer: false,
            is_writable: true,
        }
    }
    pub fn readonly(pubkey: Pubkey) -> Self {
        Self {
            pubkey,
            is_signer: false,
            is_writable: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Instruction {
    pub program_id: Pubkey,
    pub accounts: Vec<AccountMeta>,
    pub data: Vec<u8>,
}

/// `sha256("global:<name>")[..8]`
pub fn anchor_ix_discriminator(name: &str) -> [u8; 8] {
    Sha256::digest(format!("global:{name}").as_bytes())[..8]
        .try_into()
        .unwrap()
}

/// `sha256("event:<Name>")[..8]`
pub fn anchor_event_discriminator(name: &str) -> [u8; 8] {
    Sha256::digest(format!("event:{name}").as_bytes())[..8]
        .try_into()
        .unwrap()
}

pub fn set_compute_unit_limit(units: u32) -> Instruction {
    let mut data = vec![2u8];
    data.extend_from_slice(&units.to_le_bytes());
    Instruction {
        program_id: compute_budget_program(),
        accounts: vec![],
        data,
    }
}

fn is_on_curve(bytes: &Pubkey) -> bool {
    ed25519_dalek::VerifyingKey::from_bytes(bytes).is_ok()
}

/// Solana `find_program_address`: highest bump whose hash is off the ed25519 curve.
pub fn find_program_address(seeds: &[&[u8]], program_id: &Pubkey) -> Result<(Pubkey, u8), String> {
    for bump in (0..=255u8).rev() {
        let mut hasher = Sha256::new();
        for seed in seeds {
            hasher.update(seed);
        }
        hasher.update([bump]);
        hasher.update(program_id);
        hasher.update(b"ProgramDerivedAddress");
        let candidate: Pubkey = hasher.finalize().into();
        if !is_on_curve(&candidate) {
            return Ok((candidate, bump));
        }
    }
    Err("no viable bump for PDA".into())
}

pub fn pda(seeds: &[&[u8]], program_id: &Pubkey) -> Result<Pubkey, String> {
    find_program_address(seeds, program_id).map(|(k, _)| k)
}

pub fn associated_token_address(owner: &Pubkey, mint: &Pubkey) -> Result<Pubkey, String> {
    pda(
        &[owner, &token_program(), mint],
        &associated_token_program(),
    )
}

fn compact_u16(n: usize, out: &mut Vec<u8>) {
    let mut val = n;
    loop {
        let byte = (val & 0x7F) as u8;
        val >>= 7;
        if val == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// Compile a legacy message. Keys are ordered writable signers (fee payer
/// first), readonly signers, writable non-signers, readonly non-signers.
pub fn compile_message(
    payer: &Pubkey,
    ixs: &[Instruction],
    blockhash: &[u8; 32],
) -> Result<Vec<u8>, String> {
    let mut keys: Vec<AccountMeta> = vec![AccountMeta::signer(*payer)];
    let mut add = |meta: AccountMeta| match keys.iter_mut().find(|k| k.pubkey == meta.pubkey) {
        Some(k) => {
            k.is_signer |= meta.is_signer;
            k.is_writable |= meta.is_writable;
        }
        None => keys.push(meta),
    };
    for ix in ixs {
        for meta in &ix.accounts {
            add(meta.clone());
        }
        add(AccountMeta::readonly(ix.program_id));
    }

    let rank = |k: &AccountMeta| match (k.is_signer, k.is_writable) {
        (true, true) => 0,
        (true, false) => 1,
        (false, true) => 2,
        (false, false) => 3,
    };
    // Stable sort keeps the payer first and preserves first-seen order otherwise.
    keys.sort_by_key(rank);
    if keys.len() > 255 {
        return Err("too many accounts".into());
    }

    let num_signers = keys.iter().filter(|k| k.is_signer).count();
    let readonly_signed = keys
        .iter()
        .filter(|k| k.is_signer && !k.is_writable)
        .count();
    let readonly_unsigned = keys
        .iter()
        .filter(|k| !k.is_signer && !k.is_writable)
        .count();
    if num_signers != 1 {
        return Err("only single-signer transactions are supported".into());
    }

    let index = |k: &Pubkey| keys.iter().position(|m| &m.pubkey == k).unwrap() as u8;
    let mut msg = vec![
        num_signers as u8,
        readonly_signed as u8,
        readonly_unsigned as u8,
    ];
    compact_u16(keys.len(), &mut msg);
    for k in &keys {
        msg.extend_from_slice(&k.pubkey);
    }
    msg.extend_from_slice(blockhash);
    compact_u16(ixs.len(), &mut msg);
    for ix in ixs {
        msg.push(index(&ix.program_id));
        compact_u16(ix.accounts.len(), &mut msg);
        for meta in &ix.accounts {
            msg.push(index(&meta.pubkey));
        }
        compact_u16(ix.data.len(), &mut msg);
        msg.extend_from_slice(&ix.data);
    }
    Ok(msg)
}

/// Sign a compiled message with a 64-byte Solana keypair (secret || public).
pub fn sign_transaction(message: &[u8], keypair: &[u8]) -> Result<Vec<u8>, String> {
    let secret: [u8; 32] = keypair
        .get(..32)
        .and_then(|s| s.try_into().ok())
        .ok_or("keypair must be at least 32 bytes")?;
    let signature = ed25519_dalek::SigningKey::from_bytes(&secret).sign(message);
    let mut tx = Vec::with_capacity(1 + 64 + message.len());
    tx.push(1u8);
    tx.extend_from_slice(&signature.to_bytes());
    tx.extend_from_slice(message);
    if tx.len() > 1232 {
        return Err(format!("transaction too large: {} bytes", tx.len()));
    }
    Ok(tx)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference addresses from `solana find-program-derived-address` (Solana CLI 3.0).
    #[test]
    fn pdas_match_solana_cli() {
        let hub = pubkey("AKzpc9tvxfhLjj5AantKizK2YZgSjoyhLmYqRZy6b8Lz").unwrap();
        let b58 = |k: Pubkey| bs58::encode(k).into_string();
        assert_eq!(
            b58(pda(&[b"state"], &hub).unwrap()),
            "9jrUnkhcR9vth2uwehuNLeq6pHTecmvd1KZXQoHUFuQo"
        );
        assert_eq!(
            b58(pda(&[b"message", &[7u8; 32]], &hub).unwrap()),
            "3zv12gFVME1Qq1vr8TFA55WKSzTx8LQVuL3k81nnNkpn"
        );
        assert_eq!(
            b58(pda(&[b"wrapped", &1u64.to_le_bytes(), &[0x33u8; 32]], &hub).unwrap()),
            "6UUFzVFrYDBEJM673ysscefmpy6vtdh5FmaTjw4YEkoq"
        );
        let wsol = pubkey("So11111111111111111111111111111111111111112").unwrap();
        assert_eq!(
            b58(associated_token_address(&hub, &wsol).unwrap()),
            "BEeCLUVgjcSFsSLomXrUneya4GApyPeMzvpcPfM6VJUN"
        );
    }

    #[test]
    fn known_anchor_discriminators() {
        // sha256("global:initialize")[..8] is Anchor's well-known initialize discriminator.
        assert_eq!(
            anchor_ix_discriminator("initialize"),
            [175, 175, 109, 31, 13, 152, 155, 237]
        );
    }

    #[test]
    fn compact_u16_encoding() {
        let enc = |n| {
            let mut v = vec![];
            compact_u16(n, &mut v);
            v
        };
        assert_eq!(enc(0), vec![0]);
        assert_eq!(enc(127), vec![0x7F]);
        assert_eq!(enc(128), vec![0x80, 0x01]);
        assert_eq!(enc(492), vec![0xEC, 0x03]);
    }

    #[test]
    fn message_orders_and_dedups_accounts() {
        let payer = [1u8; 32];
        let program = [9u8; 32];
        let w = [2u8; 32];
        let r = [3u8; 32];
        let ix = Instruction {
            program_id: program,
            // r listed first, then w twice with different flags: must merge to writable.
            accounts: vec![
                AccountMeta::readonly(r),
                AccountMeta::readonly(w),
                AccountMeta::writable(w),
                AccountMeta::signer(payer),
            ],
            data: vec![0xAA],
        };
        let msg = compile_message(&payer, &[ix], &[7u8; 32]).unwrap();
        // header: 1 signer, 0 readonly signed, 2 readonly unsigned (r, program)
        assert_eq!(&msg[..3], &[1, 0, 2]);
        assert_eq!(msg[3], 4); // key count
        let key = |i: usize| &msg[4 + 32 * i..4 + 32 * (i + 1)];
        assert_eq!(key(0), &payer);
        assert_eq!(key(1), &w); // writable non-signer
        assert_eq!(key(2), &r);
        assert_eq!(key(3), &program);
        let ix_start = 4 + 32 * 4 + 32;
        assert_eq!(msg[ix_start], 1); // one instruction
        assert_eq!(msg[ix_start + 1], 3); // program index
        assert_eq!(&msg[ix_start + 2..ix_start + 7], &[4, 2, 1, 1, 0]);
    }
}
