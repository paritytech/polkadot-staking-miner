// Copyright 2021-2022 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

//! The emergency-solution command.

use crate::{epm, error::Error, opt::Solver, prelude::*, static_types};
use clap::Parser;
use codec::Encode;
use std::io::Write;

#[derive(Debug, Clone, Parser)]
#[cfg_attr(test, derive(PartialEq))]
pub struct EmergencySolutionConfig {
	/// The block hash at which scraping happens. If none is provided, the latest head is used.
	#[clap(long)]
	pub at: Option<Hash>,

	/// The solver algorithm to use.
	#[clap(subcommand)]
	pub solver: Solver,

	/// The number of top backed winners to take instead. All are taken, if not provided.
	pub force_winner_count: Option<u32>,
}

pub async fn emergency_solution_cmd<T>(
	api: SubxtClient,
	config: EmergencySolutionConfig,
) -> Result<(), Error>
where
	T: MinerConfig<AccountId = AccountId, MaxVotesPerVoter = static_types::MaxVotesPerVoter>
		+ Send
		+ Sync
		+ 'static,
	T::Solution: Send,
{
	let round = api
		.storage()
		.at(config.at)
		.await?
		.fetch_or_default(&runtime::storage().election_provider_multi_phase().round())
		.await?;

	let miner_solution = epm::fetch_snapshot_and_mine_solution::<T>(
		&api,
		config.at,
		config.solver,
		round,
		config.force_winner_count,
	)
	.await?;

	let ready_solution = miner_solution.feasibility_check()?;
	let encoded_size = ready_solution.encoded_size();
	let score = ready_solution.score;

	let mut supports = ready_solution.supports.into_inner();

	// maybe truncate.
	if let Some(force_winner_count) = config.force_winner_count {
		log::info!(
			target: LOG_TARGET,
			"truncating {} winners to {}",
			supports.len(),
			force_winner_count
		);
		supports.sort_unstable_by_key(|(_, s)| s.total);
		supports.truncate(force_winner_count as usize);
	}

	// write to file and stdout.
	let encoded_supports = supports.encode();
	let mut supports_file = std::fs::File::create("solution.supports.bin")?;
	supports_file.write_all(&encoded_supports)?;

	log::info!(target: LOG_TARGET, "ReadySolution: size {:?} / score = {:?}", encoded_size, score);
	log::trace!(
		target: LOG_TARGET,
		"Supports: {:?}",
		sp_core::hexdisplay::HexDisplay::from(&encoded_supports)
	);

	Ok(())
}
