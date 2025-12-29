use crate::{
	error::{Error, TimeoutError},
	prelude::{ChainClient, Config, LOG_TARGET},
	prometheus,
};
use std::{sync::Arc, time::Duration};
use subxt::backend::{
	chain_head::{ChainHeadBackend, ChainHeadBackendBuilder},
	rpc::reconnecting_rpc_client::{ExponentialBackoff, RpcClient as ReconnectingRpcClient},
};
use tokio::sync::RwLock;

/// Timeout for each connection attempt in seconds.
/// If a connection attempt doesn't complete within this time, we retry.
const CONNECTION_ATTEMPT_TIMEOUT_SECS: u64 = 30;

/// Maximum number of connection attempts per endpoint before trying the next.
const MAX_CONNECTION_ATTEMPTS: u32 = 3;

/// Delay between connection attempts in seconds.
const CONNECTION_RETRY_DELAY_SECS: u64 = 5;

/// Wraps the subxt interfaces to make it easy to use for the staking-miner.
/// Supports multiple RPC endpoints with failover capability.
#[derive(Clone, Debug)]
pub struct Client {
	/// Access to chain APIs such as storage, events etc.
	/// Uses interior mutability to allow runtime failover.
	/// Each task (listenr, miner, clear_old_rounds, era_pruning) holds its own Client and when a
	/// failover happens (e.g. RPC endpoint dies) we need to swap out the dead ChainClient with a
	/// new one and all tasks must see the new connection. Instead of reconnecting in each task
	/// independently, we can leverage RwLock semantics to swap all tasks to use the new
	/// connection without explicit coordination. One writer updates the client and after that, all
	/// subsequent .read() returns the new client.
	chain_api: Arc<RwLock<ChainClient>>,
	/// List of RPC endpoints for failover support.
	endpoints: Arc<Vec<String>>,
}

impl Client {
	/// Create a new client from a comma-separated list of RPC endpoints.
	///
	/// The client will try each endpoint in sequence until one connects successfully.
	/// Multiple endpoints can be specified for failover:
	/// "wss://rpc1.example.com,wss://rpc2.example.com"
	pub async fn new(uris: &str) -> Result<Self, Error> {
		let endpoints: Vec<String> = uris
			.split(',')
			.map(|s| s.trim().to_string())
			.filter(|s| !s.is_empty())
			.collect();

		if endpoints.is_empty() {
			return Err(Error::Other("No RPC endpoints provided".into()));
		}

		if endpoints.len() > 1 {
			log::info!(target: LOG_TARGET, "RPC endpoint pool: {} endpoint(s)", endpoints.len());
		}

		let chain_api = Self::connect_with_failover(&endpoints).await?;

		Ok(Self { chain_api: Arc::new(RwLock::new(chain_api)), endpoints: Arc::new(endpoints) })
	}

	/// Try to connect to endpoints in sequence until one succeeds.
	/// Used for both initial connection and runtime failover.
	async fn connect_with_failover(endpoints: &[String]) -> Result<ChainClient, Error> {
		let mut last_error = None;
		let total = endpoints.len();

		for (idx, uri) in endpoints.iter().enumerate() {
			let endpoint_num = idx + 1;

			for attempt in 1..=MAX_CONNECTION_ATTEMPTS {
				log::debug!(
					target: LOG_TARGET,
					"attempting to connect to {uri:?} (endpoint {endpoint_num}/{total}, attempt {attempt}/{MAX_CONNECTION_ATTEMPTS})"
				);

				match Self::try_connect(uri).await {
					Ok(client) => {
						if total > 1 {
							log::info!(
								target: LOG_TARGET,
								"Connected to endpoint {endpoint_num}/{total}: {uri}"
							);
						}
						return Ok(client);
					},
					Err(e) => {
						last_error = Some(e);
						if attempt < MAX_CONNECTION_ATTEMPTS {
							log::warn!(
								target: LOG_TARGET,
								"Connection attempt {attempt}/{MAX_CONNECTION_ATTEMPTS} to {uri} failed, \
								 retrying in {CONNECTION_RETRY_DELAY_SECS}s..."
							);
							tokio::time::sleep(Duration::from_secs(CONNECTION_RETRY_DELAY_SECS))
								.await;
						}
					},
				}
			}

			if total > 1 {
				log::warn!(
					target: LOG_TARGET,
					"Failed to connect to endpoint {endpoint_num}/{total}: {uri}, trying next..."
				);
			}
		}

		log::error!(target: LOG_TARGET, "Failed to connect to any endpoint in the pool");
		Err(last_error.unwrap_or_else(|| Error::Other("No endpoints available".into())))
	}

	/// Try to connect to a single endpoint with timeout.
	async fn try_connect(uri: &str) -> Result<ChainClient, Error> {
		let connect_future = async {
			let reconnecting_rpc = ReconnectingRpcClient::builder()
				.retry_policy(
					ExponentialBackoff::from_millis(500).max_delay(Duration::from_secs(10)).take(3),
				)
				.build(uri.to_string())
				.await
				.map_err(|e| Error::Other(format!("Failed to connect: {e:?}")))?;

			let backend: ChainHeadBackend<Config> =
				ChainHeadBackendBuilder::default().build_with_background_driver(reconnecting_rpc);
			let chain_api = ChainClient::from_backend(Arc::new(backend)).await?;

			log::info!(target: LOG_TARGET, "Connected to {uri} with ChainHead backend");

			Ok::<ChainClient, Error>(chain_api)
		};

		match tokio::time::timeout(
			Duration::from_secs(CONNECTION_ATTEMPT_TIMEOUT_SECS),
			connect_future,
		)
		.await
		{
			Ok(result) => result,
			Err(_) => {
				prometheus::on_connection_timeout();
				log::warn!(
					target: LOG_TARGET,
					"Connection attempt timed out after {CONNECTION_ATTEMPT_TIMEOUT_SECS}s"
				);
				Err(TimeoutError::InitialConnection {
					timeout_secs: CONNECTION_ATTEMPT_TIMEOUT_SECS,
					attempt: 0,
					max_attempts: MAX_CONNECTION_ATTEMPTS,
				}
				.into())
			},
		}
	}

	/// Attempt to reconnect using the endpoint pool.
	/// This is called when subscription stalls are detected for runtime failover.
	pub async fn reconnect(&self) -> Result<(), Error> {
		log::info!(
			target: LOG_TARGET,
			"Attempting runtime failover across {} endpoint(s)...",
			self.endpoints.len()
		);

		let new_client = Self::connect_with_failover(&self.endpoints).await?;

		let mut guard = self.chain_api.write().await;
		*guard = new_client;

		log::info!(target: LOG_TARGET, "Runtime failover successful");
		Ok(())
	}

	/// Get access to the chain API.
	/// Returns a read guard that must be held while using the API.
	pub async fn chain_api(&self) -> tokio::sync::RwLockReadGuard<'_, ChainClient> {
		self.chain_api.read().await
	}
}
