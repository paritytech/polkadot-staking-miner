//! This module contains types, helper functions, and APIs to
//! interact with the runtime related to submitting solutions to the validator set in polkadot-sdk.
//! This is a minimal version for the dummy miner.

use crate::error::Error;

pub mod multi_block;
pub mod pallet_api;
pub mod utils;

/// Dummy implementation for metadata constants update
pub fn update_metadata_constants(_api: &crate::prelude::ChainClient) -> Result<(), Error> {
	// Dummy miner doesn't need to update metadata constants
	Ok(())
}
