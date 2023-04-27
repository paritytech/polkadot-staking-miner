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

use crate::{epm, error::Error, opt::DryRunConfig, prelude::*, signer::Signer, static_types};
use codec::Encode;

pub async fn dry_run_cmd<T>(api: SubxtClient, config: DryRunConfig) -> Result<(), Error>
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

	let (solution, score, _size) = epm::fetch_snapshot_and_mine_solution::<T>(
		&api,
		config.at,
		config.solver,
		round,
		config.force_winner_count,
	)
	.await?;

	let round = api
		.storage()
		.at(config.at)
		.await?
		.fetch(&runtime::storage().election_provider_multi_phase().round())
		.await?
		.unwrap_or(1);

	let raw_solution = RawSolution { solution, score, round };

	log::info!(
		target: LOG_TARGET,
		"solution score {:?} / length {:?}",
		score,
		raw_solution.encode().len(),
	);

	// If an account seed or path is provided, then do a dry run to the node. Otherwise,
	// we've logged the solution above and we do nothing else.
	if let Some(seed_or_path) = &config.seed_or_path {
		let signer = Signer::new(seed_or_path)?;
		let account_info = api
			.storage()
			.at(None)
			.await?
			.fetch(&runtime::storage().system().account(signer.account_id()))
			.await?
			.ok_or(Error::AccountDoesNotExists)?;

		log::info!(target: LOG_TARGET, "Loaded account {}, {:?}", signer, account_info);

		let nonce = api.rpc().system_account_next_index(signer.account_id()).await?;
		let tx = epm::signed_solution(raw_solution)?;
		let xt =
			api.tx()
				.create_signed_with_nonce(&tx, &*signer, nonce, ExtrinsicParams::default())?;
		let outcome = api.rpc().dry_run(xt.encoded(), config.at).await?;

		log::info!(target: LOG_TARGET, "dry-run outcome is {:?}", outcome);

		outcome.map_err(|e| Error::Other(format!("{e:?}")))?;
	}

	Ok(())
}
