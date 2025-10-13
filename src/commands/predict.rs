//! Predict command implementation for election prediction

use crate::{
	client::Client,
	commands::types::PredictConfig,
	error::Error,
	prelude::LOG_TARGET,
};
use offline_election::{
	DetailedPredictionResult, 
	ElectionPredictor, 
	DataFetcher,
	ElectionData,
	save_prediction_results
};

/// Run the election prediction with the given configuration
pub async fn predict_cmd(client: Client, config: PredictConfig) -> Result<(), Error> {
	log::info!(target: LOG_TARGET, "Starting election prediction tool...");

	// Create data fetcher using the client's URI
	let mut fetcher = DataFetcher::new(&client.uri(), &config.cache_dir).await
		.map_err(|e| Error::Other(format!("Failed to create data fetcher: {}", e)))?;

	// Load election data based on CLI options
	let election_data = if config.use_cached_data {
		load_cached_data(&mut fetcher).await?
	} else {
		fetch_election_data(&mut fetcher, config.desired_validators).await?
	};
	
	// Clone election_data to avoid borrow issues
	let election_data_clone = election_data.clone();
	
	let mut candidate_stashes = vec![];
	let mut voters = Vec::from(election_data.nominators);

	for (stash, stake) in election_data.candidates {
		voters.push((stash.clone(), stake as u64, vec![stash.clone()]));
		candidate_stashes.push(stash);
	}

	// Use CLI desired_validators if provided, otherwise use chain data
	let desired_validators = config.desired_validators.unwrap_or(election_data.desired_validators);

	// Create election predictor
	let predictor = ElectionPredictor::new(
		election_data.active_era,
		desired_validators,
		election_data.desired_runners_up,
		voters,
		candidate_stashes,
	);

	log::info!(target: LOG_TARGET, "Running election prediction...");
	log::info!(target: LOG_TARGET, "Desired validators: {}", desired_validators);

	// Run prediction with detailed analysis
	match predictor.predict_with_analysis() {
		Ok(result) => {
			log::info!(target: LOG_TARGET, "Election prediction completed successfully!");
			
			// Save results to file
			save_prediction_results(&result, desired_validators, &election_data_clone, &config.output)
				.map_err(|e| Error::Other(format!("Failed to save results: {}", e)))?;
			
			log::info!(target: LOG_TARGET, "Results saved to: {}", config.output);
			
			// Print summary
			print_prediction_summary(&result);
		},
		Err(e) => {
			log::error!(target: LOG_TARGET, "Election prediction failed: {}", e);
			return Err(Error::Other(format!("Prediction failed: {}", e)));
		}
	}

	Ok(())
}

/// Load election data from cached files
async fn load_cached_data(fetcher: &mut DataFetcher) -> Result<ElectionData, Error> {
	fetcher.read_election_data_from_files().await
		.map_err(|e| Error::Other(format!("Failed to load cached data: {}", e)))
}

/// Fetch election data from the chain
async fn fetch_election_data(fetcher: &mut DataFetcher, desired_validators: Option<u32>) -> Result<ElectionData, Error> {
	let mut election_data = fetcher.fetch_election_data().await
		.map_err(|e| Error::Other(format!("Failed to fetch election data: {}", e)))?;
	
	// Override desired validators if provided via CLI
	if let Some(desired) = desired_validators {
		election_data.desired_validators = desired;
	}
	
	Ok(election_data)
}

/// Print a summary of the prediction results
fn print_prediction_summary(result: &DetailedPredictionResult) {
	println!("\n=== Election Prediction Summary ===");
	println!("Active Era: {}", result.prediction.active_era);
	println!("Total Validators Selected: {}", result.prediction.members.len());
	println!("Total Stake: {}", result.total_stake);
	println!("Total Voters: {}", result.total_voters);
	println!("Total Candidates: {}", result.total_candidates);
	println!("=====================================\n");
}
