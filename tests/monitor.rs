//! Requires a `polkadot binary ` built with `--features fast-runtime` in the path to run integration tests against.
#![cfg(feature = "slow-tests")]

use assert_cmd::cargo::cargo_bin;
use staking_miner::{any_runtime, opt::Chain};
use std::{
	io::{BufRead, BufReader, Read},
	ops::{Deref, DerefMut},
	process::{self, Child},
	time::Duration,
};

const MAX_DURATION_FOR_SUBMIT_SOLUTION: Duration = Duration::from_secs(60 * 15);

use tracing_subscriber::EnvFilter;

pub fn init_logger() {
	let _ = tracing_subscriber::fmt()
		.with_env_filter(EnvFilter::from_default_env())
		.try_init();
}

#[tokio::test]
async fn submit_monitor_works() {
	init_logger();
	test_submit_solution(Chain::Polkadot).await;
	test_submit_solution(Chain::Kusama).await;
	test_submit_solution(Chain::Westend).await;
}

async fn test_submit_solution(chain: Chain) {
	let chain_str = format!("{}-dev", chain.to_string());

	let mut node_cmd = KillChildOnDrop(
		process::Command::new("polkadot")
			.stdout(process::Stdio::piped())
			.stderr(process::Stdio::piped())
			.args(&[
				"--chain",
				&chain_str,
				"--tmp",
				"--alice",
				"--execution",
				"Native",
				"--offchain-worker=Always",
			])
			.spawn()
			.unwrap(),
	);

	let stderr = node_cmd.stderr.take().unwrap();

	let (ws_url, _) = find_ws_url_from_output(stderr);

	let crate_name = env!("CARGO_PKG_NAME");
	let _miner = KillChildOnDrop(
		process::Command::new(cargo_bin(crate_name))
			.stdout(process::Stdio::piped())
			.stderr(process::Stdio::piped())
			.args(&["--uri", &ws_url, "--seed-or-path", "//Alice", "monitor", "seq-phragmen"])
			.spawn()
			.unwrap(),
	);

	any_runtime!(chain, {
		let api: RuntimeApi = subxt::ClientBuilder::new()
			.set_url(&ws_url)
			.build()
			.await
			.unwrap()
			.to_runtime_api();

		println!("started client");
		let now = std::time::Instant::now();

		let mut success = false;

		while now.elapsed() < MAX_DURATION_FOR_SUBMIT_SOLUTION {
			let indices = api
				.storage()
				.election_provider_multi_phase()
				.signed_submission_indices(None)
				.await
				.unwrap();

			if !indices.0.is_empty() {
				println!("submissions {:?}", indices.0);
				success = true;
				break
			}
		}

		assert!(success);
	});
}

/// Read the WS address from the output.
///
/// This is hack to get the actual binded sockaddr because
/// substrate assigns a random port if the specified port was already binded.
fn find_ws_url_from_output(read: impl Read + Send) -> (String, String) {
	let mut data = String::new();

	let ws_url = BufReader::new(read)
		.lines()
		.find_map(|line| {
			let line =
				line.expect("failed to obtain next line from stdout for WS address discovery");
			log::info!("{}", line);

			data.push_str(&line);

			// does the line contain our port (we expect this specific output from substrate).
			let sock_addr = match line.split_once("Running JSON-RPC WS server: addr=") {
				None => return None,
				Some((_, after)) => after.split_once(",").unwrap().0,
			};

			Some(format!("ws://{}", sock_addr))
		})
		.expect("We should get a WebSocket address");

	(ws_url, data)
}

pub struct KillChildOnDrop(pub Child);

impl Drop for KillChildOnDrop {
	fn drop(&mut self) {
		let _ = self.0.kill();
	}
}

impl Deref for KillChildOnDrop {
	type Target = Child;

	fn deref(&self) -> &Self::Target {
		&self.0
	}
}

impl DerefMut for KillChildOnDrop {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.0
	}
}
