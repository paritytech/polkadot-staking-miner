//! Predict command implementation for election prediction
use polkadot_sdk::pallet_election_provider_multi_block::unsigned::miner::MinerConfig;

use crate::{
	client::Client,
	commands::types::{ElectionDataSource, ElectionOverrides, PredictConfig},
	dynamic::{
		election_data::{
			PredictionContext, apply_overrides, build_predictions_from_solution,
			convert_election_data_to_snapshots, get_election_data,
		},
		multi_block::mine_solution,
		update_metadata_constants,
	},
	error::Error,
	prelude::{AccountId, LOG_TARGET},
	runtime::multi_block::{self as runtime},
	static_types::multi_block::Pages,
	utils::{
		TimedFuture, get_block_hash, get_chain_properties, read_data_from_json_file,
		write_data_to_json_file,
	},
};

/// Run the election prediction with the given configuration
pub async fn predict_cmd<T>(client: Client, config: PredictConfig) -> Result<(), Error>
where
	T: MinerConfig<AccountId = AccountId> + Send + Sync + 'static,
	T::Solution: Send,
	T::Pages: Send,
	T::TargetSnapshotPerBlock: Send,
	T::VoterSnapshotPerBlock: Send,
	T::MaxVotesPerVoter: Send,
{
	// Update metadata constants
	update_metadata_constants(&*client.chain_api().await)?;
	crate::dynamic::set_balancing_iterations(config.balancing_iterations);
	crate::dynamic::set_algorithm(config.algorithm);

	let n_pages = Pages::get();

	// Determine block number: use provided or latest
	let block_number = if let Some(block_num) = config.block_number {
		block_num
	} else {
		client
			.chain_api()
			.await
			.blocks()
			.at_latest()
			.await
			.map_err(|e| Error::Other(format!("Failed to fetch latest block number: {e}")))?
			.number()
	};

	log::info!(target: LOG_TARGET, "Using block number: {block_number}");

	// Get storage at the specified block number
	let storage = if let Some(block_num) = config.block_number {
		// Get block hash from block number
		let block_hash = get_block_hash(&client, block_num).await?;

		crate::utils::storage_at(Some(block_hash), &*client.chain_api().await).await?
	} else {
		client.chain_api().await.storage().at_latest().await?
	};

	let current_round = storage
		.fetch_or_default(&runtime::storage().multi_block_election().round())
		.await?;

	let desired_targets = match config.desired_validators {
		Some(targets) => targets,
		None => {
			// Fetch from chain
			storage
				.fetch(&runtime::storage().staking().validator_count())
				.await
				.map_err(|e| {
					Error::Other(format!("Failed to fetch Desired Targets from chain: {e}"))
				})?
				.expect("Error in fetching desired validators from chain")
		},
	};

	// Fetch election data
	let (candidates, nominators, data_source) =
		get_election_data::<T>(n_pages, current_round, storage).await?;

	// Apply overrides if provided
	let (candidates, nominators) = if let Some(overrides_path) = &config.overrides {
		log::info!(target: LOG_TARGET, "Applying overrides from {overrides_path}");
		let overrides: ElectionOverrides = read_data_from_json_file(overrides_path).await?;
		apply_overrides(candidates, nominators, overrides)?
	} else {
		(candidates, nominators)
	};

	// Convert raw data to snapshots
	let (target_snapshot, mut voter_snapshot) =
		convert_election_data_to_snapshots::<T>(candidates, nominators)?;

	// Fix the order for staking data source
	if matches!(data_source, ElectionDataSource::Staking) {
		voter_snapshot.reverse();
	}

	log::info!(
		target: LOG_TARGET,
		"Mining solution with desired_targets={}, candidates={}, voter pages={}",
		desired_targets,
		target_snapshot.len(),
		voter_snapshot.len()
	);

	// Use actual voter page count, not the chain's max pages
	// Staking data may have added some pages
	let n_pages = n_pages.max(voter_snapshot.len() as u32);

	// Mine the solution with timeout to prevent indefinite hanging
	const MINING_TIMEOUT_SECS: u64 = 600; // 10 minutes
	log::debug!(target: LOG_TARGET, "Mining solution for block #{block_number} round {current_round}");

	let paged_raw_solution = match tokio::time::timeout(
		std::time::Duration::from_secs(MINING_TIMEOUT_SECS),
		mine_solution::<T>(
			target_snapshot.clone(),
			voter_snapshot.clone(),
			n_pages,
			current_round,
			desired_targets,
			block_number,
			config.do_reduce,
		)
		.timed(),
	)
	.await
	{
		Ok((Ok(sol), dur)) => {
			log::info!(target: LOG_TARGET, "Mining solution took {}ms for block #{}", dur.as_millis(), block_number);
			sol
		},
		Ok((Err(e), dur)) => {
			log::error!(target: LOG_TARGET, "Mining failed after {}ms: {:?}", dur.as_millis(), e);
			return Err(e);
		},
		Err(_) => {
			log::error!(target: LOG_TARGET, "Mining solution timed out after {MINING_TIMEOUT_SECS} seconds for block #{block_number}");
			return Err(Error::Timeout(crate::error::TimeoutError::Mining {
				timeout_secs: MINING_TIMEOUT_SECS,
			}));
		},
	};

	let (ss58_prefix, token_decimals, token_symbol) = get_chain_properties(client.clone()).await?;

	let prediction_ctx = PredictionContext {
		round: current_round,
		desired_targets,
		block_number,
		ss58_prefix,
		token_decimals,
		token_symbol: &token_symbol,
		data_source: data_source.clone(),
	};

	let (validators_prediction, nominators_prediction) = build_predictions_from_solution::<T>(
		&paged_raw_solution,
		&target_snapshot,
		&voter_snapshot,
		&prediction_ctx,
	)?;

	// Determine output file paths
	// Create output directory if it doesn't exist
	let output_dir = std::path::Path::new(&config.output_dir);
	std::fs::create_dir_all(output_dir).map_err(|e| {
		Error::Other(format!("Failed to create output directory {}: {}", output_dir.display(), e))
	})?;

	let validators_output = output_dir.join("validators_prediction.json");
	let nominators_output = output_dir.join("nominators_prediction.json");
	// Save validators prediction
	write_data_to_json_file(&validators_prediction, validators_output.to_str().unwrap()).await?;

	log::info!(
		target: LOG_TARGET,
		"Validators prediction saved to {}",
		validators_output.display()
	);

	// Save nominators prediction
	write_data_to_json_file(&nominators_prediction, nominators_output.to_str().unwrap()).await?;

	log::info!(
		target: LOG_TARGET,
		"Nominators prediction saved to {}",
		nominators_output.display()
	);

	Ok(())
}
