use crate::{
	error::{Error, TimeoutError},
	prelude::{ChainClient, LOG_TARGET},
	prometheus,
};
use std::{sync::Arc, time::Duration};
use subxt::backend::{
	legacy::LegacyBackend,
	rpc::reconnecting_rpc_client::{ExponentialBackoff, RpcClient as ReconnectingRpcClient},
};

/// Timeout for each connection attempt in seconds.
/// If a connection attempt doesn't complete within this time, we retry.
const CONNECTION_ATTEMPT_TIMEOUT_SECS: u64 = 30;

/// Maximum number of connection attempts before giving up.
const MAX_CONNECTION_ATTEMPTS: u32 = 3;

/// Delay between connection attempts in seconds.
const CONNECTION_RETRY_DELAY_SECS: u64 = 5;

/// Wraps the subxt interfaces to make it easy to use for the staking-miner.
#[derive(Clone, Debug)]
pub struct Client {
	/// Access to chain APIs such as storage, events etc.
	chain_api: ChainClient,
	/// Low-level RPC client
	rpc: ReconnectingRpcClient,
}

impl Client {
	pub async fn new(uri: &str) -> Result<Self, Error> {
		for attempt in 1..=MAX_CONNECTION_ATTEMPTS {
			log::debug!(
				target: LOG_TARGET,
				"attempting to connect to {uri:?} (attempt {attempt}/{MAX_CONNECTION_ATTEMPTS})"
			);

			match Self::try_connect(uri).await {
				Ok(client) => return Ok(client),
				Err(e) => {
					if attempt == MAX_CONNECTION_ATTEMPTS {
						log::error!(
							target: LOG_TARGET,
							"Failed to connect after {MAX_CONNECTION_ATTEMPTS} attempts: {e:?}"
						);
						return Err(e);
					}
					log::warn!(
						target: LOG_TARGET,
						"Connection attempt {attempt}/{MAX_CONNECTION_ATTEMPTS} failed: {e:?}, \
						 retrying in {CONNECTION_RETRY_DELAY_SECS}s..."
					);
					tokio::time::sleep(Duration::from_secs(CONNECTION_RETRY_DELAY_SECS)).await;
				},
			}
		}

		unreachable!("Loop should have returned or errored")
	}

	async fn try_connect(uri: &str) -> Result<Self, Error> {
		// Wrap the entire connection process with a timeout
		let connect_future = async {
			// Create a reconnecting RPC client with exponential backoff
			let reconnecting_rpc =
				ReconnectingRpcClient::builder()
					.retry_policy(
						ExponentialBackoff::from_millis(500)
							.max_delay(Duration::from_secs(10))
							.take(3), // Fewer internal retries since we have outer retry loop
					)
					.build(uri.to_string())
					.await
					.map_err(|e| Error::Other(format!("Failed to connect: {e:?}")))?;

			let backend = LegacyBackend::builder().build(reconnecting_rpc.clone());

			let chain_api = ChainClient::from_backend(Arc::new(backend)).await?;

			Ok::<Self, Error>(Self { chain_api, rpc: reconnecting_rpc })
		};

		match tokio::time::timeout(
			Duration::from_secs(CONNECTION_ATTEMPT_TIMEOUT_SECS),
			connect_future,
		)
		.await
		{
			Ok(result) => {
				if result.is_ok() {
					log::info!(target: LOG_TARGET, "Connected to {uri} with Legacy backend");
				}
				result
			},
			Err(_) => {
				prometheus::on_connection_timeout();
				log::warn!(
					target: LOG_TARGET,
					"Connection attempt timed out after {CONNECTION_ATTEMPT_TIMEOUT_SECS}s"
				);
				Err(TimeoutError::InitialConnection {
					timeout_secs: CONNECTION_ATTEMPT_TIMEOUT_SECS,
					attempt: 0, // Will be filled by caller context
					max_attempts: MAX_CONNECTION_ATTEMPTS,
				}
				.into())
			},
		}
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
