use crate::{
	error::{Error, TimeoutError},
	prelude::{ChainClient, Config, LOG_TARGET},
	prometheus,
};
use std::{
	sync::{
		Arc,
		atomic::{AtomicUsize, Ordering},
	},
	time::Duration,
};
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
	/// Each task (listener, miner, clear_old_rounds, era_pruning) holds its own Client and when a
	/// failover happens (e.g. RPC endpoint dies) we need to swap out the dead ChainClient with a
	/// new one and all tasks must see the new connection. Instead of reconnecting in each task
	/// independently, we can leverage RwLock semantics to swap all tasks to use the new
	/// connection without explicit coordination. One writer updates the client and after that, all
	/// subsequent .read() returns the new client.
	chain_api: Arc<RwLock<ChainClient>>,
	/// List of RPC endpoints for failover support.
	endpoints: Arc<Vec<String>>,
	/// Current endpoint index for round-robin selection.
	/// When reconnecting, we start from (current_index + 1) % len to avoid wasting time
	/// on known-bad endpoints.
	current_endpoint_index: Arc<AtomicUsize>,
	/// Generation counter for reconnect deduplication.
	/// Prevents racing reconnects when multiple tasks detect failures simultaneously.
	/// Each successful reconnect increments this counter.
	reconnect_generation: Arc<AtomicUsize>,
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

		let (chain_api, connected_index) = Self::connect_with_failover(&endpoints, 0).await?;

		Ok(Self {
			chain_api: Arc::new(RwLock::new(chain_api)),
			endpoints: Arc::new(endpoints),
			current_endpoint_index: Arc::new(AtomicUsize::new(connected_index)),
			reconnect_generation: Arc::new(AtomicUsize::new(0)),
		})
	}

	/// Try to connect to endpoints in round-robin sequence until one succeeds.
	/// Used for both initial connection and runtime failover.
	///
	/// # Arguments
	/// * `endpoints` - List of RPC endpoints to try
	/// * `start_index` - Index to start from (for round-robin selection)
	///
	/// # Returns
	/// * `Ok((ChainClient, usize))` - The connected client and the index of the successful endpoint
	/// * `Err(Error)` - If all endpoints fail
	async fn connect_with_failover(
		endpoints: &[String],
		start_index: usize,
	) -> Result<(ChainClient, usize), Error> {
		let mut last_error = None;
		let total = endpoints.len();
		// When pool has multiple endpoints, reduce retries to fail fast and try next endpoint
		let max_attempts = if total > 1 { 1 } else { MAX_CONNECTION_ATTEMPTS };

		for i in 0..total {
			let idx = (start_index + i) % total;
			let uri = &endpoints[idx];
			let endpoint_num = idx + 1;

			for attempt in 1..=max_attempts {
				log::debug!(
					target: LOG_TARGET,
					"attempting to connect to {uri:?} (endpoint {endpoint_num}/{total}, attempt {attempt}/{max_attempts})"
				);

				match Self::try_connect(uri).await {
					Ok(client) => {
						if total > 1 {
							log::info!(
								target: LOG_TARGET,
								"Connected to endpoint {endpoint_num}/{total}: {uri}"
							);
						}
						return Ok((client, idx));
					},
					Err(e) => {
						last_error = Some(e);
						if attempt < max_attempts {
							log::warn!(
								target: LOG_TARGET,
								"Connection attempt {attempt}/{max_attempts} to {uri} failed, \
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

	/// Attempt to reconnect using the endpoint pool with round-robin selection.
	/// This is called when subscription stalls are detected for runtime failover.
	/// Starts from (current_index + 1) % len to avoid retrying the currently-failed endpoint.
	///
	/// Uses a generation counter to prevent racing reconnects when multiple tasks detect
	/// failures simultaneously. If another task has already reconnected, this call returns
	/// Ok(()) without doing redundant work.
	pub async fn reconnect(&self) -> Result<(), Error> {
		// Capture generation before acquiring lock to detect racing reconnects
		let generation_before = self.reconnect_generation.load(Ordering::Relaxed);

		let current_idx = self.current_endpoint_index.load(Ordering::Relaxed);
		let start_idx = (current_idx + 1) % self.endpoints.len();

		log::info!(
			target: LOG_TARGET,
			"Attempting runtime failover across {} endpoint(s), starting from index {} (generation {})...",
			self.endpoints.len(),
			start_idx,
			generation_before
		);

		// Establish new connection before acquiring write lock
		let (new_client, connected_idx) =
			Self::connect_with_failover(&self.endpoints, start_idx).await?;

		// Acquire write lock and check if another task already reconnected
		let mut guard = self.chain_api.write().await;

		// Check if generation changed while we were connecting (another task already reconnected)
		let generation_now = self.reconnect_generation.load(Ordering::Relaxed);
		if generation_now != generation_before {
			log::info!(
				target: LOG_TARGET,
				"Reconnect skipped - another task already reconnected (generation {generation_before} -> {generation_now})"
			);
			// Connection already updated by another task, drop our new connection
			return Ok(());
		}

		// Update the client and generation
		*guard = new_client;
		self.current_endpoint_index.store(connected_idx, Ordering::Relaxed);
		self.reconnect_generation.fetch_add(1, Ordering::Relaxed);

		// Record the endpoint switch
		prometheus::on_endpoint_switch();

		log::info!(
			target: LOG_TARGET,
			"Runtime failover successful, now using endpoint {}/{} (generation {})",
			connected_idx + 1,
			self.endpoints.len(),
			generation_before + 1
		);
		Ok(())
	}

	/// Get access to the chain API.
	/// Returns a read guard that must be held while using the API.
	pub async fn chain_api(&self) -> tokio::sync::RwLockReadGuard<'_, ChainClient> {
		self.chain_api.read().await
	}
}
