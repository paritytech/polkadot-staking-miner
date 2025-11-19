//! This module contains types, helper functions, and APIs to
//! interact with the runtime related to submitting solutions to the validator set in polkadot-sdk.
//!
//! These are built on top of the dynamic APIs in subxt to read storage, submit transactions, read
//! events and read constants.
//!
//! The reason behind this is that subxt's static codegen generates a concrete type for NposSolution
//! and we want to be generic over the solution type to work across different runtimes.

use crate::{error::Error, prelude::ChainClient, static_types};

pub mod multi_block;
pub mod pallet_api;
pub mod utils;
pub mod staking;

use static_types::multi_block::{
	BalancingIterations, MaxBackersPerWinner, MaxLength, MaxWinnersPerPage, Pages,
	TargetSnapshotPerBlock, VoterSnapshotPerBlock,
};

/// Read the constants from the metadata and updates the static types.
pub fn update_metadata_constants(api: &ChainClient) -> Result<(), Error> {
	use polkadot_sdk::sp_runtime::Percent;

	let pages: u32 = pallet_api::multi_block::constants::PAGES.fetch(api)?;
	Pages::set(pages);
	TargetSnapshotPerBlock::set(
		pallet_api::multi_block::constants::TARGET_SNAPSHOT_PER_BLOCK.fetch(api)?,
	);
	VoterSnapshotPerBlock::set(
		pallet_api::multi_block::constants::VOTER_SNAPSHOT_PER_BLOCK.fetch(api)?,
	);

	let block_len = pallet_api::system::constants::BLOCK_LENGTH.fetch(api)?;

	// As instructed, a reasonable default max length is 75% of the total block length.
	MaxLength::set(Percent::from_percent(75) * block_len.total());
	MaxWinnersPerPage::set(
		pallet_api::multi_block_verifier::constants::MAX_WINNERS_PER_PAGE.fetch(api)?,
	);
	MaxBackersPerWinner::set(
		pallet_api::multi_block_verifier::constants::MAX_BACKERS_PER_WINNER.fetch(api)?,
	);

	Ok(())
}

/// Set the balancing iterations from CLI config.
pub fn set_balancing_iterations(iterations: usize) {
	BalancingIterations::set(iterations);
}
