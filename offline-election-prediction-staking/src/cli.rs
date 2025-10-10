use std::{
    fs::File,
    io::{BufWriter, Write},
    path::Path,
};

use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand};

use crate::{
    types::{
        DetailedPredictionResult, ElectionData, AccountId, Balance, 
        PredictionOutput, PredictionMetadata, PredictionResults, 
        ValidatorInfo, PredictionStatistics
    },
    data_fetcher::DataFetcher
};

/// Main CLI structure
#[derive(Debug, Clone, Parser)]
#[command(name = "offline-election")]
#[command(about = "A tool for predicting Substrate-based blockchain elections")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

/// Available commands
#[derive(Debug, Clone, Subcommand)]
pub enum Commands {
    /// Run election prediction
    Predict(PredictCliConfig),
}

/// CLI configuration for election prediction
#[derive(Debug, Clone, Parser, PartialEq)]
pub struct PredictCliConfig {
    /// Chain WebSocket endpoint URI
    #[clap(long, default_value = "wss://westend-asset-hub-rpc.polkadot.io")]
    pub chain_uri: String,

    /// Desired number of validators for the prediction
    #[clap(long)]
    pub desired_validators: Option<u32>,

    /// Path to custom nominators and validators JSON file
    #[clap(long)]
    pub custom_nominators_validators: Option<String>,

    /// Output file for prediction results
    #[clap(long, default_value = "election_prediction.json")]
    pub output: String,

    /// Use cached data instead of fetching from chain
    #[clap(long)]
    pub use_cached_data: bool,

    /// Path to cached data directory
    #[clap(long, default_value = "./cache")]
    pub cache_dir: String,
}

impl PredictCliConfig {
    /// Load election data based on CLI configuration
    pub async fn load_election_data(&self, fetcher: &DataFetcher) -> Result<ElectionData> {
        if self.use_cached_data {
            println!("Using cached data from: {}", self.cache_dir);
            fetcher.read_election_data_from_files().await
        } else if let Some(custom_file) = &self.custom_nominators_validators {
            println!("Using custom data from: {}", custom_file);
            load_custom_election_data(fetcher, custom_file).await
        } else {
            println!("Fetching data from chain: {} (trying snapshot first, fallback to Staking pallet)", self.chain_uri);
            fetcher.fetch_election_data_with_snapshot().await
        }
    }

    /// Create a new DataFetcher with the configured cache directory
    pub async fn create_data_fetcher(&self) -> Result<DataFetcher> {
        DataFetcher::new(&self.chain_uri, &self.cache_dir).await
    }

    /// Get the desired validators count, using CLI override if provided
    pub fn get_desired_validators(&self, chain_desired_validators: u32) -> u32 {
        self.desired_validators.unwrap_or(chain_desired_validators)
    }
}

/// Load custom election data from a JSON file
async fn load_custom_election_data(
    fetcher: &DataFetcher,
    custom_file: &str,
) -> Result<ElectionData> {
    // For now, fall back to reading from files
    // In a real implementation, you would parse the custom JSON file
    // and convert it to the ElectionData format
    println!("Loading custom data from: {}", custom_file);
    
    // Check if the custom file exists
    if !Path::new(custom_file).exists() {
        return Err(anyhow!("Custom file not found: {}", custom_file));
    }
    
    // For now, we'll use the existing file reading logic
    // You can extend this to parse custom JSON formats
    fetcher.read_election_data_from_files().await
}

