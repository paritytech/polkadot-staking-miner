//! Tests for the server command

use clap::Parser;
use http_body_util::{BodyExt, Full};
use hyper::{Method, Request, body::Bytes};
use hyper_util::rt::TokioIo;
use polkadot_staking_miner::{
	commands::{
		server::{MAX_BODY_SIZE, MAX_CONCURRENT_PREDICTIONS, ServerHandler, serve},
		types::{
			ElectionAlgorithm, ElectionDataSource, NominatorsPrediction, OverridesConfig,
			PredictConfig, PredictionMetadata, ServerConfig, SnapshotConfig, SnapshotResult,
			ValidatorsPrediction,
		},
	},
	error::Error,
};

struct HttpResponse {
	status: u16,
	body: serde_json::Value,
}

/// Send a single HTTP/1.1 request to `addr` and return status + parsed JSON body.
async fn send(
	addr: &str,
	method: Method,
	path: &str,
	body: Option<Vec<u8>>,
	content_type: Option<&str>,
) -> HttpResponse {
	let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
	let io = TokioIo::new(stream);
	let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await.unwrap();
	tokio::spawn(async move { conn.await.unwrap() });

	let body_bytes: Vec<u8> = body.unwrap_or_default();
	let mut req = Request::builder().method(method).uri(path);
	if let Some(ct) = content_type {
		req = req.header("Content-Type", ct);
	}
	req = req.header("Host", addr);
	let request = req.body(Full::new(Bytes::from(body_bytes))).unwrap();

	let res = sender.send_request(request).await.unwrap();
	let status = res.status().as_u16();
	let raw = res.collect().await.unwrap().to_bytes();
	let body = serde_json::from_slice(&raw).unwrap_or(serde_json::Value::Null);
	HttpResponse { status, body }
}

async fn get(port: u16, path: &str) -> HttpResponse {
	send(&format!("127.0.0.1:{port}"), Method::GET, path, None, None).await
}

async fn post_json(port: u16, path: &str, json: serde_json::Value) -> HttpResponse {
	let body = serde_json::to_vec(&json).unwrap();
	send(&format!("127.0.0.1:{port}"), Method::POST, path, Some(body), Some("application/json"))
		.await
}

async fn post_raw(port: u16, path: &str, body: Vec<u8>, content_type: &str) -> HttpResponse {
	send(&format!("127.0.0.1:{port}"), Method::POST, path, Some(body), Some(content_type)).await
}

#[test]
fn server_help_works() {
	use assert_cmd::Command;
	let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!(env!("CARGO_PKG_NAME")));
	cmd.args(["server", "--help"]);
	cmd.assert().success();
}

/// Test ServerConfig parsing
#[test]
fn server_config_parsing() {
	// Test default port
	let config = ServerConfig::try_parse_from(["server"]).unwrap();
	assert_eq!(config.port, 8080);
	assert_eq!(config.listen, "127.0.0.1");

	// Test custom port
	let config = ServerConfig::try_parse_from(["server", "--port", "9090"]).unwrap();
	assert_eq!(config.port, 9090);

	let config = ServerConfig::try_parse_from(["server", "--listen", "0.0.0.0"]).unwrap();
	assert_eq!(config.listen, "0.0.0.0");
}

/// Test OverridesConfig parsing (Path vs Data)
#[test]
fn test_overrides_config_parsing() {
	// Test path parsing (CLI style)
	let config =
		PredictConfig::try_parse_from(["predict", "--overrides", "my_overrides.json"]).unwrap();
	match config.overrides {
		Some(OverridesConfig::Path(p)) => assert_eq!(p, "my_overrides.json"),
		_ => panic!("Expected Path override"),
	}

	// Test JSON parsing (API style)
	let json_payload = r#"{
		"do_reduce": true,
		"overrides": {
			"candidates_include": ["acc1"],
			"voters_include": [["acc2", 1000, ["acc1"]]]
		}
	}"#;
	let config: PredictConfig = serde_json::from_str(json_payload).unwrap();
	match config.overrides {
		Some(OverridesConfig::Data(data)) => {
			assert_eq!(data.candidates_include, vec!["acc1".to_string()]);
			assert_eq!(data.voters_include.len(), 1);
		},
		_ => panic!("Expected Data override"),
	}
}

/// Test SimulateRunParameters serialization
#[test]
fn test_simulate_params_serialization() {
	use polkadot_staking_miner::commands::types::SimulateRunParameters;

	let params = SimulateRunParameters {
		block_number: 100,
		desired_validators: 50,
		balancing_iterations: 10,
		do_reduce: true,
		algorithm: ElectionAlgorithm::SeqPhragmen,
		overrides: Some(OverridesConfig::Path("test.json".to_string())),
	};

	let serialized = serde_json::to_string(&params).unwrap();
	let deserialized: SimulateRunParameters = serde_json::from_str(&serialized).unwrap();

	assert_eq!(deserialized.block_number, 100);
	assert_eq!(deserialized.overrides, Some(OverridesConfig::Path("test.json".to_string())));
}

#[derive(Clone)]
struct MockHandler {
	slow_predict: Option<std::time::Duration>,
}

