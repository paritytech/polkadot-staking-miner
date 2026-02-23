//! Server command implementation for REST API
use crate::{
	client::Client,
	commands::{
		predict::run_prediction,
		types::{
			NominatorsPrediction, PredictConfig, ServerConfig, SimulateResult,
			SimulateRunParameters, SnapshotConfig, SnapshotNominator, SnapshotResult,
			ValidatorsPrediction,
		},
	},
	dynamic::election_data::fetch_snapshots,
	error::Error,
	prelude::{AccountId, SERVER_LOG_TARGET as LOG_TARGET},
	runtime::multi_block::{self as runtime},
	static_types::multi_block::Pages,
	utils::{encode_account_id, get_block_hash, get_ss58_prefix},
};
use http_body_util::{BodyExt, Full};
use hyper::{Method, Request, Response, body::Bytes, header::CONTENT_TYPE, service::service_fn};
use hyper_util::{
	rt::{TokioExecutor, TokioIo},
	server::conn::auto::Builder,
};
use polkadot_sdk::pallet_election_provider_multi_block::unsigned::miner::MinerConfig;
use std::{
	net::{IpAddr, SocketAddr},
	sync::Arc,
};
use tokio::{
	net::TcpListener,
	sync::Semaphore,
	time::{Duration, timeout},
};

type Body = Full<Bytes>;

pub const MAX_BODY_SIZE: u64 = 1024 * 1024;
pub const MAX_CONCURRENT_CONNECTIONS: usize = 25;
pub const MAX_CONCURRENT_PREDICTIONS: usize = 1;
pub const SIMULATE_TIMEOUT: Duration = Duration::from_secs(600);

/// Abstracts the two data-fetching operations the HTTP server needs.
pub trait ServerHandler: Send + Sync + Clone + 'static {
	fn predict(
		&self,
		config: PredictConfig,
	) -> impl std::future::Future<Output = Result<(ValidatorsPrediction, NominatorsPrediction), Error>>
	+ Send;

	fn snapshot(
		&self,
		block_number: Option<u32>,
	) -> impl std::future::Future<Output = Result<SnapshotResult, Error>> + Send;
}

struct ClientHandler<T> {
	client: Client,
	_phantom: std::marker::PhantomData<T>,
}

impl<T> Clone for ClientHandler<T> {
	fn clone(&self) -> Self {
		Self { client: self.client.clone(), _phantom: std::marker::PhantomData }
	}
}

impl<T> ServerHandler for ClientHandler<T>
where
	T: MinerConfig<AccountId = AccountId> + Send + Sync + 'static,
	T::Solution: Send,
	T::Pages: Send,
	T::TargetSnapshotPerBlock: Send,
	T::VoterSnapshotPerBlock: Send,
	T::MaxVotesPerVoter: Send,
{
	async fn predict(
		&self,
		config: PredictConfig,
	) -> Result<(ValidatorsPrediction, NominatorsPrediction), Error> {
		run_prediction::<T>(self.client.clone(), config).await
	}

	async fn snapshot(&self, block_number: Option<u32>) -> Result<SnapshotResult, Error> {
		fetch_snapshot_data::<T>(self.client.clone(), block_number).await
	}
}

pub async fn server_cmd<T>(client: Client, config: ServerConfig) -> Result<(), Error>
where
	T: MinerConfig<AccountId = AccountId> + Send + Sync + 'static,
	T::Solution: Send,
	T::Pages: Send,
	T::TargetSnapshotPerBlock: Send,
	T::VoterSnapshotPerBlock: Send,
	T::MaxVotesPerVoter: Send,
{
	let handler = ClientHandler::<T> { client, _phantom: std::marker::PhantomData };
	serve(handler, config).await
}

