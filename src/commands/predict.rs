//! Predict command implementation for election prediction
use polkadot_sdk::pallet_election_provider_multi_block::unsigned::miner::MinerConfig;

use crate::{
	client::Client,
	commands::types::{CustomElectionFile, ElectionDataSource, PredictConfig},
	dynamic::{
		election_data::{
			PredictionContext, build_predictions_from_solution, convert_staking_data_to_snapshots,
			get_election_data,
		},
		multi_block::mine_solution,
		update_metadata_constants,
	},
	error::Error,
	prelude::{AccountId, LOG_TARGET},
	runtime::multi_block::{self as runtime},
	static_types::multi_block::Pages,
	utils::{get_chain_properties, read_data_from_json_file, write_data_to_json_file},
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
	log::info!(target: LOG_TARGET, "Running prediction command...");

	// Update metadata constants
	update_metadata_constants(client.chain_api())?;
	crate::dynamic::set_balancing_iterations(config.balancing_iterations);

	let n_pages = Pages::get();

	let storage = client.chain_api().storage().at_latest().await?;

	let current_round = storage
		.fetch_or_default(&runtime::storage().multi_block_election().round())
		.await?;

	let block_number = client
		.chain_api()
		.blocks()
		.at_latest()
		.await
		.map_err(|e| Error::Other(format!("Failed to fetch latest block number: {e}")))?
		.number();

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

	let do_reduce = true;

	// Check if custom file is provided
	let (targets, voters, data_source) = if let Some(path) = &config.custom_file {
		let (candidates, nominators) = load_custom_file(path).await?;
		let (target_snapshot, voter_snapshot) =
			convert_staking_data_to_snapshots::<T>(candidates, nominators)?;
		(target_snapshot, voter_snapshot, ElectionDataSource::CustomFile)
	} else {
		let (target_snapshot, voter_snapshot, source) =
			get_election_data::<T>(&client, n_pages, current_round, storage).await?;
		(target_snapshot, voter_snapshot, source)
	};

	// Take the minimum of targets
	let desired_targets = std::cmp::min(desired_targets, (targets.len().saturating_sub(1)) as u32);

	log::info!(
		target: LOG_TARGET,
		"Mining solution with desired_targets={}, candidates={}, nominators={}",
		desired_targets,
		targets.len(),
		voters.len()
	);

	let target_snapshot_for_mining = targets.clone();
	let voter_pages_for_mining = voters.clone();
	// Use actual voter page count, not the chain's max pages
	let actual_pages = voters.len() as u32;
	let paged_raw_solution = mine_solution::<T>(
		target_snapshot_for_mining,
		voter_pages_for_mining,
		actual_pages,
		current_round,
		desired_targets,
		block_number,
		do_reduce,
	)
	.await?;

	let (ss58_prefix, token_decimals, token_symbol) = get_chain_properties(&client).await?;

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
		&targets,
		&voters,
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

async fn load_custom_file(
	custom_file_path: &str,
) -> Result<(Vec<(AccountId, u128)>, Vec<(AccountId, u64, Vec<AccountId>)>), Error> {
	use std::path::PathBuf;

	// Resolve relative path → absolute
	let path: PathBuf = {
		let p = PathBuf::from(custom_file_path);
		if p.is_absolute() {
			p
		} else {
			std::env::current_dir()
				.map_err(|e| Error::Other(format!("Failed to get current directory: {e}")))?
				.join(p)
		}
	};

	log::info!(target: LOG_TARGET, "Reading election data from custom file: {}", path.display());

	if !path.exists() {
		return Err(Error::Other(format!("Custom file not found: {}", path.display())));
	}

	// Read file
	let custom_data: CustomElectionFile = read_data_from_json_file(
		path.to_str().ok_or_else(|| Error::Other("Invalid custom file path".into()))?,
	)
	.await
	.map_err(|e| Error::Other(format!("Failed to read custom file: {e}")))?;

	log::info!(
		target: LOG_TARGET,
		"Loaded {} candidates and {} nominators from custom file",
		custom_data.candidates.len(),
		custom_data.nominators.len()
	);

	// Convert directly using iterators (more idiomatic)
	let candidates = custom_data
		.candidates
		.into_iter()
		.map(|c| {
			Ok((
				c.account
					.parse::<AccountId>()
					.map_err(|e| Error::Other(format!("Invalid candidate {}: {}", c.account, e)))?,
				c.stake,
			))
		})
		.collect::<Result<Vec<_>, Error>>()?;

	let nominators = custom_data
		.nominators
		.into_iter()
		.map(|n| {
			let account = n
				.account
				.parse::<AccountId>()
				.map_err(|e| Error::Other(format!("Invalid nominator {}: {}", n.account, e)))?;

			let targets = n
				.targets
				.into_iter()
				.map(|t| {
					t.parse::<AccountId>()
						.map_err(|e| Error::Other(format!("Invalid target {t}: {e}")))
				})
				.collect::<Result<Vec<_>, Error>>()?;

			Ok((account, n.stake, targets))
		})
		.collect::<Result<Vec<_>, Error>>()?;

	Ok((candidates, nominators))
}
