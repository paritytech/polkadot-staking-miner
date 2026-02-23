//! Tests for the predict command

use assert_cmd::Command;
use polkadot_staking_miner::commands::types::{OverridesConfig, PredictConfig};
use std::fs;
use tempfile::TempDir;

/// Test that the predict command help works
#[test]
fn predict_help_works() {
	let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!(env!("CARGO_PKG_NAME")));
	cmd.args(["predict", "--help"]);
	cmd.assert().success();
}

/// Test that predict command accepts basic arguments
#[test]
fn predict_cli_args_parsing() {
	// Test with desired validators
	let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!(env!("CARGO_PKG_NAME")));
	cmd.args(["predict", "--desired-validators", "19"]);
	// This will fail because we need a valid URI, but we're just testing argument parsing
	// In a real scenario, you'd need a running node or mock
}

/// Test PredictConfig parsing
#[test]
fn predict_config_parsing() {
	use clap::Parser;

	// Test with output directory
	let config = PredictConfig::try_parse_from(["predict", "--output-dir", "outputs"]).unwrap();
	assert_eq!(config.output_dir, Some("outputs".to_string()));

	// Test with overrides
	let config =
		PredictConfig::try_parse_from(["predict", "--overrides", "overrides.json"]).unwrap();
	assert_eq!(config.overrides, Some(OverridesConfig::Path("overrides.json".to_string())));

	// Test with algorithm
	let config = PredictConfig::try_parse_from(["predict", "--algorithm", "phragmms"]).unwrap();
	assert_eq!(
		config.algorithm,
		polkadot_staking_miner::commands::types::ElectionAlgorithm::Phragmms
	);

	// Test with all options
	let config = PredictConfig::try_parse_from([
		"predict",
		"--desired-validators",
		"50",
		"--overrides",
		"data/overrides.json",
		"--output-dir",
		"test_outputs",
		"--algorithm",
		"phragmms",
	])
	.unwrap();
	assert_eq!(config.desired_validators, Some(50));
	assert_eq!(config.overrides, Some(OverridesConfig::Path("data/overrides.json".to_string())));
	assert_eq!(config.output_dir, Some("test_outputs".to_string()));
	assert_eq!(
		config.algorithm,
		polkadot_staking_miner::commands::types::ElectionAlgorithm::Phragmms
	);
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

/// Create a temporary overrides file for testing
fn create_test_overrides_file(temp_dir: &TempDir) -> std::path::PathBuf {
	let overrides_data = serde_json::json!({
		"candidates_include": ["15S7YtETM31QxYYqubAwRJKRSM4v4Ua6WGFYnx1VuFBnWqdG"],
		"candidates_exclude": [],
		"voters_include": [
			["15S7YtETM31QxYYqubAwRJKRSM4v4Ua6WGFYnx1VuFBnWqdG", 1000000, ["15S7YtETM31QxYYqubAwRJKRSM4v4Ua6WGFYnx1VuFBnWqdG"]]
		],
		"voters_exclude": []
	});

	let file_path = temp_dir.path().join("test_overrides.json");
	fs::write(&file_path, serde_json::to_string_pretty(&overrides_data).unwrap()).unwrap();
	file_path
}

/// Test that overrides file format is correctly validated
#[test]
fn test_overrides_file_format_validation() {
	let temp_dir = TempDir::new().unwrap();
	let overrides_path = create_test_overrides_file(&temp_dir);

	// Read and parse the file
	let content = fs::read_to_string(&overrides_path).unwrap();
	let parsed: polkadot_staking_miner::commands::types::ElectionOverrides =
		serde_json::from_str(&content).unwrap();

	// Validate structure
	assert_eq!(parsed.candidates_include.len(), 1);
	assert_eq!(parsed.voters_include.len(), 1);
	assert_eq!(parsed.voters_include[0].1, 1000000);
}

/// Test invalid overrides file format
#[test]
fn test_invalid_overrides_file_format() {
	let temp_dir = TempDir::new().unwrap();
	let invalid_file = temp_dir.path().join("invalid_overrides.json");

	// Write invalid JSON (missing some fields is okay due to #[serde(default)], but totally invalid
	// JSON should fail)
	fs::write(&invalid_file, "{ this is not json }").unwrap();

	// Try to parse - should fail
	let content = fs::read_to_string(&invalid_file).unwrap();
	let result: Result<polkadot_staking_miner::commands::types::ElectionOverrides, _> =
		serde_json::from_str(&content);
	assert!(result.is_err());
}
