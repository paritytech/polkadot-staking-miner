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
	let mut signer = Signer::new(&config.seed_or_path)?;

	let account_info = api
		.storage()
		.fetch(&runtime::storage().system().account(signer.account_id()), None)
		.await?
		.ok_or(Error::AccountDoesNotExists)?;

	log::info!(target: LOG_TARGET, "Loaded account {}, {:?}", signer, account_info);

	let (solution, score, _size) =
		epm::fetch_snapshot_and_mine_solution::<T>(&api, config.at, config.solver).await?;

	let round = api
		.storage()
		.fetch(&runtime::storage().election_provider_multi_phase().round(), config.at)
		.await?
		.expect("The round must exist");

	let raw_solution = RawSolution { solution, score, round };
	let nonce = api.rpc().system_account_next_index(signer.account_id()).await?;
	signer.set_nonce(nonce);

	log::info!(
		target: LOG_TARGET,
		"solution score {:?} / length {:?}",
		score,
		raw_solution.encode().len(),
	);

	let tx = epm::signed_solution(raw_solution)?;
	let xt = api.tx().create_signed(&tx, &*signer, ExtrinsicParams::default()).await?;

	let outcome = api.rpc().dry_run(xt.encoded(), config.at).await?;

	log::info!(target: LOG_TARGET, "dry-run outcome is {:?}", outcome);

	match outcome {
		Ok(Ok(())) => Ok(()),
		Ok(Err(e)) => Err(Error::Other(format!("{:?}", e))),
		Err(e) => Err(Error::Other(format!("{:?}", e))),
	}
}
