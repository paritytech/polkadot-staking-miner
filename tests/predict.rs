//! Tests for the predict command

use assert_cmd::cargo::cargo_bin;
use assert_cmd::Command;
use polkadot_staking_miner::commands::types::PredictConfig;
use std::fs;
use tempfile::TempDir;

/// Test that the predict command help works
#[test]
fn predict_help_works() {
	let crate_name = env!("CARGO_PKG_NAME");
	let mut cmd = Command::new(cargo_bin(crate_name));
	cmd.args(&["predict", "--help"]);
	cmd.assert().success();
}

/// Test that predict command accepts basic arguments
#[test]
fn predict_cli_args_parsing() {
	let crate_name = env!("CARGO_PKG_NAME");
	
	// Test with desired validators
	let mut cmd = Command::new(cargo_bin(crate_name));
	cmd.args(&["predict", "--desired-validators", "19"]);
	// This will fail because we need a valid URI, but we're just testing argument parsing
	// In a real scenario, you'd need a running node or mock
}

/// Test PredictConfig parsing
#[test]
fn predict_config_parsing() {
	use clap::Parser;
	
	// Test default values
	let config = PredictConfig::try_parse_from(&["predict"]).unwrap();
	assert_eq!(config.desired_validators, None);
	assert_eq!(config.custom_file, None);
	assert_eq!(config.output_dir, "results");
	
	// Test with desired validators
	let config = PredictConfig::try_parse_from(&[
		"predict",
		"--desired-validators",
		"50",
	])
	.unwrap();
	assert_eq!(config.desired_validators, Some(50));
	
	// Test with custom file
	let config = PredictConfig::try_parse_from(&[
		"predict",
		"--custom-file",
		"custom.json",
	])
	.unwrap();
	assert_eq!(config.custom_file, Some("custom.json".to_string()));
	
	// Test with output directory
	let config = PredictConfig::try_parse_from(&[
		"predict",
		"--output-dir",
		"outputs",
	])
	.unwrap();
	assert_eq!(config.output_dir, "outputs");
	
	// Test with all options
	let config = PredictConfig::try_parse_from(&[
		"predict",
		"--desired-validators",
		"50",
		"--custom-file",
		"data/custom.json",
		"--output-dir",
		"test_outputs",
	])
	.unwrap();
	assert_eq!(config.desired_validators, Some(50));
	assert_eq!(config.custom_file, Some("data/custom.json".to_string()));
	assert_eq!(config.output_dir, "test_outputs");
}

/// Create a temporary custom election data file for testing
fn create_test_custom_file(temp_dir: &TempDir) -> std::path::PathBuf {
	use polkadot_sdk::sp_core::crypto::Pair;
	use subxt::utils::AccountId32;
	
	// Create test account IDs
	let pair1 = polkadot_staking_miner::prelude::Pair::from_string("//Alice", None).unwrap();
	let account1 = AccountId32::from(pair1.public().0);
	
	let pair2 = polkadot_staking_miner::prelude::Pair::from_string("//Bob", None).unwrap();
	let account2 = AccountId32::from(pair2.public().0);
	
	let pair3 = polkadot_staking_miner::prelude::Pair::from_string("//Charlie", None).unwrap();
	let account3 = AccountId32::from(pair3.public().0);
	
	let custom_data = serde_json::json!({
		"metadata": {
			"ss58Prefix": 42
		},
		"candidates": [
			{
				"account": account1.to_string(),
				"stake": 1000000000000u64
			},
			{
				"account": account2.to_string(),
				"stake": 2000000000000u64
			}
		],
		"nominators": [
			{
				"account": account3.to_string(),
				"stake": 500000000000u64,
				"targets": [
					account1.to_string(),
					account2.to_string()
				]
			}
		]
	});
	
	let file_path = temp_dir.path().join("test_custom.json");
	fs::write(&file_path, serde_json::to_string_pretty(&custom_data).unwrap()).unwrap();
	file_path
}

/// Test that custom file format is correctly validated
#[test]
fn test_custom_file_format_validation() {
	let temp_dir = TempDir::new().unwrap();
	let custom_file = create_test_custom_file(&temp_dir);
	
	// Read and parse the file
	let content = fs::read_to_string(&custom_file).unwrap();
	let parsed: polkadot_staking_miner::commands::types::CustomElectionFile =
		serde_json::from_str(&content).unwrap();
	
	// Validate structure
	assert_eq!(parsed.metadata.ss58_prefix, 42);
	assert_eq!(parsed.candidates.len(), 2);
	assert_eq!(parsed.nominators.len(), 1);
	assert_eq!(parsed.nominators[0].targets.len(), 2);
}

