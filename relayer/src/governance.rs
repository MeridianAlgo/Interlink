/// DAO governance module for $INTERLINK token
///
/// Implements on-chain governance: proposal creation, voting (token-weighted),
/// execution delay (timelock), and treasury management.
///
/// Comparison:
///   - Wormhole: off-chain guardian multisig, no public voting
///   - Stargate: STG token voting via Snapshot (off-chain)
///   - Across: UMA Optimistic Oracle governance
///   - InterLink: on-chain weighted voting + timelock (beats all three)
use std::collections::HashMap;

// ─── Token Supply ───────────────────────────────────────────────────────────

/// Total $INTERLINK supply: 1,000,000,000 tokens (1B)
pub const TOTAL_SUPPLY: u64 = 1_000_000_000;
/// Community allocation: 40% of total supply
pub const COMMUNITY_ALLOC: u64 = 400_000_000;
/// Team allocation: 30% of total supply (4-year vest)
pub const TEAM_ALLOC: u64 = 300_000_000;
/// Treasury allocation: 30% of total supply
pub const TREASURY_ALLOC: u64 = 300_000_000;

// ─── Governance Parameters ───────────────────────────────────────────────────

/// Minimum tokens required to create a proposal (0.01% of supply = 100k)
pub const PROPOSAL_THRESHOLD: u64 = 100_000;
/// Voting period: 7 days in seconds
pub const VOTING_PERIOD_SECS: u64 = 7 * 24 * 3600;
/// Timelock delay before execution: 2 days
pub const TIMELOCK_DELAY_SECS: u64 = 2 * 24 * 3600;
/// Quorum: 4% of total supply must vote (40M tokens)
pub const QUORUM_TOKENS: u64 = 40_000_000;

// ─── Types ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProposalStatus {
    Pending,
    Active,
    Succeeded,
    Defeated,
    Queued,
    Executed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProposalKind {
    /// Update fee tier parameters
    UpdateFees,
    /// Add a new supported chain
    AddChain,
    /// Update the validator set (threshold/members)
    UpdateValidatorSet,
    /// Allocate treasury funds
    TreasuryAllocation,
    /// Update protocol parameters (quorum, voting period, etc.)
    UpdateParameters,
    /// Generic text proposal
    Text,
}

#[derive(Debug, Clone)]
pub struct Proposal {
    pub id: u64,
    pub kind: ProposalKind,
    pub title: String,
    pub description: String,
    pub proposer: String,
    /// Unix timestamp when voting starts
    pub start_time: u64,
    /// Unix timestamp when voting ends
    pub end_time: u64,
    /// Earliest execution timestamp (end_time + timelock)
    pub eta: u64,
    pub votes_for: u64,
    pub votes_against: u64,
    pub votes_abstain: u64,
    pub status: ProposalStatus,
    /// Encoded calldata for on-chain execution (if applicable)
    pub calldata: Vec<u8>,
}

impl Proposal {
    pub fn new(
        id: u64,
        kind: ProposalKind,
        title: impl Into<String>,
        description: impl Into<String>,
        proposer: impl Into<String>,
        now: u64,
    ) -> Self {
        let start_time = now;
        let end_time = now + VOTING_PERIOD_SECS;
        let eta = end_time + TIMELOCK_DELAY_SECS;
        Proposal {
            id,
            kind,
            title: title.into(),
            description: description.into(),
            proposer: proposer.into(),
            start_time,
            end_time,
            eta,
            votes_for: 0,
            votes_against: 0,
            votes_abstain: 0,
            status: ProposalStatus::Active,
            calldata: vec![],
        }
    }

    pub fn total_votes(&self) -> u64 {
        self.votes_for
            .saturating_add(self.votes_against)
            .saturating_add(self.votes_abstain)
    }

    pub fn quorum_reached(&self) -> bool {
        self.total_votes() >= QUORUM_TOKENS
    }

    /// Finalise status based on votes. Call after `end_time` has passed.
    pub fn finalize(&mut self) {
        if self.status != ProposalStatus::Active {
            return;
        }
        if !self.quorum_reached() || self.votes_for <= self.votes_against {
            self.status = ProposalStatus::Defeated;
        } else {
            self.status = ProposalStatus::Succeeded;
        }
    }
}

