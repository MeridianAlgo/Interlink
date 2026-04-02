/// Compliance audit trail for InterLink (Phase 12)
///
/// Immutable, append-only transaction log for regulatory compliance.
/// Every cross-chain transfer is recorded with full context for auditing.
///
/// Features:
///   - Append-only log with sequence numbering
///   - SHA-256 chain linking (each entry hashes the previous)
///   - Query by sender, receiver, chain, time range
///   - Export to CSV/JSON for regulatory reporting
///
/// Comparison:
///   Wormhole:  on-chain VAA log, but no structured audit trail
///   Across:    UMA optimistic oracle records, no compliance export
///   InterLink: structured audit trail with hash-chain integrity + CSV export
use sha2::{Digest, Sha256};
use std::collections::HashMap;

// ─── Types ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferStatus {
    Initiated,
    FinalityConfirmed,
    ProofGenerated,
    Settled,
    Failed { reason: String },
}

impl TransferStatus {
    pub fn as_str(&self) -> &str {
        match self {
            TransferStatus::Initiated => "initiated",
            TransferStatus::FinalityConfirmed => "finality_confirmed",
            TransferStatus::ProofGenerated => "proof_generated",
            TransferStatus::Settled => "settled",
            TransferStatus::Failed { .. } => "failed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuditEntry {
    /// Monotonic sequence number
    pub seq: u64,
    /// SHA-256 hash of the previous entry (zeros for first entry)
    pub prev_hash: [u8; 32],
    /// SHA-256 hash of this entry's content
    pub hash: [u8; 32],
    /// Unix timestamp
    pub timestamp: u64,
    /// Source chain ID
    pub source_chain: u32,
    /// Destination chain ID
    pub dest_chain: u32,
    /// Sender address (hex for EVM, base58 for Solana)
    pub sender: String,
    /// Receiver address
    pub receiver: String,
    /// Transfer amount in smallest denomination (wei, lamports, etc.)
    pub amount: u128,
    /// Token address or "native"
    pub token: String,
    /// Fee charged (in token's smallest denomination)
    pub fee: u128,
    /// Transfer status
    pub status: TransferStatus,
    /// On-chain transaction hash (if available)
    pub tx_hash: Option<String>,
    /// ZK proof commitment (hex)
    pub proof_commitment: Option<String>,
}

impl AuditEntry {
    fn compute_hash(&self) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(self.seq.to_be_bytes());
        h.update(self.prev_hash);
        h.update(self.timestamp.to_be_bytes());
        h.update(self.source_chain.to_be_bytes());
        h.update(self.dest_chain.to_be_bytes());
        h.update(self.sender.as_bytes());
        h.update(self.receiver.as_bytes());
        h.update(self.amount.to_be_bytes());
        h.update(self.token.as_bytes());
        h.update(self.fee.to_be_bytes());
        h.update(self.status.as_str().as_bytes());
        h.finalize().into()
    }
}

// ─── Audit Log ───────────────────────────────────────────────────────────────

pub struct AuditLog {
    entries: Vec<AuditEntry>,
    /// Index: sender → [seq numbers]
    by_sender: HashMap<String, Vec<u64>>,
    /// Index: receiver → [seq numbers]
    by_receiver: HashMap<String, Vec<u64>>,
    /// Index: corridor "src:dst" → [seq numbers]
    by_corridor: HashMap<String, Vec<u64>>,
}

impl AuditLog {
    pub fn new() -> Self {
        AuditLog {
            entries: Vec::new(),
            by_sender: HashMap::new(),
            by_receiver: HashMap::new(),
            by_corridor: HashMap::new(),
        }
    }

    /// Append a new audit entry. Returns the sequence number.
    pub fn append(
        &mut self,
        timestamp: u64,
        source_chain: u32,
        dest_chain: u32,
        sender: impl Into<String>,
        receiver: impl Into<String>,
        amount: u128,
        token: impl Into<String>,
        fee: u128,
        status: TransferStatus,
        tx_hash: Option<String>,
        proof_commitment: Option<String>,
    ) -> u64 {
        let seq = self.entries.len() as u64;
        let prev_hash = self.entries.last().map(|e| e.hash).unwrap_or([0u8; 32]);

        let sender = sender.into();
        let receiver = receiver.into();
        let token = token.into();

        let mut entry = AuditEntry {
            seq,
            prev_hash,
            hash: [0u8; 32], // computed below
            timestamp,
            source_chain,
            dest_chain,
            sender: sender.clone(),
            receiver: receiver.clone(),
            amount,
            token,
            fee,
            status,
            tx_hash,
            proof_commitment,
        };
        entry.hash = entry.compute_hash();

        // Update indices
        self.by_sender.entry(sender).or_default().push(seq);
        self.by_receiver.entry(receiver).or_default().push(seq);
        let corridor = format!("{}:{}", source_chain, dest_chain);
        self.by_corridor.entry(corridor).or_default().push(seq);

        self.entries.push(entry);
        seq
    }

