//! Predict command implementation for election prediction

use crate::{
	client::Client,
	commands::types::{CustomElectionFile, PredictConfig},
	dynamic::{
		multi_block::{mine_solution, try_fetch_snapshot},
		staking::{
			build_predictions_from_phragmen, convert_staking_to_mine_solution_input, fetch_candidates, fetch_current_round, fetch_nominators, fetch_stakes_in_batches, fetch_validator_count, predict_election
		},
		update_metadata_constants,
	},
	error::Error,
	prelude::{AccountId, LOG_TARGET},
	static_types::multi_block::{Pages, polkadot::MinerConfig},
	utils::{get_chain_properties, get_ss58_prefix, read_data_from_json_file, write_data_to_json_file},
};

/// Run the election prediction with the given configuration
pub async fn predict_cmd(client: Client, config: PredictConfig) -> Result<(), Error> {
	log::info!(target: LOG_TARGET, "Starting election prediction tool...");

	// Update metadata constants
	update_metadata_constants(client.chain_api())?;

	let n_pages = Pages::get();

	let round = fetch_current_round(&client)
		.await
		.map_err(|e| Error::Other(format!("Failed to fetch current round: {}", e)))?;

	let desired_targets = match config.desired_validators {
		Some(targets) => targets,
		None => fetch_validator_count(&client)
			.await
			.map_err(|e| Error::Other(format!("Failed to fetch validator count: {}", e)))?,
	};

	let block_number = client
		.chain_api()
		.blocks()
		.at_latest()
		.await
		.map_err(|e| Error::Other(format!("Failed to fetch latest block number: {}", e)))?
		.number();

	// Check if custom file is provided
	let (data_source, candidates, nominators, ss58_prefix_from_file) =
		if let Some(path) = &config.custom_file {
			load_custom_file(path).await?
		} else {
			load_from_snapshot_or_chain(&client, n_pages, round, desired_targets).await?
		};

	log::info!(
		target: LOG_TARGET,
		"Mining solution with desired_targets={}, candidates={}, nominators={}",
		desired_targets,
		candidates.len(),
		nominators.len()
	);

	// Get chain-specific encoding parameters
	// Use SS58 prefix from custom file if provided, otherwise fetch from chain
	let (ss58_prefix, token_decimals, token_symbol) = if let Some(prefix) = ss58_prefix_from_file {
		// If we have SS58 prefix from file, still need to fetch token properties from chain
		let (token_decimals, token_symbol) = get_chain_properties(&client).await?;
		(prefix, token_decimals, token_symbol)
	} else {
		// Fetch everything from chain
		let prefix = get_ss58_prefix(&client).await?;
		let (token_decimals, token_symbol) = get_chain_properties(&client).await?;
		(prefix, token_decimals, token_symbol)
	};

	log::info!(
		target: LOG_TARGET,
		"Using SS58 prefix: {}, token decimals: {}, token symbol: {}",
		ss58_prefix,
		token_decimals,
		token_symbol
	);

	// in case of snapshot self_vote is already included in the nominators
	// in case of staking self_vote is not included in the nominators
	let self_vote = data_source != "snapshot";

	let election_result = predict_election(
		desired_targets,
		&candidates,
		&nominators,
		self_vote,
	)?;

	// Run election using seq_phragmen directly
	let (mut validators_prediction, mut nominators_prediction) = build_predictions_from_phragmen(
		election_result,
		desired_targets,
		&candidates,
		&nominators,
		ss58_prefix,
		token_decimals,
		&token_symbol,
	);

	let do_reduce = true;

	// Mine Solution for submitting to the chain
	// only `mine_solution` if we have enough candidates
	let solution_score = if candidates.len() >= desired_targets as usize {
		let _mine_solution_input = convert_staking_to_mine_solution_input::<MinerConfig>(candidates.clone(), nominators.clone())?;
		let paged_raw_solution = mine_solution::<MinerConfig>(_mine_solution_input.0, _mine_solution_input.1, n_pages, round, desired_targets, block_number, false).await?;
		Some(paged_raw_solution.score)
	}else {
		log::info!(
			target: LOG_TARGET,
			"Skipping mine_solution: only {} candidates available but {} desired, setting score to None",
			candidates.len(),
			desired_targets
		);
		None
	};

	// Update metadata with actual values
	validators_prediction.metadata.block_number = block_number;
	validators_prediction.metadata.data_source = data_source.clone();
	validators_prediction.metadata.round = round;
	validators_prediction.metadata.solution_score = solution_score;
	nominators_prediction.metadata.block_number = block_number;
	nominators_prediction.metadata.data_source = data_source.clone();
	nominators_prediction.metadata.round = round;
	nominators_prediction.metadata.solution_score = solution_score;

	// Determine output file paths
	// Create output directory if it doesn't exist
	let output_dir = std::path::Path::new(&config.output_dir);
	std::fs::create_dir_all(output_dir)
		.map_err(|e| Error::Other(format!("Failed to create output directory {}: {}", output_dir.display(), e)))?;

	let validators_output = output_dir.join("validators_prediction.json");
	let nominators_output = output_dir.join("nominators_prediction.json");
	// Save validators prediction
	write_data_to_json_file(&validators_prediction, validators_output.to_str().unwrap())
		.await
		.map_err(|e| Error::Other(format!("Failed to write validators prediction: {}", e)))?;

	log::info!(
		target: LOG_TARGET,
		"Validators prediction saved to {}",
		validators_output.display()
	);

	// Save nominators prediction
	write_data_to_json_file(&nominators_prediction, nominators_output.to_str().unwrap())
		.await
		.map_err(|e| Error::Other(format!("Failed to write nominators prediction: {}", e)))?;

	log::info!(
		target: LOG_TARGET,
		"Nominators prediction saved to {}",
		nominators_output.display()
	);

	Ok(())
}

