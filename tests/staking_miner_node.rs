//! Requires a `staking-miner-playground binary ` in the path to run integration tests against.
#![cfg(feature = "staking-miner-playground-tests")]

pub mod common;

use assert_cmd::cargo::cargo_bin;
use codec::Decode;
use common::{
	init_logger, run_staking_miner_playground, spawn_cli_output_threads, KillChildOnDrop,
};
use regex::Regex;
use scale_info::TypeInfo;
use sp_keyring::AccountKeyring;
use staking_miner::{prelude::SubxtClient, signer::PairSigner};
use std::{
	process,
	time::{Duration, Instant},
};
use subxt::dynamic::Value;

#[tokio::test]
async fn constants_updated_on_the_fly() {
	init_logger();
	let (_drop, ws_url) = run_staking_miner_playground();

	let _miner = KillChildOnDrop(
		process::Command::new(cargo_bin(env!("CARGO_PKG_NAME")))
			.stdout(process::Stdio::piped())
			.stderr(process::Stdio::piped())
			.env("RUST_LOG", "runtime=debug,staking-miner=debug")
			.args(&["--uri", &ws_url, "monitor", "--seed-or-path", "//Alice", "seq-phragmen"])
			.spawn()
			.unwrap(),
	);

	let api = SubxtClient::from_url(&ws_url).await.unwrap();
	let signer = PairSigner::new(AccountKeyring::Alice.pair());

	let length: u32 = 1024;
	let weight: u64 = 2048;

	submit_tx("ConfigBlock", "set_block_weight", &api, vec![Value::u128(weight as u128)], &signer)
		.await;
	submit_tx("ConfigBlock", "set_block_length", &api, vec![Value::u128(length as u128)], &signer)
		.await;

	tokio::time::sleep(std::time::Duration::from_secs(12)).await;

	assert_eq!(weight, read_storage::<u64>("ConfigBlock", "BlockWeight", &api, vec![]).await);
	assert_eq!(length, read_storage::<u32>("ConfigBlock", "BlockLength", &api, vec![]).await);
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

async fn submit_tx(
	pallet: &str,
	name: &str,
	api: &SubxtClient,
	params: Vec<Value>,
	signer: &PairSigner,
) {
	let tx = subxt::dynamic::tx(pallet, name, params);
	_ = api.tx().sign_and_submit_then_watch_default(&tx, signer).await.unwrap();
}

async fn read_storage<T: Decode + TypeInfo + 'static>(
	pallet: &str,
	name: &str,
	api: &SubxtClient,
	params: Vec<Value>,
) -> T {
	let addr = subxt::dynamic::storage(pallet, name, params);
	let val = api.storage().at(None).await.unwrap().fetch(&addr).await.unwrap().unwrap();
	Decode::decode(&mut val.encoded()).unwrap()
}