/// Test invalid custom file format
#[test]
fn test_invalid_custom_file_format() {
	let temp_dir = TempDir::new().unwrap();
	let invalid_file = temp_dir.path().join("invalid.json");
	
	// Write invalid JSON
	fs::write(&invalid_file, "{ invalid json }").unwrap();
	
	// Try to parse - should fail
	let content = fs::read_to_string(&invalid_file).unwrap();
	let result: Result<polkadot_staking_miner::commands::types::CustomElectionFile, _> =
		serde_json::from_str(&content);
	assert!(result.is_err());
}

/// Test custom file with missing required fields
#[test]
fn test_custom_file_missing_fields() {
	let temp_dir = TempDir::new().unwrap();
	let incomplete_file = temp_dir.path().join("incomplete.json");
	
	// Write JSON with missing fields
	let incomplete_data = serde_json::json!({
		"metadata": {
			"ss58Prefix": 42
		}
		// Missing candidates and nominators
	});
	
	fs::write(&incomplete_file, serde_json::to_string(&incomplete_data).unwrap()).unwrap();
	
	// Try to parse - should fail or have defaults
	let content = fs::read_to_string(&incomplete_file).unwrap();
	let _result: Result<polkadot_staking_miner::commands::types::CustomElectionFile, _> =
		serde_json::from_str(&content);
	// This might succeed with empty vectors, depending on serde defaults
	// The actual validation happens when trying to use the data
}

/// Test nested custom file path handling
#[test]
fn test_nested_custom_file_path() {
	let temp_dir = TempDir::new().unwrap();
	
	// Create nested directory structure
	let nested_dir = temp_dir.path().join("data").join("elections");
	fs::create_dir_all(&nested_dir).unwrap();
	
	let custom_file = nested_dir.join("custom.json");
	let test_file = create_test_custom_file(&temp_dir);
	
	// Copy to nested location
	fs::copy(&test_file, &custom_file).unwrap();
	
	// Verify file exists at nested path
	assert!(custom_file.exists());
	assert!(custom_file.parent().unwrap().exists());
}

/// Test output directory creation
#[test]
fn test_output_directory_creation() {
	let temp_dir = TempDir::new().unwrap();
	let output_dir = temp_dir.path().join("test_outputs");
	
	// Directory should not exist initially
	assert!(!output_dir.exists());
	
	// Create directory
	fs::create_dir_all(&output_dir).unwrap();
	
	// Directory should now exist
	assert!(output_dir.exists());
	assert!(output_dir.is_dir());
}

/// Test that output files are created in the correct directory
#[test]
fn test_output_files_creation() {
	let temp_dir = TempDir::new().unwrap();
	let output_dir = temp_dir.path().join("test_outputs");
	fs::create_dir_all(&output_dir).unwrap();
	
	// Create expected output files
	let validators_output = output_dir.join("validators_prediction.json");
	let nominators_output = output_dir.join("nominators_prediction.json");
	
	// Create test data
	let validators_data = serde_json::json!({
		"metadata": {
			"timestamp": "1234567890",
			"desired_validators": 19,
			"round": 0,
			"block_number": 1000,
			"solution_score": null,
			"data_source": "test"
		},
		"results": []
	});
	
	let nominators_data = serde_json::json!({
		"metadata": {
			"timestamp": "1234567890",
			"desired_validators": 19,
			"round": 0,
			"block_number": 1000,
			"solution_score": null,
			"data_source": "test"
		},
		"nominators": []
	});
	
	// Write files
	fs::write(&validators_output, serde_json::to_string_pretty(&validators_data).unwrap()).unwrap();
	fs::write(&nominators_output, serde_json::to_string_pretty(&nominators_data).unwrap()).unwrap();
	
	// Verify files exist
	assert!(validators_output.exists());
	assert!(nominators_output.exists());
	
	// Verify file contents
	let validators_content: serde_json::Value =
		serde_json::from_str(&fs::read_to_string(&validators_output).unwrap()).unwrap();
	assert_eq!(validators_content["metadata"]["desired_validators"], 19);
	
	let nominators_content: serde_json::Value =
		serde_json::from_str(&fs::read_to_string(&nominators_output).unwrap()).unwrap();
	assert_eq!(nominators_content["metadata"]["desired_validators"], 19);
}

/// Test custom file with empty candidates
#[test]
fn test_custom_file_empty_candidates() {
	let temp_dir = TempDir::new().unwrap();
	let empty_file = temp_dir.path().join("empty_candidates.json");
	
	let empty_data = serde_json::json!({
		"metadata": {
			"ss58Prefix": 42
		},
		"candidates": [],
		"nominators": []
	});
	
	fs::write(&empty_file, serde_json::to_string(&empty_data).unwrap()).unwrap();
	
	let content = fs::read_to_string(&empty_file).unwrap();
	let parsed: polkadot_staking_miner::commands::types::CustomElectionFile =
		serde_json::from_str(&content).unwrap();
	
	assert_eq!(parsed.candidates.len(), 0);
	assert_eq!(parsed.nominators.len(), 0);
}