/// Convert detailed prediction result to new output format
pub fn convert_to_new_format(
    result: &DetailedPredictionResult, 
    desired_validators: u32,
    election_data: &ElectionData,
) -> Result<PredictionOutput> {
    let timestamp = chrono::Utc::now().to_rfc3339();
    
    // Convert members to ValidatorInfo
    let mut active_validators = Vec::new();
    let mut total_staked = 0u128;
    let mut stakes = Vec::new();
    
    for (account, stake) in &result.prediction.members {
        // Calculate self stake (this is a simplified approach - in reality you'd need to look up the validator's own stake)
        let self_stake = calculate_self_stake(account, election_data);
        
        // Count nominators for this validator
        let _nominators_count = count_nominators_for_validator(account, election_data);
        
        active_validators.push(ValidatorInfo {
            account: account.to_string(),
            total_stake: *stake,
            self_stake
        });
        
        total_staked += stake;
        stakes.push(*stake);
    }
    
    // Calculate statistics
    let minimum_stake = stakes.iter().min().copied().unwrap_or(0);
    let average_stake = if !stakes.is_empty() {
        total_staked / stakes.len() as u128
    } else {
        0
    };
    
    let statistics = PredictionStatistics {
        minimum_stake,
        average_stake,
        total_staked,
    };
    
    Ok(PredictionOutput {
        metadata: PredictionMetadata {
            timestamp,
            desired_validators,
        },
        results: PredictionResults {
            active_validators,
            statistics,
        },
    })
}

/// Calculate self stake for a validator (simplified implementation)
fn calculate_self_stake(validator: &AccountId, election_data: &ElectionData) -> Balance {
    // Look for the validator in candidates to get their self stake
    for (account, stake) in &election_data.candidates {
        if account == validator {
            return *stake;
        }
    }
    0 // If not found in candidates, assume no self stake
}

/// Count nominators for a specific validator
fn count_nominators_for_validator(validator: &AccountId, election_data: &ElectionData) -> u32 {
    let mut count = 0;
    for (_, _, targets) in &election_data.nominators {
        if targets.contains(validator) {
            count += 1;
        }
    }
    count
}

