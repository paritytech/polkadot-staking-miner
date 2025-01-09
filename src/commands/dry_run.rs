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

//! The dry-run command.

use pallet_election_provider_multi_phase::RawSolution;
use subxt::config::DefaultExtrinsicParamsBuilder;

use crate::{
	client::Client, epm, error::Error, helpers::storage_at, opt::Solver, prelude::*,
	signer::Signer, static_types,
};
use clap::Parser;
use codec::Encode;

#[derive(Debug, Clone, Parser)]
#[cfg_attr(test, derive(PartialEq))]
pub struct DryRunConfig {
	/// The block hash at which scraping happens. If none is provided, the latest head is used.
	#[clap(long)]
	pub at: Option<Hash>,

	/// The solver algorithm to use.
	#[clap(subcommand)]
	pub solver: Solver,

	/// Force create a new snapshot, else expect one to exist onchain.
	#[clap(long)]
	pub force_snapshot: bool,

	/// The number of winners to take, instead of the `desired_targets` in snapshot.
	// Doing this would cause the dry-run to typically fail, but that's fine, the program should
	// still print out some score, and that should be it.
	#[clap(long)]
	pub force_winner_count: Option<u32>,

	/// The path to a file containing the seed of the account. If the file is not found, the seed is
	/// used as-is. If this is not provided, we won't attempt to submit anything.
	///
	/// Can also be provided via the `SEED` environment variable.
	///
	/// WARNING: Don't use an account with a large stash for this. Based on how the bot is
	/// configured, it might re-try and lose funds through transaction fees/deposits.
	#[clap(long, short, env = "SEED")]
	pub seed_or_path: Option<String>,
}

pub async fn dry_run_cmd<T>(client: Client, config: DryRunConfig) -> Result<(), Error>
where
	T: MinerConfig<AccountId = AccountId, MaxVotesPerVoter = static_types::MaxVotesPerVoter>
		+ Send
		+ Sync
		+ 'static,
	T::Solution: Send,
{
	let storage = storage_at(config.at, client.chain_api()).await?;
	let round = storage
		.fetch_or_default(&runtime::storage().election_provider_multi_phase().round())
		.await?;

	let miner_solution = epm::fetch_snapshot_and_mine_solution::<T>(
		client.chain_api(),
		config.at,
		config.solver,
		round,
		config.force_winner_count,
	)
	.await?;

	let solution = miner_solution.solution();
	let score = miner_solution.score();
	let raw_solution = RawSolution { solution, score, round };

	log::info!(
		target: LOG_TARGET,
		"solution score {:?} / length {:?}",
		score,
		raw_solution.encode().len(),
	);

	// Now we've logged the score, check whether the solution makes sense. No point doing this
	// if force_winner_count is selected since it'll definitely fail in that case.
	if config.force_winner_count.is_none() {
		miner_solution.feasibility_check()?;
	}

	// If an account seed or path is provided, then do a dry run to the node. Otherwise,
	// we've logged the solution above and we do nothing else.
	if let Some(seed_or_path) = &config.seed_or_path {
		let signer = Signer::new(seed_or_path)?;
		let account_info = storage
			.fetch(&runtime::storage().system().account(signer.account_id()))
			.await?
			.ok_or(Error::AccountDoesNotExists)?;

		log::info!(target: LOG_TARGET, "Loaded account {} {{ nonce: {}, free_balance: {}, reserved_balance: {}, frozen_balance: {} }}",
			signer,
			account_info.nonce,
			account_info.data.free,
			account_info.data.reserved,
			account_info.data.frozen,
		);

		let nonce = client.rpc().system_account_next_index(signer.account_id()).await?;
		let tx = epm::signed_solution(raw_solution)?;
		let params = DefaultExtrinsicParamsBuilder::new().nonce(nonce).build();
		let xt = client.chain_api().tx().create_signed(&tx, &*signer, params).await?;
		let dry_run_bytes = client.rpc().dry_run(xt.encoded(), config.at).await?;
		let dry_run_result = dry_run_bytes.into_dry_run_result(&client.chain_api().metadata())?;

		log::info!(target: LOG_TARGET, "dry-run outcome is {:?}", dry_run_result);
	}

	Ok(())
}
