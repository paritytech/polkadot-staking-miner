//! Integration tests for the offline election prediction tool

use offline_election::{
    types::*,
    predict::ElectionPredictor,
    cli::{PredictCliConfig, save_prediction_results},
};
use std::str::FromStr;
use tempfile::TempDir;

fn create_test_account_id() -> AccountId {
    AccountId::from_str("5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY").unwrap()
}

fn create_test_account_id_2() -> AccountId {
    AccountId::from_str("5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty").unwrap()
}

fn create_test_account_id_3() -> AccountId {
    AccountId::from_str("5DAAnrj7VHTznn2AWBemMuyBwZWs6FNFjdyVXUeYum3PTXFy").unwrap()
}

#[test]
fn test_full_election_prediction_workflow() {
    let account1 = create_test_account_id();
    let account2 = create_test_account_id_2();
    let account3 = create_test_account_id_3();
    
    // Create test data
    let voters = vec![
        (account1.clone(), 1000000u64, vec![account2.clone()]),
        (account2.clone(), 2000000u64, vec![account3.clone()]),
        (account3.clone(), 3000000u64, vec![account1.clone()]),
    ];
    let candidate_stashes = vec![account1.clone(), account2.clone(), account3.clone()];

    // Create predictor
    let predictor = ElectionPredictor::new(1000, 2, 1, voters, candidate_stashes);
    
    // Run prediction with analysis
    let result = predictor.predict_with_analysis();
    assert!(result.is_ok());
    
    let detailed_result = result.unwrap();
    
    // Verify basic properties
    assert_eq!(detailed_result.total_voters, 3);
    assert_eq!(detailed_result.total_candidates, 3);
    assert_eq!(detailed_result.total_stake, 6000000u128); // 1000000 + 2000000 + 3000000
    assert!(!detailed_result.stake_distribution.is_empty());
    assert!(!detailed_result.validator_support.is_empty());
    
    // Verify stake distribution percentages add up to 100%
    let total_percentage: f64 = detailed_result.stake_distribution.iter()
        .map(|(_, _, percentage)| percentage)
        .sum();
    assert!((total_percentage - 100.0).abs() < 0.01);
}

#[test]
fn test_cli_config_parsing() {
    // Test that CLI config can be created with various options
    let config = PredictCliConfig {
        chain_uri: "wss://test.example.com".to_string(),
        desired_validators: Some(100),
        custom_nominators_validators: None,
        output: "test_output.json".to_string(),
        use_cached_data: false,
        cache_dir: "./test_cache".to_string(),
    };

    // Test desired validators override
    assert_eq!(config.get_desired_validators(19), 100);
    
    // Test without override
    let config_no_override = PredictCliConfig {
        desired_validators: None,
        ..config
    };
    assert_eq!(config_no_override.get_desired_validators(19), 19);
}

#[test]
fn test_election_data_creation_and_usage() {
    let account1 = create_test_account_id();
    let account2 = create_test_account_id_2();
    
    let election_data = ElectionData {
        active_era: 1000,
        desired_validators: 19,
        desired_runners_up: 16,
        candidates: vec![(account1.clone(), 1000000u128)],
        nominators: vec![(account2.clone(), 500000u64, vec![account1.clone()])],
    };

    // Test that election data can be used in prediction
    let voters: Vec<NominatorData> = election_data.nominators.clone();
    let candidate_stashes: Vec<AccountId> = election_data.candidates.iter()
        .map(|(account, _)| account.clone())
        .collect();

    let predictor = ElectionPredictor::new(
        election_data.active_era,
        election_data.desired_validators,
        election_data.desired_runners_up,
        voters,
        candidate_stashes,
    );

    let result = predictor.predict_election();
    assert!(result.is_ok());
    
    let prediction = result.unwrap();
    assert_eq!(prediction.active_era, election_data.active_era);
    assert_eq!(prediction.total_voters, 1);
    assert_eq!(prediction.total_candidates, 1);
}

#[test]
fn test_snapshot_data_workflow() {
    let account1 = create_test_account_id();
    let account2 = create_test_account_id_2();
    
    // Create snapshot data
    let snapshot_data = SnapshotData {
        current_round: 100,
        desired_targets: 19,
        target_snapshot: vec![account1.clone(), account2.clone()],
        voter_snapshots: vec![
            (account1.clone(), 1000000u128, vec![account2.clone()]),
            (account2.clone(), 2000000u128, vec![account1.clone()]),
        ],
    };

    // Test snapshot result variants
    let available_result = SnapshotResult::Available(snapshot_data.clone());
    match available_result {
        SnapshotResult::Available(data) => {
            assert_eq!(data.current_round, 100);
            assert_eq!(data.desired_targets, 19);
            assert_eq!(data.target_snapshot.len(), 2);
            assert_eq!(data.voter_snapshots.len(), 2);
        }
        SnapshotResult::NotAvailable => panic!("Expected Available variant"),
    }

    let not_available_result = SnapshotResult::NotAvailable;
    match not_available_result {
        SnapshotResult::Available(_) => panic!("Expected NotAvailable variant"),
        SnapshotResult::NotAvailable => {
            // This is the expected case
        }
    }
}

