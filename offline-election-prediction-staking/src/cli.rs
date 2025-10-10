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
