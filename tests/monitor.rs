//! Requires a `polkadot binary ` built with `--features fast-runtime` in the path to run integration tests against.
#![cfg(feature = "slow-tests")]

pub mod common;

use assert_cmd::cargo::cargo_bin;
use codec::Decode;
use common::{init_logger, run_polkadot_node, KillChildOnDrop};
use pallet_election_provider_multi_phase::ReadySolution;
use sp_storage::StorageChangeSet;
use staking_miner::{
	opt::Chain,
	prelude::{runtime, AccountId, Hash, SubxtClient},
};
use std::{
	process,
	time::{Duration, Instant},
};
use subxt::{ext::sp_core::Bytes, rpc::rpc_params};

const MAX_DURATION_FOR_SUBMIT_SOLUTION: Duration = Duration::from_secs(60 * 15);

#[tokio::test]
async fn submit_monitor_works() {
	init_logger();
	test_submit_solution(Chain::Polkadot).await;
	test_submit_solution(Chain::Kusama).await;
	test_submit_solution(Chain::Westend).await;
}

async fn test_submit_solution(chain: Chain) {
	let (_drop, ws_url) = run_polkadot_node(chain);

	let crate_name = env!("CARGO_PKG_NAME");
	let _miner = KillChildOnDrop(
		process::Command::new(cargo_bin(crate_name))
			.stdout(process::Stdio::piped())
			.stderr(process::Stdio::piped())
			.args(&["--uri", &ws_url, "--seed-or-path", "//Alice", "monitor", "seq-phragmen"])
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
		let x: StorageChangeSet<Hash> = sub.next().await.unwrap().unwrap();

		if let Some(data) = x.changes[0].clone().1 {
			let solution: ReadySolution<AccountId> = Decode::decode(&mut data.0.as_slice())
				.expect("Failed to decode storage as QueuedSolution");
			println!("solution: {:?}", solution);
			success = true;
			break
		}
	}

	assert!(success);
}
