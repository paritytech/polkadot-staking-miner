//! Requires a `staking-miner-playground binary ` in the path to run integration tests against.
#![cfg(feature = "staking-miner-playground-tests")]

pub mod common;

use assert_cmd::cargo::cargo_bin;
use codec::{Decode, Encode};
use common::{
	init_logger, run_staking_miner_playground, spawn_cli_output_threads, KillChildOnDrop,
};
use regex::Regex;
use staking_miner::prelude::SubxtClient;
use std::{
	process,
	time::{Duration, Instant},
};
use subxt::{ext::sp_core::Bytes, rpc::rpc_params};

#[tokio::test]
async fn constants_updated_on_the_fly() {
	init_logger();
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

	let length: u32 = 1024;
	let weight: u64 = 2048;

	let _ = api
		.rpc()
		.request::<Bytes>(
			"state_call",
			rpc_params!["TestConfigApi_set_length", Bytes(length.encode())],
		)
		.await
		.unwrap();

	let _ = api
		.rpc()
		.request::<Bytes>(
			"state_call",
			rpc_params!["TestConfigApi_set_weight", Bytes(weight.encode())],
		)
		.await
		.unwrap();

	assert!(has_trimming_output(miner).await);

	let read_length = api
		.rpc()
		.request::<Bytes>("state_call", rpc_params!["TestConfigApi_get_length", Bytes(vec![])])
		.await
		.unwrap();

	let read_length: u32 = Decode::decode(&mut read_length.as_ref()).unwrap();

	assert_eq!(length, read_length);

	let read_weight = api
		.rpc()
		.request::<Bytes>("state_call", rpc_params!["TestConfigApi_get_weight", Bytes(vec![])])
		.await
		.unwrap();

	let read_weight: u64 = Decode::decode(&mut read_weight.as_ref()).unwrap();

	assert_eq!(weight, read_weight);
}

#[tokio::test]
async fn default_trimming_works() {
	init_logger();
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

	assert!(has_trimming_output(miner).await);
}

// Helper that parses the CLI output to find logging outputs based on the following:
//
// i) DEBUG runtime::election-provider: 🗳 from 934 assignments, truncating to 1501 for weight, removing 0
// ii) DEBUG runtime::election-provider: 🗳 from 931 assignments, truncating to 755 for length, removing 176
//
// Thus, the only way to ensure that trimming actually works.
async fn has_trimming_output(mut miner: KillChildOnDrop) -> bool {
	let trimming_re = Regex::new(
		r#"from (\d+) assignments, truncating to (\d+) for (?P<target>weight|length), removing (\d+)"#,
	)
	.unwrap();

	let mut got_truncate_len = false;
	let mut got_truncate_weight = false;

	let now = Instant::now();
	let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();

	spawn_cli_output_threads(miner.stdout.take().unwrap(), miner.stderr.take().unwrap(), tx);

	while !got_truncate_weight || !got_truncate_len {
		let line = rx.recv().await.unwrap();
		println!("{}", line);

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

	false
}
