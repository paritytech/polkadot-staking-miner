// Copyright 2021 Parity Technologies (UK) Ltd.
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

use crate::{error::Error, opt::DryRunConfig, prelude::*, signer::Signer};
use codec::Encode;

macro_rules! dry_run_cmd_for {
	($runtime:tt) => {
		paste::paste! {
			pub async fn [<run_$runtime>](api: SubxtClient, config: DryRunConfig) -> Result<(), Error> {
				use crate::chain::[<$runtime>]::runtime as runtime;

				let mut signer = Signer::new(&config.seed_or_path)?;

				let account_info = api.storage().fetch(
					&runtime::storage().system().account(signer.account_id()),
					None
				)
				.await?
				.ok_or(Error::AccountDoesNotExists)?;

				log::info!(target: LOG_TARGET, "Loaded account {}, {:?}", signer, account_info);

				let (solution, score, _size) =
					crate::helpers::[<mine_solution_$runtime>](&api, config.at, config.solver).await?;

				let round = api.storage().fetch(
					&runtime::storage().election_provider_multi_phase().round(),
					config.at
				)
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

				let tx = runtime::tx().election_provider_multi_phase().submit(raw_solution);
				let xt = api.tx().create_signed(&tx, &*signer, ExtrinsicParams::default()).await?;

				let outcome = api
					.rpc()
					.dry_run(xt.encoded(), config.at)
					.await?;

				log::info!(target: LOG_TARGET, "dry-run outcome is {:?}", outcome);

				match outcome {
					Ok(Ok(())) => Ok(()),
					Ok(Err(e)) => Err(Error::Other(format!("{:?}", e))),
					Err(e) => Err(Error::Other(format!("{:?}", e))),
				}
			}
		}
	};
}

#[cfg(feature = "polkadot")]
dry_run_cmd_for!(polkadot);
#[cfg(feature = "kusama")]
dry_run_cmd_for!(kusama);
#[cfg(feature = "westend")]
dry_run_cmd_for!(westend);
