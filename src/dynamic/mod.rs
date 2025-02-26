//! This module contains types, helper functions, and APIs to
//! interact with the runtime related to submitting solutions to the validator set in polkadot-sdk.
//!
//! These are built on top of the dynamic APIs in subxt to read storage, submit transactions, read events and read constants.
//!
//! The reason behind this is that subxt's static codegen generates a concrete type for NposSolution and we want to be
//! generic over the solution type to work across different runtimes.

use crate::macros::{cfg_experimental_multi_block, cfg_legacy};
use crate::{error::Error, prelude::ChainClient, static_types};

#[allow(dead_code, unused)]
pub(crate) mod pallet_api;
pub(crate) mod utils;

cfg_legacy! {
    mod legacy;
    pub use legacy::*;

    /// Read the constants from the metadata and updates the static types.
    pub fn update_metadata_constants(api: &ChainClient) -> Result<(), Error> {

        static_types::MaxWeight::set(
            pallet_api::election_provider_multi_phase::constants::MAX_WEIGHT.fetch(api)?,
        );
        static_types::MaxLength::set(
            pallet_api::election_provider_multi_phase::constants::MAX_LENGTH.fetch(api)?,
        );
        static_types::MaxVotesPerVoter::set(
            pallet_api::election_provider_multi_phase::constants::MAX_VOTES_PER_VOTER.fetch(api)?,
        );
        static_types::MaxWinners::set(
            pallet_api::election_provider_multi_phase::constants::MAX_WINNERS.fetch(api)?,
        );

        Ok(())
    }
}

cfg_experimental_multi_block! {
    mod multi_block;
    pub use multi_block::*;
    use polkadot_sdk::sp_runtime::Percent;

    /// Read the constants from the metadata and updates the static types.
    pub fn update_metadata_constants(api: &ChainClient) -> Result<(), Error> {
        let pages: u32 = pallet_api::multi_block::constants::PAGES.fetch(api)?;
        static_types::Pages::set(pages);
        static_types::TargetSnapshotPerBlock::set(
            pallet_api::multi_block::constants::TARGET_SNAPSHOT_PER_BLOCK.fetch(api)?,
        );
        static_types::VoterSnapshotPerBlock::set(
            pallet_api::multi_block::constants::VOTER_SNAPSHOT_PER_BLOCK.fetch(api)?,
        );

        let block_len = pallet_api::system::constants::BLOCK_LENGTH.fetch(api)?;

        // As instructed, a reasonable default max length is 75% of the total block length.
        static_types::MaxLength::set(Percent::from_percent(75) * block_len.total());
        static_types::MaxWinnersPerPage::set(
            pallet_api::multi_block_verifier::constants::MAX_WINNERS_PER_PAGE.fetch(api)?,
        );
        static_types::MaxBackersPerWinner::set(
            pallet_api::multi_block_verifier::constants::MAX_BACKERS_PER_WINNER.fetch(api)?,
        );

        Ok(())
    }
}