// ─── Vote ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoteChoice {
    For,
    Against,
    Abstain,
}

#[derive(Debug, Clone)]
pub struct Vote {
    pub voter: String,
    pub weight: u64,
    pub choice: VoteChoice,
}

// ─── Treasury ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Treasury {
    /// Token balance in the treasury
    pub balance_tokens: u64,
    /// Disbursements: (recipient, amount, description)
    pub history: Vec<(String, u64, String)>,
}

impl Treasury {
    pub fn new(initial: u64) -> Self {
        Treasury {
            balance_tokens: initial,
            history: vec![],
        }
    }

    /// Disburse `amount` tokens to `recipient` for `reason`.
    pub fn disburse(
        &mut self,
        recipient: impl Into<String>,
        amount: u64,
        reason: impl Into<String>,
    ) -> Result<(), GovernanceError> {
        if amount > self.balance_tokens {
            return Err(GovernanceError::InsufficientTreasuryBalance {
                requested: amount,
                available: self.balance_tokens,
            });
        }
        self.balance_tokens -= amount;
        self.history.push((recipient.into(), amount, reason.into()));
        Ok(())
    }
}

// ─── Governance State ────────────────────────────────────────────────────────

pub struct Governance {
    pub proposals: HashMap<u64, Proposal>,
    /// Votes per proposal: proposal_id → Vec<Vote>
    pub votes: HashMap<u64, Vec<Vote>>,
    pub treasury: Treasury,
    next_proposal_id: u64,
}

impl Governance {
    pub fn new() -> Self {
        Governance {
            proposals: HashMap::new(),
            votes: HashMap::new(),
            treasury: Treasury::new(TREASURY_ALLOC),
            next_proposal_id: 1,
        }
    }

    /// Submit a new governance proposal.
    pub fn propose(
        &mut self,
        kind: ProposalKind,
        title: impl Into<String>,
        description: impl Into<String>,
        proposer: impl Into<String>,
        proposer_tokens: u64,
        now: u64,
    ) -> Result<u64, GovernanceError> {
        if proposer_tokens < PROPOSAL_THRESHOLD {
            return Err(GovernanceError::BelowProposalThreshold {
                have: proposer_tokens,
                need: PROPOSAL_THRESHOLD,
            });
        }
        let id = self.next_proposal_id;
        self.next_proposal_id += 1;
        let proposal = Proposal::new(id, kind, title, description, proposer, now);
        self.proposals.insert(id, proposal);
        self.votes.insert(id, vec![]);
        Ok(id)
    }

    /// Cast a vote on an active proposal.
    pub fn vote(
        &mut self,
        proposal_id: u64,
        voter: impl Into<String>,
        weight: u64,
        choice: VoteChoice,
        now: u64,
    ) -> Result<(), GovernanceError> {
        let voter = voter.into();
        let proposal = self
            .proposals
            .get_mut(&proposal_id)
            .ok_or(GovernanceError::ProposalNotFound { id: proposal_id })?;

        if now < proposal.start_time || now > proposal.end_time {
            return Err(GovernanceError::VotingNotActive { id: proposal_id });
        }
        if proposal.status != ProposalStatus::Active {
            return Err(GovernanceError::VotingNotActive { id: proposal_id });
        }

        // Check for duplicate vote
        let votes = self.votes.get(&proposal_id).unwrap();
        if votes.iter().any(|v| v.voter == voter) {
            return Err(GovernanceError::AlreadyVoted {
                voter: voter.clone(),
            });
        }

        match &choice {
            VoteChoice::For => proposal.votes_for = proposal.votes_for.saturating_add(weight),
            VoteChoice::Against => {
                proposal.votes_against = proposal.votes_against.saturating_add(weight)
            }
            VoteChoice::Abstain => {
                proposal.votes_abstain = proposal.votes_abstain.saturating_add(weight)
            }
        }
        self.votes.get_mut(&proposal_id).unwrap().push(Vote {
            voter,
            weight,
            choice,
        });
        Ok(())
    }