impl ServerHandler for MockHandler {
	async fn predict(
		&self,
		config: PredictConfig,
	) -> Result<(ValidatorsPrediction, NominatorsPrediction), Error> {
		if let Some(d) = self.slow_predict {
			tokio::time::sleep(d).await;
		}
		let block_number = config.block_number.unwrap_or(42);
		Ok((
			ValidatorsPrediction {
				metadata: PredictionMetadata {
					timestamp: "2024-01-01T00:00:00Z".to_string(),
					desired_validators: config.desired_validators.unwrap_or(10),
					round: 1,
					block_number,
					solution_score: None,
					data_source: "mock".to_string(),
				},
				results: vec![],
			},
			NominatorsPrediction { nominators: vec![] },
		))
	}

	async fn snapshot(&self, block_number: Option<u32>) -> Result<SnapshotResult, Error> {
		Ok(SnapshotResult {
			validators: vec!["Alice".to_string()],
			nominators: vec![],
			config: SnapshotConfig {
				block_number: block_number.unwrap_or(42),
				round: 1,
				data_source: ElectionDataSource::Snapshot,
			},
		})
	}
}

async fn start_mock_server(handler: MockHandler) -> u16 {
	let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
	let port = listener.local_addr().unwrap().port();
	drop(listener);

	let config = ServerConfig { port, listen: "127.0.0.1".to_string() };
	tokio::spawn(async move { serve(handler, config).await.unwrap() });
	tokio::time::sleep(std::time::Duration::from_millis(50)).await;
	port
}

#[tokio::test]
async fn test_simulate_returns_200_with_correct_json() {
	let port = start_mock_server(MockHandler { slow_predict: None }).await;

	let res = post_json(port, "/simulate", serde_json::json!({ "desired_validators": 5 })).await;
	assert_eq!(res.status, 200);
	assert_eq!(res.body["result"]["run_parameters"]["desired_validators"], 5);
	assert_eq!(res.body["result"]["run_parameters"]["block_number"], 42);
}

#[tokio::test]
async fn test_simulate_query_param_overrides_block_number() {
	let port = start_mock_server(MockHandler { slow_predict: None }).await;

	let res = post_json(port, "/simulate?block=999", serde_json::json!({})).await;
	assert_eq!(res.status, 200);
	assert_eq!(res.body["result"]["run_parameters"]["block_number"], 999);
}

#[tokio::test]
async fn test_simulate_rejects_invalid_json_with_400() {
	let port = start_mock_server(MockHandler { slow_predict: None }).await;

	let res = post_raw(port, "/simulate", b"not valid json {{{".to_vec(), "application/json").await;
	assert_eq!(res.status, 400);
	assert!(res.body["error"].as_str().unwrap().contains("Invalid JSON"));
}

#[tokio::test]
async fn test_simulate_rejects_oversized_body_with_413() {
	let port = start_mock_server(MockHandler { slow_predict: None }).await;

	// MAX_BODY_SIZE + 1 byte
	let large_body = vec![b'a'; (MAX_BODY_SIZE + 1) as usize];
	let res = post_raw(port, "/simulate", large_body, "application/json").await;
	assert_eq!(res.status, 413);
	assert!(res.body["error"].as_str().unwrap().contains("too large"));
}

#[tokio::test]
async fn test_snapshot_returns_200_with_correct_json() {
	let port = start_mock_server(MockHandler { slow_predict: None }).await;

	let res = get(port, "/snapshot").await;
	assert_eq!(res.status, 200);
	assert_eq!(res.body["result"]["config"]["block_number"], 42);
	assert_eq!(res.body["result"]["validators"][0], "Alice");
}

#[tokio::test]
async fn test_snapshot_query_param_sets_block_number() {
	let port = start_mock_server(MockHandler { slow_predict: None }).await;

	let res = get(port, "/snapshot?block=777").await;
	assert_eq!(res.status, 200);
	assert_eq!(res.body["result"]["config"]["block_number"], 777);
}

#[tokio::test]
async fn test_unknown_route_returns_404() {
	let port = start_mock_server(MockHandler { slow_predict: None }).await;

	let res = get(port, "/not-a-real-endpoint").await;
	assert_eq!(res.status, 404);
	assert_eq!(res.body["error"], "Not Found");
}

#[tokio::test]
async fn test_wrong_method_returns_404() {
	let port = start_mock_server(MockHandler { slow_predict: None }).await;

	// simulate only allows POST; GET should 404
	let res = get(port, "/simulate").await;
	assert_eq!(res.status, 404);
}

#[tokio::test]
async fn test_concurrent_prediction_returns_503() {
	let port = start_mock_server(MockHandler {
		slow_predict: Some(std::time::Duration::from_millis(500)),
	})
	.await;

	// Fire N (MAX_CONCURRENT_PREDICTIONS) requests in background
	let mut handles = Vec::new();
	for _ in 0..MAX_CONCURRENT_PREDICTIONS {
		handles.push(tokio::spawn(async move {
			post_json(port, "/simulate", serde_json::json!({})).await.status
		}));
	}

	// Small delay to ensure the requests have acquired the semaphore
	tokio::time::sleep(std::time::Duration::from_millis(100)).await;

	// Request N+1 while the others are still running
	let late_request = post_json(port, "/simulate", serde_json::json!({})).await;
	assert_eq!(late_request.status, 503);
	assert!(late_request.body["error"].as_str().unwrap().contains("already in progress"));

	// All original requests should eventually complete with 200
	for handle in handles {
		assert_eq!(handle.await.unwrap(), 200);
	}
}