pub async fn serve<H: ServerHandler>(handler: H, config: ServerConfig) -> Result<(), Error> {
	let listen_addr: IpAddr = config
		.listen
		.parse()
		.map_err(|e| Error::Other(format!("Invalid listen address '{}': {}", config.listen, e)))?;
	let addr = SocketAddr::from((listen_addr, config.port));
	let listener = TcpListener::bind(&addr)
		.await
		.map_err(|e| Error::Other(format!("Failed to bind to {addr}: {e}")))?;

	log::info!(target: LOG_TARGET, "REST API server listening on http://{addr}");

	let connection_semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS));
	let prediction_semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_PREDICTIONS));

	loop {
		let (stream, _) = match listener.accept().await {
			Ok(conn) => conn,
			Err(e) => {
				log::error!(target: LOG_TARGET, "Failed to accept connection: {e}");
				continue;
			},
		};

		let permit = match connection_semaphore.clone().acquire_owned().await {
			Ok(permit) => permit,
			Err(_) => break Ok(()), // Closed
		};

		let handler_clone = handler.clone();
		let prediction_semaphore_clone = prediction_semaphore.clone();
		let io = TokioIo::new(stream);
		let conn = Builder::new(TokioExecutor::new())
			.serve_connection_with_upgrades(
				io,
				service_fn(move |req| {
					handle_request(handler_clone.clone(), req, prediction_semaphore_clone.clone())
				}),
			)
			.into_owned();

		tokio::spawn(async move {
			if let Err(e) = conn.await {
				log::error!(target: LOG_TARGET, "Error serving connection: {e}");
			}
			drop(permit);
		});
	}
}

async fn handle_request<H: ServerHandler>(
	handler: H,
	req: Request<hyper::body::Incoming>,
	prediction_semaphore: Arc<Semaphore>,
) -> Result<Response<Body>, hyper::Error> {
	match (req.method(), req.uri().path()) {
		(&Method::POST, "/simulate") => {
			let query_block_number = req.uri().query().and_then(|query| {
				url::form_urlencoded::parse(query.as_bytes())
					.find(|(key, _)| key == "block")
					.and_then(|(_, value)| value.parse::<u32>().ok())
			});

			let body_bytes = match req.collect().await {
				Ok(collected) => {
					let bytes = collected.to_bytes();
					if bytes.len() as u64 > MAX_BODY_SIZE {
						return error_response(413, "Request body too large".to_string());
					}
					bytes
				},
				Err(_) => return error_response(400, "Failed to read body".to_string()),
			};

			let mut predict_config: PredictConfig = match serde_json::from_slice(&body_bytes) {
				Ok(config) => config,
				Err(_) => return error_response(400, "Invalid JSON".to_string()),
			};

			if let Some(num) = query_block_number {
				predict_config.block_number = Some(num);
			}

			log::info!(
				target: LOG_TARGET,
				"Received /simulate request: {predict_config:?}"
			);

			// Reject immediately if a prediction is already running
			let Ok(_permit) = prediction_semaphore.try_acquire() else {
				return error_response(503, "A prediction is already in progress".to_string());
			};

			let result = timeout(SIMULATE_TIMEOUT, handler.predict(predict_config.clone())).await;

			match result {
				Ok(Ok((validators, nominators))) => {
					// Build run_parameters with actual values used
					let metadata = &validators.metadata;
					let run_parameters = SimulateRunParameters {
						block_number: metadata.block_number,
						desired_validators: metadata.desired_validators,
						balancing_iterations: predict_config.balancing_iterations,
						do_reduce: predict_config.do_reduce,
						algorithm: predict_config.algorithm,
						overrides: predict_config.overrides,
					};

					json_response(&serde_json::json!({
						"result": SimulateResult {
							run_parameters,
							validators,
							nominators,
						}
					}))
				},
				Ok(Err(e)) => {
					log::error!(target: LOG_TARGET, "Prediction failed: {e:?}");
					error_response(500, "Internal Server Error: Prediction failed".to_string())
				},
				Err(_) => {
					log::error!(target: LOG_TARGET, "Prediction timed out after {SIMULATE_TIMEOUT:?}");
					error_response(504, "Gateway Timeout: Prediction took too long".to_string())
				},
			}
		},

		(&Method::GET, "/snapshot") => {
			let query_block_number = req.uri().query().and_then(|query| {
				url::form_urlencoded::parse(query.as_bytes())
					.find(|(key, _)| key == "block")
					.and_then(|(_, value)| value.parse::<u32>().ok())
			});

			match handler.snapshot(query_block_number).await {
				Ok(snapshot_data) => json_response(&serde_json::json!({ "result": snapshot_data })),
				Err(e) => {
					log::error!(target: LOG_TARGET, "Snapshot fetch failed: {e:?}");
					error_response(500, "Internal Server Error".to_string())
				},
			}
		},

		_ => error_response(404, "Not Found".to_string()),
	}
}

