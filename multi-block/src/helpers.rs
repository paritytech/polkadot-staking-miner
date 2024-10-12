use subxt::{
	error::RpcError,
	storage::Storage,
	tx::{TxInBlock, TxProgress},
};

use crate::{error::Error, prelude::*};

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

/// Wait for the transaction to be in a block.
///
/// **Note:** transaction statuses like `Invalid`/`Usurped`/`Dropped` indicate with some
/// probability that the transaction will not make it into a block but there is no guarantee
/// that this is true. In those cases the stream is closed however, so you currently have no way to find
/// out if they finally made it into a block or not.
pub async fn wait_for_in_block<T, C>(
	mut tx: TxProgress<T, C>,
) -> Result<TxInBlock<T, C>, subxt::Error>
where
	T: subxt::Config + Clone,
	C: subxt::client::OnlineClientT<T> + std::fmt::Debug + Clone,
{
	use subxt::{error::TransactionError, tx::TxStatus};

	while let Some(status) = tx.next().await {
		match status? {
			// Finalized or otherwise in a block! Return.
			TxStatus::InBestBlock(s) | TxStatus::InFinalizedBlock(s) => return Ok(s),
			// Error scenarios; return the error.
			TxStatus::Error { message } => return Err(TransactionError::Error(message).into()),
			TxStatus::Invalid { message } => return Err(TransactionError::Invalid(message).into()),
			TxStatus::Dropped { message } => return Err(TransactionError::Dropped(message).into()),
			// Ignore anything else and wait for next status event:
			_ => continue,
		}
	}
	Err(RpcError::SubscriptionDropped.into())
}
