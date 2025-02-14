//! This module contains types, helper functions, and APIs to
//! interact with the runtime related to submitting solutions to the validator set in polkadot-sdk.
//!
//! These are built on top of the dynamic APIs in subxt to read storage, submit transactions, read events and read constants.

#[cfg(legacy)]
mod legacy;
#[cfg(experimental_multi_block)]
mod multi_block;
#[allow(dead_code, unused)]
pub(crate) mod pallet_api;
pub(crate) mod utils;

#[cfg(legacy)]
pub(crate) use legacy::*;
#[cfg(experimental_multi_block)]
pub(crate) use multi_block::*;

use crate::{error::Error, prelude::ChainClient, static_types};

/// Read the constants from the metadata and updates the static types.
pub fn update_metadata_constants(api: &ChainClient) -> Result<(), Error> {
    #[cfg(legacy)]
    {
        static_types::MaxWeight::set(
            pallet_api::election_provider_multi_phase::constants::MAX_WEIGHT.fetch(api)?,
        );
        static_types::MaxLength::set(
            pallet_api::election_provider_multi_phase::constants::MAX_LENGTH.fetch(api)?,
        );
        static_types::MaxVotesPerVoter::set(
            pallet_api::election_provider_multi_phase::constants::MAX_VOTES.fetch(api)?,
        );
        static_types::MaxWinners::set(
            pallet_api::election_provider_multi_phase::constants::MAX_WINNERS.fetch(api)?,
        );
    }

    #[cfg(experimental_multi_block)]
    {
        // multi block constants.
        let pages: u32 = pallet_api::multi_block::constants::PAGES.fetch(api)?;
        static_types::Pages::set(pages);
        static_types::TargetSnapshotPerBlock::set(
            pallet_api::multi_block::constants::TARGET_SNAPSHOT_PER_BLOCK.fetch(api)?,
        );
        static_types::VoterSnapshotPerBlock::set(
            pallet_api::multi_block::constants::VOTER_SNAPSHOT_PER_BLOCK.fetch(api)?,
        );

        // election provider constants.
        static_types::MaxBackersPerWinner::set(
            pallet_api::election_provider_multi_phase::constants::MAX_VOTES_PER_VOTER.fetch(api)?,
        );
        static_types::MaxLength::set(
            pallet_api::election_provider_multi_phase::constants::MAX_LENGTH.fetch(api)?,
        );
        static_types::MaxWinnersPerPage::set(
            pallet_api::election_provider_multi_phase::constants::MAX_WINNERS.fetch(api)?,
        );
    }

    Ok(())
}
