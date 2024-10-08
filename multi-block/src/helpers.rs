use subxt::storage::Storage;

use crate::{error::Error, prelude::*};
use codec::{Decode, Encode};

/// Helper to get storage at block.
pub async fn storage_at(
	block: Option<Hash>,
	api: &ChainClient,
) -> Result<Storage<Config, ChainClient>, Error> {
	if let Some(block_hash) = block {
		Ok(api.storage().at(block_hash))
	} else {
		api.storage().at_latest().await.map_err(Into::into)
	}
}