    /// Finalize a proposal after its voting period.
    pub fn finalize(
        &mut self,
        proposal_id: u64,
        now: u64,
    ) -> Result<ProposalStatus, GovernanceError> {
        let proposal = self
            .proposals
            .get_mut(&proposal_id)
            .ok_or(GovernanceError::ProposalNotFound { id: proposal_id })?;
        if now < proposal.end_time {
            return Err(GovernanceError::VotingStillActive {
                ends_at: proposal.end_time,
            });
        }
        proposal.finalize();
        Ok(proposal.status.clone())
    }

    /// Queue a succeeded proposal for execution (starts timelock).
    pub fn queue(&mut self, proposal_id: u64) -> Result<(), GovernanceError> {
        let proposal = self
            .proposals
            .get_mut(&proposal_id)
            .ok_or(GovernanceError::ProposalNotFound { id: proposal_id })?;
        if proposal.status != ProposalStatus::Succeeded {
            return Err(GovernanceError::NotSucceeded { id: proposal_id });
        }
        proposal.status = ProposalStatus::Queued;
        Ok(())
    }

    /// Execute a queued proposal after its timelock has elapsed.
    pub fn execute(&mut self, proposal_id: u64, now: u64) -> Result<(), GovernanceError> {
        let proposal = self
            .proposals
            .get_mut(&proposal_id)
            .ok_or(GovernanceError::ProposalNotFound { id: proposal_id })?;
        if proposal.status != ProposalStatus::Queued {
            return Err(GovernanceError::NotQueued { id: proposal_id });
        }
        if now < proposal.eta {
            return Err(GovernanceError::TimelockNotExpired {
                available_at: proposal.eta,
            });
        }
        proposal.status = ProposalStatus::Executed;
        Ok(())
    }

    pub fn active_proposals(&self) -> Vec<&Proposal> {
        self.proposals
            .values()
            .filter(|p| p.status == ProposalStatus::Active)
            .collect()
    }
}

impl Default for Governance {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
pub enum GovernanceError {
    BelowProposalThreshold { have: u64, need: u64 },
    ProposalNotFound { id: u64 },
    VotingNotActive { id: u64 },
    VotingStillActive { ends_at: u64 },
    AlreadyVoted { voter: String },
    NotSucceeded { id: u64 },
    NotQueued { id: u64 },
    TimelockNotExpired { available_at: u64 },
    InsufficientTreasuryBalance { requested: u64, available: u64 },
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn gov() -> Governance {
        Governance::new()
    }

    #[test]
    fn test_token_supply_constants() {
        assert_eq!(COMMUNITY_ALLOC + TEAM_ALLOC + TREASURY_ALLOC, TOTAL_SUPPLY);
    }

    #[test]
    fn test_propose_success() {
        let mut g = gov();
        let id = g
            .propose(
                ProposalKind::UpdateFees,
                "Lower Tier2 fee",
                "Reduce tier2 from 0.05% to 0.03%",
                "alice",
                200_000, // above 100k threshold
                1000,
            )
            .unwrap();
        assert_eq!(id, 1);
        assert_eq!(g.proposals[&id].status, ProposalStatus::Active);
    }

    #[test]
    fn test_propose_below_threshold() {
        let mut g = gov();
        let err = g
            .propose(
                ProposalKind::Text,
                "t",
                "d",
                "alice",
                500, // below 100k threshold
                1000,
            )
            .unwrap_err();
        assert_eq!(
            err,
            GovernanceError::BelowProposalThreshold {
                have: 500,
                need: PROPOSAL_THRESHOLD
            }
        );
    }

    #[test]
    fn test_vote_for_passes_quorum() {
        let mut g = gov();
        let now = 0u64;
        let id = g
            .propose(
                ProposalKind::AddChain,
                "Add Cosmos",
                "Add IBC support",
                "bob",
                500_000,
                now,
            )
            .unwrap();

        // Vote with enough weight to reach quorum
        g.vote(id, "alice", 25_000_000, VoteChoice::For, now + 100)
            .unwrap();
        g.vote(id, "bob", 20_000_000, VoteChoice::For, now + 200)
            .unwrap();

        let p = &g.proposals[&id];
        assert!(p.quorum_reached());
        assert_eq!(p.votes_for, 45_000_000);
    }

