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

use crate::{
    client::Client,
    commands::types::EmergencySolutionConfig,
    dynamic::legacy as dynamic,
    error::Error,
    prelude::{AccountId, LOG_TARGET},
    runtime::legacy as runtime,
    static_types::legacy as static_types,
    utils::storage_at,
};
use codec::Encode;
use polkadot_sdk::{
    pallet_election_provider_multi_phase::MinerConfig, sp_core::hexdisplay::HexDisplay,
    sp_npos_elections::Supports,
};
use std::io::Write;
use subxt::tx::Payload;

pub async fn emergency_solution_cmd<T>(
    client: Client,
    config: EmergencySolutionConfig,
) -> Result<(), Error>
where
    T: MinerConfig<AccountId = AccountId, MaxVotesPerVoter = static_types::MaxVotesPerVoter>
        + Send
        + Sync
        + 'static,
    T::Solution: Send,
{
    if let Some(max_winners) = config.force_winner_count {
        static_types::MaxWinners::set(max_winners);
    }

    let storage = storage_at(config.at, client.chain_api()).await?;

    let round = storage
        .fetch_or_default(&runtime::storage().election_provider_multi_phase().round())
        .await?;
    let solution = dynamic::fetch_snapshot_and_mine_solution::<T>(
        client.chain_api(),
        config.at,
        config.solver,
        round,
        config.force_winner_count,
    )
    .await?;

    let ready_solution = solution.feasibility_check()?;
    let encoded_size = ready_solution.encoded_size();
    let score = ready_solution.score;
    let mut supports: Supports<_> = ready_solution.supports.into();

    // Truncate if necessary.
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

    let call = dynamic::set_emergency_result(supports.clone())?;
    let encoded_call = call
        .encode_call_data(&client.chain_api().metadata())
        .map_err(|e| Error::Subxt(e.into()))?;
    let encoded_supports = supports.encode();

    // write results to files.
    let mut supports_file = std::fs::File::create("solution.supports.bin")?;
    let mut encoded_call_file = std::fs::File::create("encoded.call")?;
    supports_file.write_all(&encoded_supports)?;
    encoded_call_file.write_all(&encoded_call)?;

    let hex = HexDisplay::from(&encoded_call);
    log::info!(target: LOG_TARGET, "Hex call:\n {:?}", hex);

    log::info!(target: LOG_TARGET, "Use the hex encoded call above to construct the governance proposal or the extrinsic to submit.");
    log::info!(target: LOG_TARGET, "ReadySolution: size {:?} / score = {:?}", encoded_size, score);
    log::info!(target: LOG_TARGET, "`set_emergency_result` encoded call written to ./encoded.call");

    Ok(())
}
