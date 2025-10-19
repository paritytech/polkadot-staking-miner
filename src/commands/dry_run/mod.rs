//! Dry run commands for testing and simulation.

use crate::{
	client::Client,
	commands::multi_block::types::Snapshot,
	dynamic::multi_block as dynamic,
	error::Error,
	prelude::{AccountId, LOG_TARGET},
	runtime::multi_block as runtime,
	static_types::multi_block as static_types,
	utils,
};
use polkadot_sdk::pallet_election_provider_multi_block::unsigned::miner::MinerConfig;

/// Run a dry run at a specific block with a snapshot.
pub async fn at_block_with_snapshot<T>(client: Client, block_hash_str: String) -> Result<(), Error>
where
	T: MinerConfig<AccountId = AccountId> + Send + Sync + 'static,
	T::Solution: Send + Sync + 'static,
	T::Pages: Send + Sync + 'static,
	T::TargetSnapshotPerBlock: Send + Sync + 'static,
	T::VoterSnapshotPerBlock: Send + Sync + 'static,
	T::MaxVotesPerVoter: Send + Sync + 'static,
{
	log::info!(target: LOG_TARGET, "Running dry run at block {}", block_hash_str);

	// Parse the block hash
	let block_hash: polkadot_sdk::sp_core::H256 =
		block_hash_str.parse().expect("Failed to parse block hash");

	log::info!(target: LOG_TARGET, "genesis = {:?}, runtime ={:?}", client.chain_api().genesis_hash() , client.chain_api().runtime_version());
	// Get storage at the specified block
	log::info!(target: LOG_TARGET, "Fetching storage at block {}", block_hash);
	let storage = utils::storage_at(Some(block_hash), client.chain_api())
		.await
		.expect("Failed to get storage at block");

	// Get the round number at this block
	let round = storage
		.fetch_or_default(&runtime::storage().multi_block_election().round())
		.await
		.expect("Round number not found in storage at the specified block");

	log::info!(target: LOG_TARGET, "Block round: {}", round);

	// Get desired targets
	let desired_targets = storage
		.fetch(&runtime::storage().multi_block_election().desired_targets(round))
		.await
		.expect("Failed to fetch desired targets")
		.unwrap_or(0);

	log::info!(target: LOG_TARGET, "Desired targets: {}", desired_targets);

	// Get number of pages
	let n_pages = static_types::Pages::get();

	log::info!(target: LOG_TARGET, "Number of pages: {}", n_pages);

	// Create a snapshot and fetch all the data
	let mut snapshot = Snapshot::<T>::new(n_pages);

	log::info!(target: LOG_TARGET, "Fetching snapshots for round {}...", round);
	dynamic::fetch_missing_snapshots::<T>(&mut snapshot, &storage, round)
		.await
		.expect("Failed to fetch missing snapshots");

	let (target_snapshot, voter_snapshot) = snapshot.get();

	log::info!(
		target: LOG_TARGET,
		"Snapshots fetched - targets: {}, voters across {} pages",
		target_snapshot.len(),
		voter_snapshot.len()
	);

	// Mine the solution
	log::info!(target: LOG_TARGET, "Mining solution...");
	let paged_raw_solution = dynamic::mine_solution::<T>(
		target_snapshot,
		voter_snapshot,
		n_pages,
		round,
		desired_targets,
		0,    // block_number doesn't matter for dry run
		true, // do_reduce
	)
	.await
	.expect("Failed to mine solution");

	// Print the results
	println!("\n========== DRY RUN RESULTS ==========");
	println!("Block Hash: {}", block_hash_str);
	println!("Round: {}", round);
	println!("Desired Targets: {}", desired_targets);
	println!("Number of Pages: {}", n_pages);
	println!("\nSolution Score:");
	println!("  Minimal Stake: {}", paged_raw_solution.score.minimal_stake);
	println!("  Sum Stake: {}", paged_raw_solution.score.sum_stake);
	println!("  Sum Stake Squared: {}", paged_raw_solution.score.sum_stake_squared);
	println!("\nSolution Pages: {}", paged_raw_solution.solution_pages.len());
	println!("Winner Count: {}", paged_raw_solution.winner_count_single_page_target_snapshot());
	println!("=====================================\n");

	log::info!(target: LOG_TARGET, "Dry run completed successfully");
	Ok(())
}

/// Run a dry run with the current snapshot.
pub async fn with_current_snapshot<T>(_client: Client) -> Result<(), Error>
where
	T: MinerConfig<AccountId = AccountId> + Send + Sync + 'static,
	T::Solution: Send + Sync + 'static,
	T::Pages: Send + Sync + 'static,
	T::TargetSnapshotPerBlock: Send + Sync + 'static,
	T::VoterSnapshotPerBlock: Send + Sync + 'static,
	T::MaxVotesPerVoter: Send + Sync + 'static,
{
	// TODO: Implementation to be added
	log::info!("with_current_snapshot: not yet implemented");
	Ok(())
}
