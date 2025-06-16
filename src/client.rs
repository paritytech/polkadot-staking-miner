use crate::prelude::{ChainClient, LOG_TARGET, RpcClient};
use std::time::Duration;
use subxt::backend::rpc::{
	RpcClient as RawRpcClient,
	reconnecting_rpc_client::{ExponentialBackoff, RpcClient as ReconnectingRpcClient},
};

/// Wraps the subxt interfaces to make it easy to use for the staking-miner.
#[derive(Clone, Debug)]
pub struct Client {
	/// Access to typed rpc calls from subxt.
	rpc: RpcClient,
	/// Access to chain APIs such as storage, events etc.
	chain_api: ChainClient,
}

impl Client {
	pub async fn new(uri: &str) -> Result<Self, subxt::Error> {
		log::debug!(target: LOG_TARGET, "attempting to connect to {:?}", uri);

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
				.map_err(|e| subxt::Error::Other(format!("Failed to connect: {:?}", e)))?;

		let rpc = RawRpcClient::new(reconnecting_rpc);
		let chain_api = ChainClient::from_rpc_client(rpc.clone()).await?;

		log::info!(target: LOG_TARGET, "Connected to {} with reconnecting RPC client", uri);

		Ok(Self { rpc: RpcClient::new(rpc), chain_api })
	}

	/// Get a reference to the RPC interface exposed by subxt.
	pub fn rpc(&self) -> &RpcClient {
		&self.rpc
	}

	/// Get a reference to the chain API.
	pub fn chain_api(&self) -> &ChainClient {
		&self.chain_api
	}
}
