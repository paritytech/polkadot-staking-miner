//! Requires a `polkadot binary ` built with `--features fast-runtime` in the path to run integration tests against.
#![cfg(feature = "slow-tests")]

pub mod common;

use assert_cmd::cargo::cargo_bin;
use codec::{Decode, Encode};
use common::{init_logger, run_polkadot_node, run_staking_miner_playground, KillChildOnDrop};
use pallet_election_provider_multi_phase::{ElectionCompute, ReadySolution};
use regex::Regex;
use sp_storage::StorageChangeSet;
use staking_miner::{
	opt::Chain,
	prelude::{runtime, AccountId, Hash, SubxtClient},
};
use std::{
	io::{BufRead, BufReader},
	println, process,
	time::{Duration, Instant},
};
use subxt::{ext::sp_core::Bytes, rpc::rpc_params};
use tokio::time::timeout;

const MAX_DURATION_FOR_SUBMIT_SOLUTION: Duration = Duration::from_secs(60 * 15);

#[tokio::test]
async fn default_trimming_works() {
	let (_drop, ws_url) = run_staking_miner_playground();
	let miner = KillChildOnDrop(
		process::Command::new(cargo_bin(env!("CARGO_PKG_NAME")))
			.stdout(process::Stdio::piped())
			.stderr(process::Stdio::piped())
			.env("RUST_LOG", "runtime=debug,staking-miner=debug")
			.args(&["--uri", &ws_url, "monitor", "--seed-or-path", "//Alice", "seq-phragmen"])
			.spawn()
			.unwrap(),
	);

	assert!(has_trimming_output(miner));
}

#[tokio::test]
async fn constants_updated_on_the_fly() {
	let (_drop, ws_url) = run_staking_miner_playground();
	let miner = KillChildOnDrop(
		process::Command::new(cargo_bin(env!("CARGO_PKG_NAME")))
			.stdout(process::Stdio::piped())
			.stderr(process::Stdio::piped())
			.env("RUST_LOG", "runtime=debug,staking-miner=debug")
			.args(&["--uri", &ws_url, "monitor", "--seed-or-path", "//Alice", "seq-phragmen"])
			.spawn()
			.unwrap(),
	);

	let api = SubxtClient::from_url(&ws_url).await.unwrap();
	let _ = api
		.rpc()
		.request::<Bytes>(
			"state_call",
			rpc_params!["TestConfigApi_set_length", Bytes(1024_u32.encode())],
		)
		.await
		.unwrap();

	let _ = api
		.rpc()
		.request::<Bytes>(
			"state_call",
			rpc_params!["TestConfigApi_set_weight", Bytes(2048_u64.encode())],
		)
		.await
		.unwrap();

	assert!(has_trimming_output(miner));
}

#[tokio::test]
async fn submit_monitor_works_basic() {
	init_logger();
	test_submit_solution(Chain::Polkadot).await;
	test_submit_solution(Chain::Kusama).await;
	test_submit_solution(Chain::Westend).await;
}

async fn test_submit_solution(chain: Chain) {
	let (_drop, ws_url) = run_polkadot_node(chain);

	let _miner = KillChildOnDrop(
		process::Command::new(cargo_bin(env!("CARGO_PKG_NAME")))
			.stdout(process::Stdio::piped())
			.stderr(process::Stdio::piped())
			.args(&["--uri", &ws_url, "monitor", "--seed-or-path", "//Alice", "seq-phragmen"])
			.spawn()
			.unwrap(),
	);

	let api = SubxtClient::from_url(&ws_url).await.unwrap();
	let now = Instant::now();

	let key = Bytes(
		runtime::storage()
			.election_provider_multi_phase()
			.queued_solution()
			.to_root_bytes(),
	);

	let mut sub = api
		.rpc()
		.subscribe("state_subscribeStorage", rpc_params![vec![key]], "state_unsubscribeStorage")
		.await
		.unwrap();

	let mut success = false;

	while now.elapsed() < MAX_DURATION_FOR_SUBMIT_SOLUTION {
		let x: StorageChangeSet<Hash> =
			match timeout(MAX_DURATION_FOR_SUBMIT_SOLUTION, sub.next()).await {
				Err(e) => panic!("Timeout exceeded: {:?}", e),
				Ok(Some(Ok(storage))) => storage,
				Ok(None) => panic!("Subscription closed"),
				Ok(Some(Err(e))) => panic!("Failed to decode StorageChangeSet {:?}", e),
			};

		if let Some(data) = x.changes[0].clone().1 {
			let solution: ReadySolution<AccountId> = Decode::decode(&mut data.0.as_slice())
				.expect("Failed to decode storage as QueuedSolution");
			eprintln!("solution: {:?}", solution);
			assert!(solution.compute == ElectionCompute::Signed);
			success = true;
			break
		}
	}

	assert!(success);
}

// Helper that parses the CLI output to find logging outputs based on the following:
//
// i) DEBUG runtime::election-provider: 🗳 from 934 assignments, truncating to 1501 for weight, removing 0
// ii) DEBUG runtime::election-provider: 🗳 from 931 assignments, truncating to 755 for length, removing 176
//
// Thus, the only way to ensure that trimming actually works.
fn has_trimming_output(mut miner: KillChildOnDrop) -> bool {
	let trimming_re = Regex::new(
		r#"from (\d+) assignments, truncating to (\d+) for (?P<target>weight|length), removing (\d+)"#,
	)
	.unwrap();

	let mut got_truncate_len = false;
	let mut got_truncate_weight = false;

	let now = Instant::now();

	while !got_truncate_weight || !got_truncate_len {
		let stdout = miner.stdout.take().unwrap();
		for line in BufReader::new(stdout).lines() {
			let line = line.unwrap();
			println!("OK: {}", line);

			if let Some(caps) = trimming_re.captures(&line) {
				if caps.name("target").unwrap().as_str() == "weight" {
					got_truncate_weight = true;
				}

				if caps.name("target").unwrap().as_str() == "length" {
					got_truncate_len = true;
				}
			}

			if got_truncate_weight && got_truncate_len {
				return true
			}

			if now.elapsed() > Duration::from_secs(5 * 60) {
				break
			}
		}
	}

	false
}
