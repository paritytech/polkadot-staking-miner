use crate::prelude::{ChainClient, LOG_TARGET};
use std::{sync::Arc, time::Duration};
use subxt::backend::{
	legacy::LegacyBackend,
	rpc::reconnecting_rpc_client::{ExponentialBackoff, RpcClient as ReconnectingRpcClient},
};

/// Wraps the subxt interfaces to make it easy to use for the staking-miner.
#[derive(Clone, Debug)]
pub struct Client {
	/// Access to chain APIs such as storage, events etc.
	chain_api: ChainClient,
	/// Low-level RPC client
	rpc: ReconnectingRpcClient,
}

impl Client {
	pub async fn new(uri: &str) -> Result<Self, subxt::Error> {
		log::debug!(target: LOG_TARGET, "attempting to connect to {uri:?}");

		// Create a reconnecting RPC client with exponential backoff
		let reconnecting_rpc =
			ReconnectingRpcClient::builder()
				.retry_policy(
					ExponentialBackoff::from_millis(500)
						.max_delay(Duration::from_secs(30))
						.take(10), // Allow up to 10 retry attempts before giving up
				)
				.build(uri.to_string())
				.await
				.map_err(|e| subxt::Error::Other(format!("Failed to connect: {e:?}")))?;

		let backend = LegacyBackend::builder().build(reconnecting_rpc.clone());

		let chain_api = ChainClient::from_backend(Arc::new(backend)).await?;

		log::info!(target: LOG_TARGET, "Connected to {uri} with ChainHead backend");

		Ok(Self { chain_api, rpc: reconnecting_rpc })
	}

	/// Get a reference to the chain API.
	pub fn chain_api(&self) -> &ChainClient {
		&self.chain_api
	}

	/// Get the RPC used to connect to the chain.
	pub fn rpc(&self) -> &ReconnectingRpcClient {
		&self.rpc
	}
}
