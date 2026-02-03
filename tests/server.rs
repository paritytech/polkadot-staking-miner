//! Tests for the server command

use clap::Parser;
use polkadot_staking_miner::commands::types::{
	ElectionAlgorithm, OverridesConfig, PredictConfig, ServerConfig,
};

/// Test server command help works
#[test]
fn server_help_works() {
	use assert_cmd::Command;
	let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!(env!("CARGO_PKG_NAME")));
	cmd.args(["server", "--help"]);
	cmd.assert().success();
}

/// Test ServerConfig parsing
#[test]
fn server_config_parsing() {
	// Test default port
	let config = ServerConfig::try_parse_from(["server"]).unwrap();
	assert_eq!(config.port, 8080);

	// Test custom port
	let config = ServerConfig::try_parse_from(["server", "--port", "9090"]).unwrap();
	assert_eq!(config.port, 9090);

	// Test environment variable
	unsafe { std::env::set_var("PORT", "7070") };
	let config = ServerConfig::try_parse_from(["server"]).unwrap();
	assert_eq!(config.port, 7070);
	unsafe { std::env::remove_var("PORT") };
}

/// Test OverridesConfig parsing (Path vs Data)
#[test]
fn test_overrides_config_parsing() {
	// Test path parsing (CLI style)
	let config =
		PredictConfig::try_parse_from(["predict", "--overrides", "my_overrides.json"]).unwrap();
	match config.overrides {
		Some(OverridesConfig::Path(p)) => assert_eq!(p, "my_overrides.json"),
		_ => panic!("Expected Path override"),
	}

	// Test JSON parsing (API style)
	let json_payload = r#"{
		"do_reduce": true,
		"overrides": {
			"candidates_include": ["acc1"],
			"voters_include": [["acc2", 1000, ["acc1"]]]
		}
	}"#;
	let config: PredictConfig = serde_json::from_str(json_payload).unwrap();
	match config.overrides {
		Some(OverridesConfig::Data(data)) => {
			assert_eq!(data.candidates_include, vec!["acc1".to_string()]);
			assert_eq!(data.voters_include.len(), 1);
		},
		_ => panic!("Expected Data override"),
	}
}

/// Test SimulateRunParameters serialization
#[test]
fn test_simulate_params_serialization() {
	use polkadot_staking_miner::commands::types::SimulateRunParameters;

	let params = SimulateRunParameters {
		block_number: 100,
		desired_validators: 50,
		balancing_iterations: 10,
		do_reduce: true,
		algorithm: ElectionAlgorithm::SeqPhragmen,
		overrides: Some(OverridesConfig::Path("test.json".to_string())),
	};

	let serialized = serde_json::to_string(&params).unwrap();
	let deserialized: SimulateRunParameters = serde_json::from_str(&serialized).unwrap();

	assert_eq!(deserialized.block_number, 100);
	assert_eq!(deserialized.overrides, Some(OverridesConfig::Path("test.json".to_string())));
}