    /// Verify hash-chain integrity. Returns the first broken link, if any.
    pub fn verify_integrity(&self) -> Result<(), IntegrityError> {
        for (i, entry) in self.entries.iter().enumerate() {
            // Check sequence
            if entry.seq != i as u64 {
                return Err(IntegrityError::SequenceGap {
                    expected: i as u64,
                    got: entry.seq,
                });
            }
            // Check prev_hash link
            if i > 0 {
                let prev = &self.entries[i - 1];
                if entry.prev_hash != prev.hash {
                    return Err(IntegrityError::HashMismatch { seq: entry.seq });
                }
            }
            // Recompute hash
            let recomputed = entry.compute_hash();
            if recomputed != entry.hash {
                return Err(IntegrityError::HashTampered { seq: entry.seq });
            }
        }
        Ok(())
    }

    /// Get entry by sequence number.
    pub fn get(&self, seq: u64) -> Option<&AuditEntry> {
        self.entries.get(seq as usize)
    }

    /// Total entries in the log.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    // ── Query ────────────────────────────────────────────────────────────────

    /// All entries by a sender.
    pub fn query_by_sender(&self, sender: &str) -> Vec<&AuditEntry> {
        self.by_sender
            .get(sender)
            .map(|seqs| seqs.iter().filter_map(|s| self.get(*s)).collect())
            .unwrap_or_default()
    }

    /// All entries to a receiver.
    pub fn query_by_receiver(&self, receiver: &str) -> Vec<&AuditEntry> {
        self.by_receiver
            .get(receiver)
            .map(|seqs| seqs.iter().filter_map(|s| self.get(*s)).collect())
            .unwrap_or_default()
    }

    /// All entries in a time range [from, to].
    pub fn query_by_time_range(&self, from: u64, to: u64) -> Vec<&AuditEntry> {
        self.entries
            .iter()
            .filter(|e| e.timestamp >= from && e.timestamp <= to)
            .collect()
    }

    /// All entries for a corridor (e.g., "1:900" for Ethereum → Solana).
    pub fn query_by_corridor(&self, source_chain: u32, dest_chain: u32) -> Vec<&AuditEntry> {
        let key = format!("{source_chain}:{dest_chain}");
        self.by_corridor
            .get(&key)
            .map(|seqs| seqs.iter().filter_map(|s| self.get(*s)).collect())
            .unwrap_or_default()
    }

    // ── Export ────────────────────────────────────────────────────────────────

    /// Export to CSV format for regulatory reporting.
    pub fn export_csv(&self) -> String {
        let mut out = String::from(
            "seq,timestamp,source_chain,dest_chain,sender,receiver,amount,token,fee,status,tx_hash\n",
        );
        for e in &self.entries {
            let tx = e.tx_hash.as_deref().unwrap_or("");
            out.push_str(&format!(
                "{},{},{},{},{},{},{},{},{},{},{}\n",
                e.seq,
                e.timestamp,
                e.source_chain,
                e.dest_chain,
                e.sender,
                e.receiver,
                e.amount,
                e.token,
                e.fee,
                e.status.as_str(),
                tx,
            ));
        }
        out
    }

    /// Export to JSON array for API consumers.
    pub fn export_json(&self) -> serde_json::Value {
        let entries: Vec<serde_json::Value> = self
            .entries
            .iter()
            .map(|e| {
                let hash_hex: String = e.hash.iter().map(|b| format!("{b:02x}")).collect();
                serde_json::json!({
                    "seq": e.seq,
                    "hash": hash_hex,
                    "timestamp": e.timestamp,
                    "source_chain": e.source_chain,
                    "dest_chain": e.dest_chain,
                    "sender": e.sender,
                    "receiver": e.receiver,
                    "amount": e.amount.to_string(),
                    "token": e.token,
                    "fee": e.fee.to_string(),
                    "status": e.status.as_str(),
                    "tx_hash": e.tx_hash,
                })
            })
            .collect();
        serde_json::json!({
            "total": self.entries.len(),
            "entries": entries,
        })
    }
}

impl Default for AuditLog {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
pub enum IntegrityError {
    SequenceGap { expected: u64, got: u64 },
    HashMismatch { seq: u64 },
    HashTampered { seq: u64 },
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_log() -> AuditLog {
        let mut log = AuditLog::new();
        log.append(
            1000,
            1,
            900,
            "0xAlice",
            "SolBob",
            1_000_000_000_000_000_000u128,
            "native",
            0,
            TransferStatus::Settled,
            Some("0xtx1".into()),
            Some("0xproof1".into()),
        );
        log.append(
            1100,
            1,
            900,
            "0xCharlie",
            "SolBob",
            500_000_000_000_000_000u128,
            "native",
            250_000_000_000_000u128,
            TransferStatus::Settled,
            Some("0xtx2".into()),
            None,
        );
        log.append(
            1200,
            137,
            1,
            "0xDave",
            "0xAlice",
            100_000_000u128,
            "0xUSDC",
            50_000u128,
            TransferStatus::Initiated,
            None,
            None,
        );
        log
    }