async fn load_custom_file(
    custom_file_path: &str,
) -> Result<(String, Vec<(AccountId, u128)>, Vec<(AccountId, u64, Vec<AccountId>)>, Option<u16>), Error> 
{
    use std::path::PathBuf;

    // Resolve relative path → absolute
    let path: PathBuf = {
        let p = PathBuf::from(custom_file_path);
        if p.is_absolute() {
            p
        } else {
            std::env::current_dir()
                .map_err(|e| Error::Other(format!("Failed to get current directory: {}", e)))?
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
    .map_err(|e| Error::Other(format!("Failed to read custom file: {}", e)))?;

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
            Ok((c.account.parse::<AccountId>()
                .map_err(|e| Error::Other(format!("Invalid candidate {}: {}", c.account, e)))?,
                c.stake))
        })
        .collect::<Result<Vec<_>, Error>>()?;

    let nominators = custom_data
        .nominators
        .into_iter()
        .map(|n| {
            let account = n.account.parse::<AccountId>()
                .map_err(|e| Error::Other(format!("Invalid nominator {}: {}", n.account, e)))?;

            let targets = n.targets.into_iter()
                .map(|t| {
                    t.parse::<AccountId>()
                        .map_err(|e| Error::Other(format!("Invalid target {}: {}", t, e)))
                })
                .collect::<Result<Vec<_>, Error>>()?;

            Ok((account, n.stake, targets))
        })
        .collect::<Result<Vec<_>, Error>>()?;

    Ok(("custom_file".into(), candidates, nominators, Some(custom_data.metadata.ss58_prefix)))
}

async fn load_from_snapshot_or_chain(
    client: &Client,
    n_pages: u32,
    round: u32,
    desired_targets: u32,
) -> Result<(String, Vec<(AccountId, u128)>, Vec<(AccountId, u64, Vec<AccountId>)>, Option<u16>), Error> 
{
    let storage = client
        .chain_api()
        .storage()
        .at_latest()
        .await
        .map_err(|e| Error::Other(format!("Failed to get storage client: {}", e)))?;

    match try_fetch_snapshot::<MinerConfig>(n_pages, round, desired_targets, &storage).await {
        Ok((target_snapshot, voter_pages)) => {
            log::info!(target: LOG_TARGET, "Snapshot found, fetching stake info...");

            let candidate_accounts: Vec<_> = target_snapshot.iter().cloned().collect();
			// Fetch stakes for candidates since snapshot doesn't contain stakes for candidates
            let stakes = fetch_stakes_in_batches(client, &candidate_accounts).await?;

            let candidates = candidate_accounts
                .iter()
                .zip(stakes)
                .map(|(acc, stake)| (acc.clone(), stake))
                .collect();

            let nominators = voter_pages
                .into_iter()
                .flat_map(|page| {
                    page.into_iter().map(|(stash, stake, votes)| {
                        (stash, stake, votes.into_iter().collect())
                    })
                })
                .collect();

            Ok(("snapshot".into(), candidates, nominators, None))
        }
        Err(err) => {
            log::warn!(target: LOG_TARGET, "Snapshot failed: {}. Falling back to staking pallet", err);

            let candidates = fetch_candidates(client)
                .await
                .map_err(|e| Error::Other(format!("Failed to fetch candidates: {}", e)))?;

            let nominators = fetch_nominators(client)
                .await
                .map_err(|e| Error::Other(format!("Failed to fetch nominators: {}", e)))?;

            Ok(("staking".into(), candidates, nominators, None))
        }
    }
}
