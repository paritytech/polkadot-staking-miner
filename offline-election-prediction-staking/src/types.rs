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

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn create_test_account_id() -> AccountId {
        AccountId::from_str("5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY").unwrap()
    }

    fn create_test_account_id_2() -> AccountId {
        AccountId::from_str("5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty").unwrap()
    }

    #[test]
    fn test_nominations_creation() {
        let targets = vec![create_test_account_id(), create_test_account_id_2()];
        let nominations = Nominations {
            targets: targets.clone(),
            submitted_in: 100,
            suppressed: false,
        };

        assert_eq!(nominations.targets, targets);
        assert_eq!(nominations.submitted_in, 100);
        assert_eq!(nominations.suppressed, false);
    }

    #[test]
    fn test_nominator_out_serialization() {
        let nominator = NominatorOut {
            stash: "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY".to_string(),
            active_stake: "1000000000000000000".to_string(),
            targets: vec!["5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty".to_string()],
            submitted_in: 100,
            suppressed: false,
        };

        // Test serialization
        let json = serde_json::to_string(&nominator).unwrap();
        assert!(json.contains("5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY"));
        assert!(json.contains("1000000000000000000"));

        // Test deserialization
        let deserialized: NominatorOut = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.stash, nominator.stash);
        assert_eq!(deserialized.active_stake, nominator.active_stake);
    }

    #[test]
    fn test_candidate_out_serialization() {
        let candidate = CandidateOut {
            stash: "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY".to_string(),
            stake: "5000000000000000000".to_string(),
        };

        // Test serialization
        let json = serde_json::to_string(&candidate).unwrap();
        assert!(json.contains("5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY"));
        assert!(json.contains("5000000000000000000"));

        // Test deserialization
        let deserialized: CandidateOut = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.stash, candidate.stash);
        assert_eq!(deserialized.stake, candidate.stake);
    }

    #[test]
    fn test_election_data_creation() {
        let account1 = create_test_account_id();
        let account2 = create_test_account_id_2();
        
        let candidates = vec![(account1.clone(), 1000000u128)];
        let nominators = vec![(account2.clone(), 500000u64, vec![account1.clone()])];

        let election_data = ElectionData {
            active_era: 1000,
            desired_validators: 19,
            desired_runners_up: 16,
            candidates: candidates.clone(),
            nominators: nominators.clone()
        };

        assert_eq!(election_data.active_era, 1000);
        assert_eq!(election_data.desired_validators, 19);
        assert_eq!(election_data.desired_runners_up, 16);
        assert_eq!(election_data.candidates, candidates);
        assert_eq!(election_data.nominators, nominators);
    }

    #[test]
    fn test_predicted_election_result_serialization() {
        let account1 = create_test_account_id();
        let account2 = create_test_account_id_2();
        
        let members = vec![(account1.clone(), 1000000u128)];
        let runners_up = vec![(account2.clone(), 500000u128)];
        let score = sp_npos_elections::ElectionScore {
            minimal_stake: 100000,
            sum_stake: 1500000,
            sum_stake_squared: 1250000000000,
        };

        let result = PredictedElectionResult {
            members: members.clone(),
            runners_up: runners_up.clone(),
            score,
            total_voters: 100,
            total_candidates: 50,
            active_era: 1000,
        };

        // Test serialization
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("1000"));
        assert!(json.contains("100"));
        assert!(json.contains("50"));

        // Test deserialization
        let deserialized: PredictedElectionResult = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.total_voters, result.total_voters);
        assert_eq!(deserialized.total_candidates, result.total_candidates);
        assert_eq!(deserialized.active_era, result.active_era);
    }

    #[test]
    fn test_detailed_prediction_result_serialization() {
        let account1 = create_test_account_id();
        let account2 = create_test_account_id_2();
        
        let prediction = PredictedElectionResult {
            members: vec![(account1.clone(), 1000000u128)],
            runners_up: vec![(account2.clone(), 500000u128)],
            score: sp_npos_elections::ElectionScore {
                minimal_stake: 100000,
                sum_stake: 1500000,
                sum_stake_squared: 1250000000000,
            },
            total_voters: 100,
            total_candidates: 50,
            active_era: 1000,
        };

        let stake_distribution = vec![(account1.clone(), 1000000u64, 50.0)];
        let mut validator_support = HashMap::new();
        validator_support.insert(account1.to_string(), 1000000u128);

        let detailed_result = DetailedPredictionResult {
            prediction: prediction.clone(),
            total_stake: 2000000u128,
            total_voters: 100,
            total_candidates: 50,
            stake_distribution: stake_distribution.clone(),
            validator_support: validator_support.clone(),
        };

        // Test serialization
        let json = serde_json::to_string(&detailed_result).unwrap();
        assert!(json.contains("2000000"));
        assert!(json.contains("95.5"));

        // Test deserialization
        let deserialized: DetailedPredictionResult = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.total_stake, detailed_result.total_stake);
    }

    #[test]
    fn test_prediction_output_serialization() {
        let account1 = create_test_account_id();
        
        let validator_info = ValidatorInfo {
            account: account1.to_string(),
            total_stake: 1000000u128,
            self_stake: 100000u128
        };

        let statistics = PredictionStatistics {
            minimum_stake: 100000u128,
            average_stake: 500000u128,
            total_staked: 10000000u128,
        };

        let results = PredictionResults {
            active_validators: vec![validator_info.clone()],
            statistics: statistics.clone(),
        };

        let metadata = PredictionMetadata {
            timestamp: "2025-01-01T00:00:00Z".to_string(),
            desired_validators: 19,
        };

        let output = PredictionOutput {
            metadata: metadata.clone(),
            results: results.clone(),
        };

        // Test serialization
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("2025-01-01T00:00:00Z"));
        assert!(json.contains("19"));
        assert!(json.contains("1000000"));

        // Test deserialization
        let deserialized: PredictionOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.metadata.desired_validators, output.metadata.desired_validators);
        assert_eq!(deserialized.results.active_validators.len(), output.results.active_validators.len());
    }

    #[test]
    fn test_snapshot_data_creation() {
        let account1 = create_test_account_id();
        let account2 = create_test_account_id_2();
        
        let target_snapshot = vec![account1.clone(), account2.clone()];
        let voter_snapshots = vec![(account1.clone(), 1000000u128, vec![account2.clone()])];

        let snapshot_data = SnapshotData {
            current_round: 100,
            desired_targets: 19,
            target_snapshot: target_snapshot.clone(),
            voter_snapshots: voter_snapshots.clone(),
        };

        assert_eq!(snapshot_data.current_round, 100);
        assert_eq!(snapshot_data.desired_targets, 19);
        assert_eq!(snapshot_data.target_snapshot, target_snapshot);
        assert_eq!(snapshot_data.voter_snapshots, voter_snapshots);
    }

    #[test]
    fn test_snapshot_result_available() {
        let account1 = create_test_account_id();
        let snapshot_data = SnapshotData {
            current_round: 100,
            desired_targets: 19,
            target_snapshot: vec![account1.clone()],
            voter_snapshots: vec![(account1.clone(), 1000000u128, vec![account1.clone()])],
        };

        let result = SnapshotResult::Available(snapshot_data.clone());
        
        match result {
            SnapshotResult::Available(data) => {
                assert_eq!(data.current_round, 100);
                assert_eq!(data.desired_targets, 19);
            }
            SnapshotResult::NotAvailable => panic!("Expected Available variant"),
        }
    }

    #[test]
    fn test_snapshot_result_not_available() {
        let result = SnapshotResult::NotAvailable;
        
        match result {
            SnapshotResult::Available(_) => panic!("Expected NotAvailable variant"),
            SnapshotResult::NotAvailable => {
                // This is the expected case
            }
        }
    }

    #[test]
    fn test_prediction_error_display() {
        let errors = vec![
            PredictionError::NoCandidates,
            PredictionError::NoVoters,
            PredictionError::AlgorithmError,
            PredictionError::InvalidInput,
            PredictionError::InsufficientStake,
        ];

        for error in errors {
            let error_msg = format!("{}", error);
            assert!(!error_msg.is_empty());
            assert!(error_msg.len() > 10); // Ensure meaningful error messages
        }
    }

    #[test]
    fn test_type_aliases() {
        let account1 = create_test_account_id();
        let account2 = create_test_account_id_2();
        
        // Test ValidatorStake
        let validator_stake: ValidatorStake = (account1.clone(), 1000000u128);
        assert_eq!(validator_stake.0, account1);
        assert_eq!(validator_stake.1, 1000000u128);

        // Test NominatorData
        let nominator_data: NominatorData = (account1.clone(), 500000u64, vec![account2.clone()]);
        assert_eq!(nominator_data.0, account1);
        assert_eq!(nominator_data.1, 500000u64);
        assert_eq!(nominator_data.2, vec![account2]);

        // Test StakeDistribution
        let stake_dist: StakeDistribution = (account1.clone(), 1000000u64, 50.0);
        assert_eq!(stake_dist.0, account1);
        assert_eq!(stake_dist.1, 1000000u64);
        assert_eq!(stake_dist.2, 50.0);

        // Test ValidatorSupport
        let mut validator_support: ValidatorSupport = HashMap::new();
        validator_support.insert(account1.to_string(), 1000000u128);
        assert_eq!(validator_support.get(&account1.to_string()), Some(&1000000u128));
    }
}