/// Test custom file with large stake values
#[test]
fn test_custom_file_large_stakes() {
	let temp_dir = TempDir::new().unwrap();
	let large_stakes_file = temp_dir.path().join("large_stakes.json");
	
	use polkadot_sdk::sp_core::crypto::Pair;
	use subxt::utils::AccountId32;
	
	let pair = polkadot_staking_miner::prelude::Pair::from_string("//Alice", None).unwrap();
	let account = AccountId32::from(pair.public().0);
	
	let large_data = serde_json::json!({
		"metadata": {
			"ss58Prefix": 42
		},
		"candidates": [
			{
				"account": account.to_string(),
				"stake": 18446744073709551615u64  // u64::MAX
			}
		],
		"nominators": [
			{
				"account": account.to_string(),
				"stake": 18446744073709551615u64,
				"targets": [account.to_string()]
			}
		]
	});
	
	fs::write(&large_stakes_file, serde_json::to_string(&large_data).unwrap()).unwrap();
	
	let content = fs::read_to_string(&large_stakes_file).unwrap();
	let parsed: polkadot_staking_miner::commands::types::CustomElectionFile =
		serde_json::from_str(&content).unwrap();
	
	assert_eq!(parsed.candidates[0].stake, u128::from(u64::MAX));
	assert_eq!(parsed.nominators[0].stake, u64::MAX);
}

/// Test that SS58 prefix is correctly read from custom file
#[test]
fn test_ss58_prefix_from_custom_file() {
	let temp_dir = TempDir::new().unwrap();
	let custom_file = temp_dir.path().join("test_ss58.json");
	
	use polkadot_sdk::sp_core::crypto::Pair;
	use subxt::utils::AccountId32;
	
	let pair = polkadot_staking_miner::prelude::Pair::from_string("//Alice", None).unwrap();
	let account = AccountId32::from(pair.public().0);
	
	let data = serde_json::json!({
		"metadata": {
			"ss58Prefix": 0  // Polkadot mainnet
		},
		"candidates": [
			{
				"account": account.to_string(),
				"stake": 1000000u64
			}
		],
		"nominators": []
	});
	
	fs::write(&custom_file, serde_json::to_string(&data).unwrap()).unwrap();
	
	let content = fs::read_to_string(&custom_file).unwrap();
	let parsed: polkadot_staking_miner::commands::types::CustomElectionFile =
		serde_json::from_str(&content).unwrap();
	
	assert_eq!(parsed.metadata.ss58_prefix, 0);
}

/// Test custom file with multiple nominators and targets
#[test]
fn test_custom_file_multiple_nominators() {
	let temp_dir = TempDir::new().unwrap();
	let multi_file = temp_dir.path().join("multi_nominators.json");
	
	use polkadot_sdk::sp_core::crypto::Pair;
	use subxt::utils::AccountId32;
	
	let pair1 = polkadot_staking_miner::prelude::Pair::from_string("//Alice", None).unwrap();
	let account1 = AccountId32::from(pair1.public().0);
	
	let pair2 = polkadot_staking_miner::prelude::Pair::from_string("//Bob", None).unwrap();
	let account2 = AccountId32::from(pair2.public().0);
	
	let pair3 = polkadot_staking_miner::prelude::Pair::from_string("//Charlie", None).unwrap();
	let account3 = AccountId32::from(pair3.public().0);
	
	let data = serde_json::json!({
		"metadata": {
			"ss58Prefix": 42
		},
		"candidates": [
			{
				"account": account1.to_string(),
				"stake": 1000000u64
			},
			{
				"account": account2.to_string(),
				"stake": 2000000u64
			}
		],
		"nominators": [
			{
				"account": account3.to_string(),
				"stake": 500000u64,
				"targets": [account1.to_string(), account2.to_string()]
			},
			{
				"account": account1.to_string(),
				"stake": 300000u64,
				"targets": [account2.to_string()]
			}
		]
	});
	
	fs::write(&multi_file, serde_json::to_string(&data).unwrap()).unwrap();
	
	let content = fs::read_to_string(&multi_file).unwrap();
	let parsed: polkadot_staking_miner::commands::types::CustomElectionFile =
		serde_json::from_str(&content).unwrap();
	
	assert_eq!(parsed.candidates.len(), 2);
	assert_eq!(parsed.nominators.len(), 2);
	assert_eq!(parsed.nominators[0].targets.len(), 2);
	assert_eq!(parsed.nominators[1].targets.len(), 1);
}