#[test]
fn test_prediction_output_format() {
    let account1 = create_test_account_id();
    let account2 = create_test_account_id_2();
    
    // Create detailed prediction result
    let prediction = PredictedElectionResult {
        members: vec![(account1.clone(), 1000000u128)],
        runners_up: vec![(account2.clone(), 500000u128)],
        score: sp_npos_elections::ElectionScore {
            minimal_stake: 100000,
            sum_stake: 1500000,
            sum_stake_squared: 1250000000000,
        },
        total_voters: 2,
        total_candidates: 2,
        active_era: 1000,
    };

    let detailed_result = DetailedPredictionResult {
        prediction,
        total_stake: 2000000u128,
        total_voters: 2,
        total_candidates: 2,
        stake_distribution: vec![
            (account1.clone(), 1000000u64, 50.0),
            (account2.clone(), 1000000u64, 50.0),
        ],
        validator_support: {
            let mut map = std::collections::HashMap::new();
            map.insert(account1.to_string(), 1000000u128);
            map.insert(account2.to_string(), 1000000u128);
            map
        },
    };

    // Create election data for conversion
    let election_data = ElectionData {
        active_era: 1000,
        desired_validators: 19,
        desired_runners_up: 16,
        candidates: vec![(account1.clone(), 1000000u128)],
        nominators: vec![(account2.clone(), 1000000u64, vec![account1.clone()])],
    };

    // Test conversion to new format
    let result = offline_election::cli::convert_to_new_format(&detailed_result, 19, &election_data);
    assert!(result.is_ok());
    
    let output = result.unwrap();
    assert_eq!(output.metadata.desired_validators, 19);
    assert_eq!(output.results.active_validators.len(), 1);
    assert_eq!(output.results.statistics.total_staked, 1000000u128);
    
    // Test serialization
    let json = serde_json::to_string(&output).unwrap();
    assert!(json.contains("19"));
    assert!(json.contains("1000000"));
}

#[test]
fn test_file_operations() {
    let temp_dir = TempDir::new().unwrap();
    let output_path = temp_dir.path().join("test_prediction.json");
    
    let account1 = create_test_account_id();
    let election_data = ElectionData {
        active_era: 1000,
        desired_validators: 19,
        desired_runners_up: 16,
        candidates: vec![(account1.clone(), 1000000u128)],
        nominators: vec![(account1.clone(), 500000u64, vec![account1.clone()])],
    };
    
    let prediction = PredictedElectionResult {
        members: vec![(account1.clone(), 1000000u128)],
        runners_up: vec![],
        score: sp_npos_elections::ElectionScore {
            minimal_stake: 100000,
            sum_stake: 1000000,
            sum_stake_squared: 1000000000000,
        },
        total_voters: 1,
        total_candidates: 1,
        active_era: 1000,
    };

    let detailed_result = DetailedPredictionResult {
        prediction,
        total_stake: 1000000u128,
        total_voters: 1,
        total_candidates: 1,
        stake_distribution: vec![(account1.clone(), 1000000u64, 100.0)],
        validator_support: {
            let mut map = std::collections::HashMap::new();
            map.insert(account1.to_string(), 1000000u128);
            map
        },
    };

    // Test saving prediction results
    let result = save_prediction_results(&detailed_result, 19, &election_data, output_path.to_str().unwrap());
    assert!(result.is_ok());

    // Verify file was created and contains expected content
    assert!(output_path.exists());
    let content = std::fs::read_to_string(&output_path).unwrap();
    assert!(content.contains("1000000"));
    assert!(content.contains("19"));
    assert!(content.contains("metadata"));
    assert!(content.contains("results"));
}

#[test]
fn test_error_handling() {
    // Test prediction errors
    let account1 = create_test_account_id();
    
    // Test with no candidates
    let voters = vec![(account1.clone(), 1000000u64, vec![account1.clone()])];
    let candidate_stashes = vec![];
    let predictor = ElectionPredictor::new(1000, 19, 16, voters, candidate_stashes);
    let result = predictor.predict_election();
    assert!(result.is_err());
    match result.unwrap_err() {
        PredictionError::NoCandidates => {
            // Expected error
        }
        _ => panic!("Expected NoCandidates error"),
    }

    // Test with no voters
    let voters = vec![];
    let candidate_stashes = vec![account1.clone()];
    let predictor = ElectionPredictor::new(1000, 19, 16, voters, candidate_stashes);
    let result = predictor.predict_election();
    assert!(result.is_err());
    match result.unwrap_err() {
        PredictionError::NoVoters => {
            // Expected error
        }
        _ => panic!("Expected NoVoters error"),
    }
}

#[test]
fn test_type_aliases_work_correctly() {
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
    let mut validator_support: ValidatorSupport = std::collections::HashMap::new();
    validator_support.insert(account1.to_string(), 1000000u128);
    assert_eq!(validator_support.get(&account1.to_string()), Some(&1000000u128));
}
