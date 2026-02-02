//! Server command implementation for REST API
use crate::{
	client::Client,
	commands::{
		predict::run_prediction,
		types::{
			PredictConfig, ServerConfig, SimulateResult, SimulateRunParameters, SnapshotConfig,
			SnapshotNominator, SnapshotResult,
		},
	},
	dynamic::election_data::fetch_snapshots,
	error::Error,
	prelude::{AccountId, LOG_TARGET},
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
use std::net::SocketAddr;
use tokio::net::TcpListener;

type Body = Full<Bytes>;

pub async fn server_cmd<T>(client: Client, config: ServerConfig) -> Result<(), Error>
where
	T: MinerConfig<AccountId = AccountId> + Send + Sync + 'static,
	T::Solution: Send,
	T::Pages: Send,
	T::TargetSnapshotPerBlock: Send,
	T::VoterSnapshotPerBlock: Send,
	T::MaxVotesPerVoter: Send,
{
	let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
	let listener = TcpListener::bind(&addr)
		.await
		.map_err(|e| Error::Other(format!("Failed to bind to port {}: {}", config.port, e)))?;

	log::info!(target: LOG_TARGET, "REST API server listening on http://{addr}");

	loop {
		let (stream, _) = match listener.accept().await {
			Ok(conn) => conn,
			Err(e) => {
				log::error!(target: LOG_TARGET, "Failed to accept connection: {e}");
				continue;
			},
		};

		let client_clone = client.clone();
		let io = TokioIo::new(stream);
		let builder = Builder::new(TokioExecutor::new());
		let conn = builder
			.serve_connection_with_upgrades(
				io,
				service_fn(move |req| handle_request::<T>(client_clone.clone(), req)),
			)
			.into_owned();

		tokio::spawn(async move {
			if let Err(e) = conn.await {
				log::error!(target: LOG_TARGET, "Error serving connection: {e}");
			}
		});
	}
}

async fn handle_request<T>(
	client: Client,
	req: Request<hyper::body::Incoming>,
) -> Result<Response<Body>, hyper::Error>
where
	T: MinerConfig<AccountId = AccountId> + Send + Sync + 'static,
	T::Solution: Send,
	T::Pages: Send,
	T::TargetSnapshotPerBlock: Send,
	T::VoterSnapshotPerBlock: Send,
	T::MaxVotesPerVoter: Send,
{
	match (req.method(), req.uri().path()) {
		(&Method::POST, "/simulate") => {
			let query = req.uri().query().unwrap_or("");
			let query_block_number =
				parse_query_param(query, "block").and_then(|s| s.parse::<u32>().ok());

			let body_bytes = match req.collect().await {
				Ok(collected) => collected.to_bytes(),
				Err(e) => {
					return Ok(Response::builder()
						.status(400)
						.body(Body::from(format!("Failed to read body: {e}")))
						.unwrap());
				},
			};

			let mut predict_config: PredictConfig = match serde_json::from_slice(&body_bytes) {
				Ok(config) => config,
				Err(e) => {
					return Ok(Response::builder()
						.status(400)
						.body(Body::from(format!("Invalid JSON: {e}")))
						.unwrap());
				},
			};

			// Override block number if provided via query param
			if let Some(num) = query_block_number {
				predict_config.block_number = Some(num);
			}

			log::info!(target: LOG_TARGET, "Received /simulate request with config: {predict_config:?}");

			match run_prediction::<T>(client, predict_config.clone()).await {
				Ok((validators, nominators)) => {
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

					let result = serde_json::json!({
						"result": SimulateResult {
							run_parameters,
							active_validators: validators,
							nominators,
						}
					});

					let response_body = serde_json::to_vec(&result).unwrap();

					Ok(Response::builder()
						.status(200)
						.header(CONTENT_TYPE, "application/json")
						.body(Body::from(response_body))
						.unwrap())
				},
				Err(e) => {
					log::error!(target: LOG_TARGET, "Prediction failed: {e:?}");
					Ok(Response::builder()
						.status(500)
						.body(Body::from(format!("Prediction failed: {e:?}")))
						.unwrap())
				},
			}
		},

		(&Method::GET, "/snapshot") => {
			let query = req.uri().query().unwrap_or("");
			let query_block_number =
				parse_query_param(query, "block").and_then(|s| s.parse::<u32>().ok());

			match fetch_snapshot_data::<T>(client, query_block_number).await {
				Ok(snapshot_data) => {
					let result = serde_json::json!({
						"result": snapshot_data
					});
					let response_body = serde_json::to_vec(&result).unwrap();
					Ok(Response::builder()
						.status(200)
						.header(CONTENT_TYPE, "application/json")
						.body(Body::from(response_body))
						.unwrap())
				},
				Err(e) => {
					log::error!(target: LOG_TARGET, "Snapshot fetch failed: {e:?}");
					Ok(Response::builder()
						.status(500)
						.body(Body::from(format!("Snapshot fetch failed: {e:?}")))
						.unwrap())
				},
			}
		},

		_ => Ok(Response::builder().status(404).body(Body::from("Not Found")).unwrap()),
	}
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

	// Determine block number and storage
	let (block_number, storage) = if let Some(num) = block_number {
		let block_hash = get_block_hash(&client, num).await?;
		let storage =
			crate::utils::storage_at(Some(block_hash), &*client.chain_api().await).await?;
		(num, storage)
	} else {
		let storage = client.chain_api().await.storage().at_latest().await?;
		let block = client.chain_api().await.blocks().at_latest().await?;
		(block.number(), storage)
	};

	let current_round = storage
		.fetch_or_default(&runtime::storage().multi_block_election().round())
		.await?;

	// Use the refactored fetch_snapshots function from election_data.rs
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

	let data_source_str = match data_source {
		crate::commands::types::ElectionDataSource::Snapshot => "snapshot",
		crate::commands::types::ElectionDataSource::Staking => "staking",
	}
	.to_string();

	Ok(SnapshotResult {
		validators,
		nominators: snapshot_nominators,
		config: SnapshotConfig { block_number, round: current_round, data_source: data_source_str },
	})
}

/// Parse query parameter from query string
fn parse_query_param(query: &str, param_name: &str) -> Option<String> {
	query.split('&').find_map(|pair| {
		let mut parts = pair.split('=');
		match (parts.next(), parts.next()) {
			(Some(key), Some(value)) if key == param_name => Some(value.to_string()),
			_ => None,
		}
	})
}
