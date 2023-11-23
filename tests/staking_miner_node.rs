//! Requires a `staking-miner-playground binary ` in the path to run integration tests against.
#![cfg(feature = "staking-miner-playground-tests")]

pub mod common;

use assert_cmd::cargo::cargo_bin;
use codec::Decode;
use common::{init_logger, run_staking_miner_playground, KillChildOnDrop};
use scale_info::TypeInfo;
use staking_miner::{
	prelude::ChainClient,
	signer::{PairSigner, Signer},
};
use std::process;
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
			.args(["--uri", &ws_url, "monitor", "--seed-or-path", "//Alice", "seq-phragmen"])
			.spawn()
			.unwrap(),
	);

	let api = ChainClient::from_url(&ws_url).await.unwrap();
	let signer = Signer::new("//Alice").unwrap();

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

async fn submit_tx(
	pallet: &str,
	name: &str,
	api: &ChainClient,
	params: Vec<Value>,
	signer: &PairSigner,
) {
	let tx = subxt::dynamic::tx(pallet, name, params);
	api.tx().sign_and_submit_then_watch_default(&tx, signer).await.unwrap();
}

async fn read_storage<T: Decode + TypeInfo + 'static>(
	pallet: &str,
	name: &str,
	api: &ChainClient,
	params: Vec<Value>,
) -> T {
	let addr = subxt::dynamic::storage(pallet, name, params);
	let val = api
		.storage()
		.at_latest()
		.await
		.expect("Get storage should work")
		.fetch(&addr)
		.await
		.expect("Fetch storage should work")
		.expect("Storage should exist");
	Decode::decode(&mut val.encoded()).expect("Storage should be decodable")
}