/// Save prediction results to a JSON file using new format
pub fn save_prediction_results(
    result: &DetailedPredictionResult, 
    desired_validators: u32,
    election_data: &ElectionData,
    output_path: &str
) -> Result<()> {
    let file = File::create(output_path)
        .context("Failed to create output file")?;
    let mut writer = BufWriter::new(file);
    
    let new_format = convert_to_new_format(result, desired_validators, election_data)?;
    let json = serde_json::to_string_pretty(&new_format)
        .context("Failed to serialize prediction results")?;
    
    writer.write_all(json.as_bytes())
        .context("Failed to write prediction results")?;
    writer.flush()
        .context("Failed to flush writer")?;
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{str::FromStr, collections::HashMap};
    use tempfile::TempDir;
    use crate::types::{PredictedElectionResult, DetailedPredictionResult};

    fn create_test_account_id() -> AccountId {
        AccountId::from_str("5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY").unwrap()
    }

    fn create_test_election_data() -> ElectionData {
        let account1 = create_test_account_id();
        ElectionData {
            active_era: 1000,
            desired_validators: 19,
            desired_runners_up: 16,
            candidates: vec![(account1.clone(), 1000000u128)],
            nominators: vec![(account1.clone(), 500000u64, vec![account1.clone()])],
        }
    }

    #[test]
    fn test_predict_cli_config_defaults() {
        let config = PredictCliConfig {
            chain_uri: "wss://test.example.com".to_string(),
            desired_validators: None,
            custom_nominators_validators: None,
            output: "test_output.json".to_string(),
            use_cached_data: false,
            cache_dir: "./test_cache".to_string(),
        };

        assert_eq!(config.chain_uri, "wss://test.example.com");
        assert_eq!(config.desired_validators, None);
        assert_eq!(config.custom_nominators_validators, None);
        assert_eq!(config.output, "test_output.json");
        assert_eq!(config.use_cached_data, false);
        assert_eq!(config.cache_dir, "./test_cache");
    }

    #[test]
    fn test_get_desired_validators_with_override() {
        let config = PredictCliConfig {
            chain_uri: "wss://test.example.com".to_string(),
            desired_validators: Some(100),
            custom_nominators_validators: None,
            output: "test_output.json".to_string(),
            use_cached_data: false,
            cache_dir: "./test_cache".to_string(),
        };

        let result = config.get_desired_validators(19);
        assert_eq!(result, 100);
    }

    #[test]
    fn test_get_desired_validators_without_override() {
        let config = PredictCliConfig {
            chain_uri: "wss://test.example.com".to_string(),
            desired_validators: None,
            custom_nominators_validators: None,
            output: "test_output.json".to_string(),
            use_cached_data: false,
            cache_dir: "./test_cache".to_string(),
        };

        let result = config.get_desired_validators(19);
        assert_eq!(result, 19);
    }

    #[test]
    fn test_convert_to_new_format() {
        let account1 = create_test_account_id();
        let election_data = create_test_election_data();
        
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
                let mut map = HashMap::new();
                map.insert(account1.to_string(), 1000000u128);
                map
            },
        };

        let result = convert_to_new_format(&detailed_result, 19, &election_data);
        assert!(result.is_ok());
        
        let output = result.unwrap();
        assert_eq!(output.metadata.desired_validators, 19);
        assert_eq!(output.results.active_validators.len(), 1);
        assert_eq!(output.results.statistics.total_staked, 1000000u128);
    }

    #[test]
    fn test_calculate_self_stake_found() {
        let account1 = create_test_account_id();
        let election_data = ElectionData {
            active_era: 1000,
            desired_validators: 19,
            desired_runners_up: 16,
            candidates: vec![(account1.clone(), 1000000u128)],
            nominators: vec![],
        };

        let self_stake = calculate_self_stake(&account1, &election_data);
        assert_eq!(self_stake, 1000000u128);
    }

    #[test]
    fn test_calculate_self_stake_not_found() {
        let account1 = create_test_account_id();
        let account2 = AccountId::from_str("5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty").unwrap();
        
        let election_data = ElectionData {
            active_era: 1000,
            desired_validators: 19,
            desired_runners_up: 16,
            candidates: vec![(account1, 1000000u128)],
            nominators: vec![],
        };

        let self_stake = calculate_self_stake(&account2, &election_data);
        assert_eq!(self_stake, 0u128);
    }

    #[test]
    fn test_count_nominators_for_validator() {
        let account1 = create_test_account_id();
        let account2 = AccountId::from_str("5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty").unwrap();
        let account3 = AccountId::from_str("5DAAnrj7VHTznn2AWBemMuyBwZWs6FNFjdyVXUeYum3PTXFy").unwrap();
        
        let election_data = ElectionData {
            active_era: 1000,
            desired_validators: 19,
            desired_runners_up: 16,
            candidates: vec![],
            nominators: vec![
                (account1.clone(), 1000000u64, vec![account2.clone()]),
                (account2.clone(), 2000000u64, vec![account2.clone(), account3.clone()]),
                (account3.clone(), 3000000u64, vec![account3.clone()]),
            ],
        };

        let count = count_nominators_for_validator(&account2, &election_data);
        assert_eq!(count, 2); // account1 and account2 nominate account2

        let count = count_nominators_for_validator(&account3, &election_data);
        assert_eq!(count, 2); // account2 and account3 nominate account3

        let count = count_nominators_for_validator(&account1, &election_data);
        assert_eq!(count, 0); // no one nominates account1
    }

    #[test]
    fn test_save_prediction_results() {
        let temp_dir = TempDir::new().unwrap();
        let output_path = temp_dir.path().join("test_output.json");
        
        let account1 = create_test_account_id();
        let election_data = create_test_election_data();
        
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
                let mut map = HashMap::new();
                map.insert(account1.to_string(), 1000000u128);
                map
            },
        };

        let result = save_prediction_results(&detailed_result, 19, &election_data, output_path.to_str().unwrap());
        assert!(result.is_ok());

        // Verify file was created and contains expected content
        assert!(output_path.exists());
        let content = std::fs::read_to_string(&output_path).unwrap();
        assert!(content.contains("1000000"));
        assert!(content.contains("19"));
    }

    #[test]
    fn test_save_prediction_results_invalid_path() {
        let account1 = create_test_account_id();
        let election_data = create_test_election_data();
        
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
                let mut map = HashMap::new();
                map.insert(account1.to_string(), 1000000u128);
                map
            }
        };

        // Try to save to an invalid path (directory that doesn't exist)
        let result = save_prediction_results(&detailed_result, 19, &election_data, "/invalid/path/that/does/not/exist.json");
        assert!(result.is_err());
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
}