/// Create a JSON response safely
fn json_response(result: &serde_json::Value) -> Result<Response<Body>, hyper::Error> {
	match serde_json::to_vec(result) {
		Ok(body) => Ok(Response::builder()
			.status(200)
			.header(CONTENT_TYPE, "application/json")
			.body(Body::from(body))
			.unwrap_or_else(|e| {
				log::error!(target: LOG_TARGET, "Failed to build JSON response: {e}");
				Response::builder()
					.status(500)
					.header(CONTENT_TYPE, "application/json")
					.body(Body::from("{\"error\": \"Internal Server Error\"}"))
					.unwrap()
			})),
		Err(e) => {
			log::error!(target: LOG_TARGET, "Failed to serialize JSON: {e}");
			error_response(500, "Failed to serialize JSON".to_string())
		},
	}
}

/// Create an error response safely
fn error_response(status: u16, message: String) -> Result<Response<Body>, hyper::Error> {
	let body = serde_json::json!({ "error": message });
	let body_bytes = serde_json::to_vec(&body).unwrap_or_else(|_| {
		// Fallback for extreme cases where serialization fails
		b"{\"error\": \"Internal Server Error\"}".to_vec()
	});

	Ok(Response::builder()
		.status(status)
		.header(CONTENT_TYPE, "application/json")
		.body(Body::from(body_bytes))
		.unwrap_or_else(|e| {
			log::error!(target: LOG_TARGET, "Failed to build error response: {e}");
			Response::builder()
				.status(500)
				.header(CONTENT_TYPE, "application/json")
				.body(Body::from("{\"error\": \"Internal Server Error\"}"))
				.unwrap()
		}))
}

async fn fetch_snapshot_data<T>(
	client: Client,
	block_number: Option<u32>,
) -> Result<SnapshotResult, Error>
where
	T: MinerConfig<AccountId = AccountId> + Send + Sync + 'static,
	T::Solution: Send,
	T::Pages: Send,
	T::TargetSnapshotPerBlock: Send,
	T::VoterSnapshotPerBlock: Send,
	T::MaxVotesPerVoter: Send,
{
	let n_pages = Pages::get();

	// Get block number and storage
	let (block_number, storage) = if let Some(num) = block_number {
		let block_hash = get_block_hash(&client, num).await?;
		let storage =
			crate::utils::storage_at(Some(block_hash), &*client.chain_api().await).await?;
		(num, storage)
	} else {
		let block = client.chain_api().await.blocks().at_latest().await?;
		let storage = client.chain_api().await.storage().at(block.hash());
		(block.number(), storage)
	};

	let current_round = storage
		.fetch_or_default(&runtime::storage().multi_block_election().round())
		.await?;

	let (target_snapshot, voter_snapshot, data_source) =
		fetch_snapshots::<T>(n_pages, current_round, &storage, None).await?;

	let ss58_prefix = get_ss58_prefix(&client).await?;

	// Format validators
	let validators: Vec<String> = target_snapshot
		.into_iter()
		.map(|acc| encode_account_id(&acc, ss58_prefix))
		.collect();

	// Format nominators
	let snapshot_nominators: Vec<SnapshotNominator> = voter_snapshot
		.into_iter()
		.flat_map(|page| {
			page.into_iter().map(|(acc, stake, targets)| SnapshotNominator {
				account: encode_account_id(&acc, ss58_prefix),
				stake: stake.to_string(),
				targets: targets.into_iter().map(|t| encode_account_id(&t, ss58_prefix)).collect(),
			})
		})
		.collect();

	Ok(SnapshotResult {
		validators,
		nominators: snapshot_nominators,
		config: SnapshotConfig { block_number, round: current_round, data_source },
	})
}

#[cfg(test)]
mod tests {

	#[test]
	fn test_query_parsing() {
		// Mock query strings for testing url-based parsing logic
		let query = "block=123&other=abc";
		let parsed = url::form_urlencoded::parse(query.as_bytes())
			.find(|(key, _)| key == "block")
			.and_then(|(_, value)| value.parse::<u32>().ok());
		assert_eq!(parsed, Some(123));

		let empty_query = "";
		let parsed = url::form_urlencoded::parse(empty_query.as_bytes())
			.find(|(key, _)| key == "block")
			.and_then(|(_, value)| value.parse::<u32>().ok());
		assert_eq!(parsed, None);
	}
}