    #[test]
    fn test_append_and_len() {
        let log = sample_log();
        assert_eq!(log.len(), 3);
        assert!(!log.is_empty());
    }

    #[test]
    fn test_sequence_numbers() {
        let log = sample_log();
        assert_eq!(log.get(0).unwrap().seq, 0);
        assert_eq!(log.get(1).unwrap().seq, 1);
        assert_eq!(log.get(2).unwrap().seq, 2);
    }

    #[test]
    fn test_hash_chain_integrity() {
        let log = sample_log();
        assert!(log.verify_integrity().is_ok());
    }

    #[test]
    fn test_first_entry_has_zero_prev_hash() {
        let log = sample_log();
        assert_eq!(log.get(0).unwrap().prev_hash, [0u8; 32]);
    }

    #[test]
    fn test_prev_hash_links() {
        let log = sample_log();
        let e0 = log.get(0).unwrap();
        let e1 = log.get(1).unwrap();
        assert_eq!(e1.prev_hash, e0.hash);
    }

    #[test]
    fn test_query_by_sender() {
        let log = sample_log();
        let alice_txs = log.query_by_sender("0xAlice");
        assert_eq!(alice_txs.len(), 1);
        assert_eq!(alice_txs[0].seq, 0);
    }

    #[test]
    fn test_query_by_receiver() {
        let log = sample_log();
        let bob_txs = log.query_by_receiver("SolBob");
        assert_eq!(bob_txs.len(), 2); // entries 0 and 1
    }

    #[test]
    fn test_query_by_time_range() {
        let log = sample_log();
        let range = log.query_by_time_range(1050, 1150);
        assert_eq!(range.len(), 1);
        assert_eq!(range[0].seq, 1);
    }

    #[test]
    fn test_query_by_corridor() {
        let log = sample_log();
        let eth_to_sol = log.query_by_corridor(1, 900);
        assert_eq!(eth_to_sol.len(), 2);
        let poly_to_eth = log.query_by_corridor(137, 1);
        assert_eq!(poly_to_eth.len(), 1);
    }

    #[test]
    fn test_csv_export() {
        let log = sample_log();
        let csv = log.export_csv();
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 4); // header + 3 entries
        assert!(lines[0].starts_with("seq,timestamp"));
        assert!(lines[1].contains("0xAlice"));
    }

    #[test]
    fn test_json_export() {
        let log = sample_log();
        let j = log.export_json();
        assert_eq!(j["total"], 3);
        let entries = j["entries"].as_array().unwrap();
        assert_eq!(entries[0]["sender"], "0xAlice");
        assert_eq!(entries[2]["status"], "initiated");
    }

    #[test]
    fn test_failed_transfer_recorded() {
        let mut log = AuditLog::new();
        log.append(
            500,
            1,
            900,
            "0xBad",
            "SolTarget",
            100,
            "native",
            0,
            TransferStatus::Failed {
                reason: "proof timeout".into(),
            },
            None,
            None,
        );
        let e = log.get(0).unwrap();
        assert_eq!(e.status.as_str(), "failed");
    }

    #[test]
    fn test_empty_log_integrity() {
        let log = AuditLog::new();
        assert!(log.verify_integrity().is_ok());
    }

    #[test]
    fn test_hash_determinism() {
        let mut log1 = AuditLog::new();
        let mut log2 = AuditLog::new();
        log1.append(
            100,
            1,
            900,
            "A",
            "B",
            1000,
            "native",
            0,
            TransferStatus::Settled,
            None,
            None,
        );
        log2.append(
            100,
            1,
            900,
            "A",
            "B",
            1000,
            "native",
            0,
            TransferStatus::Settled,
            None,
            None,
        );
        assert_eq!(log1.get(0).unwrap().hash, log2.get(0).unwrap().hash);
    }
}
