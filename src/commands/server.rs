//! Server command implementation for REST API
use crate::{
	client::Client,
	commands::{
		predict::run_prediction,
		types::{PredictConfig, ServerConfig},
	},
	error::Error,
	prelude::{AccountId, LOG_TARGET},
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
			let body_bytes = match req.collect().await {
				Ok(collected) => collected.to_bytes(),
				Err(e) => {
					return Ok(Response::builder()
						.status(400)
						.body(Body::from(format!("Failed to read body: {e}")))
						.unwrap());
				},
			};

			let predict_config: PredictConfig = match serde_json::from_slice(&body_bytes) {
				Ok(config) => config,
				Err(e) => {
					return Ok(Response::builder()
						.status(400)
						.body(Body::from(format!("Invalid JSON: {e}")))
						.unwrap());
				},
			};

			log::info!(target: LOG_TARGET, "Received /simulate request with config: {predict_config:?}");

			match run_prediction::<T>(client, predict_config).await {
				Ok((validators, nominators)) => {
					let response_json = serde_json::json!({
						"validators": validators,
						"nominators": nominators,
					});
					let response_body = serde_json::to_vec(&response_json).unwrap();

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
		_ => Ok(Response::builder().status(404).body(Body::from("Not Found")).unwrap()),
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
