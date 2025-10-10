//! Type definitions for the offline election prediction tool
//!
//! This module contains all the data structures used throughout the application,
//! organized by their purpose and functionality.

use serde::{Deserialize, Serialize};
use subxt::utils::AccountId32;
use std::collections::HashMap;

// ============================================================================
// Core Blockchain Types
// ============================================================================

/// Account identifier type alias for consistency
pub type AccountId = AccountId32;

/// Balance type alias for consistency
pub type Balance = u128;

/// Era index type alias
pub type EraIndex = u32;

/// Block number type alias
pub type BlockNumber = u32;

// ============================================================================
// Staking and Nominations Types
// ============================================================================

/// Nominations data structure from the blockchain
#[derive(Debug, Clone, parity_scale_codec::Decode)]
pub struct Nominations {
    /// List of validator accounts being nominated
    pub targets: Vec<AccountId>,
    /// Era when nominations were submitted
    pub submitted_in: EraIndex,
    /// Whether the nominator is suppressed
    pub suppressed: bool,
}

/// Nominator output structure for JSON serialization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NominatorOut {
    /// Nominator's stash account (SS58 format)
    pub stash: String,
    /// Active stake amount (as string to avoid precision loss)
    pub active_stake: String,
    /// List of nominated validator accounts (SS58 format)
    pub targets: Vec<String>,
    /// Era when nominations were submitted
    pub submitted_in: EraIndex,
    /// Whether the nominator is suppressed
    pub suppressed: bool,
}

/// Candidate output structure for JSON serialization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateOut {
    /// Candidate's stash account (SS58 format)
    pub stash: String,
    /// Candidate's stake amount (as string to avoid precision loss)
    pub stake: String,
}

// ============================================================================
// Election Data Types
// ============================================================================

/// Container for all election-related data
#[derive(Debug, Clone)]
pub struct ElectionData {
    /// Current active era
    pub active_era: EraIndex,
    /// Desired number of validators
    pub desired_validators: u32,
    /// Desired number of runners-up
    pub desired_runners_up: u32,
    /// List of candidates with their stakes
    pub candidates: Vec<(AccountId, Balance)>,
    /// List of nominators with their stakes and targets
    pub nominators: Vec<(AccountId, u64, Vec<AccountId>)>
}

// ============================================================================
// Prediction Result Types
// ============================================================================

/// Predicted election result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PredictedElectionResult {
    /// Elected validators with their stakes
    pub members: Vec<(AccountId, Balance)>,
    /// Runners-up with their stakes
    pub runners_up: Vec<(AccountId, Balance)>,
    /// Election score
    pub score: sp_npos_elections::ElectionScore,
    /// Total number of voters
    pub total_voters: u32,
    /// Total number of candidates
    pub total_candidates: u32,
    /// Active era
    pub active_era: EraIndex,
}

/// Detailed prediction result with additional analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetailedPredictionResult {
    /// Basic prediction result
    pub prediction: PredictedElectionResult,
    /// Total stake in the system
    pub total_stake: Balance,
    /// Total number of voters
    pub total_voters: usize,
    /// Total number of candidates
    pub total_candidates: usize,
    /// Stake distribution by voter (voter, stake, percentage)
    pub stake_distribution: Vec<(AccountId, u64, f64)>,
    /// Validator support mapping (validator -> total support)
    pub validator_support: HashMap<String, Balance>
}

// ============================================================================
// Output Format Types
// ============================================================================

/// New output format for prediction results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PredictionOutput {
    /// Metadata about the prediction
    pub metadata: PredictionMetadata,
    /// Prediction results
    pub results: PredictionResults,
}

/// Metadata for prediction output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PredictionMetadata {
    /// Timestamp when prediction was made
    pub timestamp: String,
    /// Desired number of validators
    pub desired_validators: u32,
}

/// Prediction results container
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PredictionResults {
    /// List of active validators with detailed info
    pub active_validators: Vec<ValidatorInfo>,
    /// Overall statistics
    pub statistics: PredictionStatistics,
}

/// Individual validator information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorInfo {
    /// Validator's account (SS58 format)
    pub account: String,
    /// Total stake backing this validator
    pub total_stake: Balance,
    /// Validator's own stake
    pub self_stake: Balance,
}

/// Overall prediction statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PredictionStatistics {
    /// Minimum stake among elected validators
    pub minimum_stake: Balance,
    /// Average stake among elected validators
    pub average_stake: Balance,
    /// Total stake across all elected validators
    pub total_staked: Balance,
}

// ============================================================================
// Error Types
// ============================================================================

/// Prediction errors
#[derive(Debug, Clone)]
pub enum PredictionError {
    /// No candidates available for election
    NoCandidates,
    /// No voters available for election
    NoVoters,
    /// Election algorithm failed
    AlgorithmError,
    /// Invalid input parameters
    InvalidInput,
    /// Insufficient stake for election
    InsufficientStake,
}

impl std::fmt::Display for PredictionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PredictionError::NoCandidates => write!(f, "No candidates available for election"),
            PredictionError::NoVoters => write!(f, "No voters available for election"),
            PredictionError::AlgorithmError => write!(f, "Election algorithm failed"),
            PredictionError::InvalidInput => write!(f, "Invalid input parameters"),
            PredictionError::InsufficientStake => write!(f, "Insufficient stake for election"),
        }
    }
}

impl std::error::Error for PredictionError {}

// ============================================================================
// Snapshot Types
// ============================================================================

/// Snapshot data for election prediction
#[derive(Debug, Clone)]
pub struct SnapshotData {
    /// Current round number
    pub current_round: u32,
    /// Desired targets (active set size)
    pub desired_targets: u32,
    /// Target snapshot (validators)
    pub target_snapshot: Vec<AccountId>,
    /// Voter snapshots (nominators)
    pub voter_snapshots: Vec<(AccountId, Balance, Vec<AccountId>)>,
}

/// Snapshot fetching result
#[derive(Debug, Clone)]
pub enum SnapshotResult {
    /// Snapshot data successfully fetched
    Available(SnapshotData),
    /// Snapshot not available, fallback to Staking pallet
    NotAvailable,
}

// ============================================================================
// Utility Types
// ============================================================================

/// Type alias for validator stake pairs
pub type ValidatorStake = (AccountId, Balance);

/// Type alias for nominator data (stash, stake, targets)
pub type NominatorData = (AccountId, u64, Vec<AccountId>);

/// Type alias for stake distribution entries
pub type StakeDistribution = (AccountId, u64, f64);

/// Type alias for validator support mapping
pub type ValidatorSupport = HashMap<String, Balance>;