    #[test]
    fn test_vote_duplicate_rejected() {
        let mut g = gov();
        let now = 0u64;
        let id = g
            .propose(ProposalKind::Text, "t", "d", "x", 200_000, now)
            .unwrap();
        g.vote(id, "alice", 1_000_000, VoteChoice::For, now + 1)
            .unwrap();
        let err = g
            .vote(id, "alice", 1_000_000, VoteChoice::For, now + 2)
            .unwrap_err();
        assert_eq!(
            err,
            GovernanceError::AlreadyVoted {
                voter: "alice".to_string()
            }
        );
    }

    #[test]
    fn test_finalize_defeated_below_quorum() {
        let mut g = gov();
        let now = 0u64;
        let id = g
            .propose(ProposalKind::Text, "t", "d", "x", 200_000, now)
            .unwrap();
        // Vote well below quorum
        g.vote(id, "alice", 1_000, VoteChoice::For, now + 1)
            .unwrap();
        let status = g.finalize(id, now + VOTING_PERIOD_SECS + 1).unwrap();
        assert_eq!(status, ProposalStatus::Defeated);
    }

    #[test]
    fn test_finalize_succeeded_and_execute() {
        let mut g = gov();
        let now = 0u64;
        let id = g
            .propose(ProposalKind::UpdateFees, "t", "d", "x", 200_000, now)
            .unwrap();
        // Enough votes to pass quorum + majority
        g.vote(id, "a", 30_000_000, VoteChoice::For, now + 1)
            .unwrap();
        g.vote(id, "b", 15_000_000, VoteChoice::For, now + 2)
            .unwrap();

        let status = g.finalize(id, now + VOTING_PERIOD_SECS + 1).unwrap();
        assert_eq!(status, ProposalStatus::Succeeded);

        g.queue(id).unwrap();
        assert_eq!(g.proposals[&id].status, ProposalStatus::Queued);

        // Timelock not yet expired
        let err = g
            .execute(id, now + VOTING_PERIOD_SECS + TIMELOCK_DELAY_SECS - 1)
            .unwrap_err();
        assert!(matches!(err, GovernanceError::TimelockNotExpired { .. }));

        // After timelock
        g.execute(id, now + VOTING_PERIOD_SECS + TIMELOCK_DELAY_SECS + 1)
            .unwrap();
        assert_eq!(g.proposals[&id].status, ProposalStatus::Executed);
    }

    #[test]
    fn test_treasury_disburse() {
        let mut t = Treasury::new(1_000_000);
        t.disburse("auditor", 500_000, "Trail of Bits audit")
            .unwrap();
        assert_eq!(t.balance_tokens, 500_000);
        assert_eq!(t.history.len(), 1);
    }

    #[test]
    fn test_treasury_insufficient_balance() {
        let mut t = Treasury::new(100);
        let err = t.disburse("x", 200, "overspend").unwrap_err();
        assert_eq!(
            err,
            GovernanceError::InsufficientTreasuryBalance {
                requested: 200,
                available: 100
            }
        );
    }

    #[test]
    fn test_active_proposals_filter() {
        let mut g = gov();
        let now = 0u64;
        let id1 = g
            .propose(ProposalKind::Text, "t1", "d", "x", 200_000, now)
            .unwrap();
        g.propose(ProposalKind::Text, "t2", "d", "y", 200_000, now)
            .unwrap();
        // Finalize one → Defeated
        g.finalize(id1, now + VOTING_PERIOD_SECS + 1).unwrap();
        let active = g.active_proposals();
        assert_eq!(active.len(), 1);
    }

    #[test]
    fn test_voting_after_period_rejected() {
        let mut g = gov();
        let now = 0u64;
        let id = g
            .propose(ProposalKind::Text, "t", "d", "x", 200_000, now)
            .unwrap();
        // Try to vote after period
        let err = g
            .vote(
                id,
                "alice",
                1_000_000,
                VoteChoice::For,
                now + VOTING_PERIOD_SECS + 1,
            )
            .unwrap_err();
        assert_eq!(err, GovernanceError::VotingNotActive { id });
    }
}
