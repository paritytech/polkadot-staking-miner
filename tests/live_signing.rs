#![cfg(feature = "integration-tests")]
//! Proves the miner can build and sign every transaction it submits, on each Asset Hub it runs
//! against. Guards the transaction-extension handling: a chain-specific extension the miner does
//! not supply a value for makes signing fail outright, which is invisible to the offline tests
//! because it depends on the live runtime's metadata.
//!
//! Needs public RPC access, so it is gated behind `integration-tests` and run from nightly.yml.

use polkadot_sdk::sp_npos_elections::ElectionScore;
use polkadot_staking_miner::{
	client::Client,
	dynamic::{multi_block::MultiBlockTransaction, utils::with_restrict_origins},
	prelude::ExtrinsicParamsBuilder,
	runtime::multi_block as runtime,
	signer::Signer,
};

/// Every Asset Hub the miner targets, each with an endpoint pool. `Client::new` walks the pool
/// until one connects, so a single provider being down does not fail the run.
const ASSET_HUBS: &[(&str, &str)] = &[
	(
		"asset-hub-polkadot",
		"wss://polkadot-asset-hub-rpc.polkadot.io,\
		 wss://asset-hub-polkadot.dotters.network,\
		 wss://asset-hub-polkadot-rpc.n.dwellir.com",
	),
	(
		"asset-hub-kusama",
		"wss://kusama-asset-hub-rpc.polkadot.io,\
		 wss://asset-hub-kusama.dotters.network,\
		 wss://asset-hub-kusama-rpc.n.dwellir.com",
	),
	(
		"asset-hub-paseo",
		"wss://asset-hub-paseo-rpc.n.dwellir.com,\
		 wss://sys.turboflakes.io/asset-hub-paseo,\
		 wss://asset-hub-paseo.dotters.network",
	),
	(
		"asset-hub-westend",
		"wss://westend-asset-hub-rpc.polkadot.io,\
		 wss://asset-hub-westend.dotters.network",
	),
];

#[tokio::test]
async fn signs_every_submitted_call_on_every_asset_hub() {
	let signer = Signer::new("//Alice").expect("dev seed is valid");

	for (chain, endpoints) in ASSET_HUBS {
		// GIVEN a live Asset Hub and the metadata it is currently running
		let client = Client::new(endpoints)
			.await
			.unwrap_or_else(|e| panic!("{chain}: no endpoint in the pool reachable: {e}"));
		let chain_api = client.chain_api().await;
		let at_block = chain_api.at_current_block().await.expect("current block");
		let metadata = at_block.metadata_ref();

		// The dynamic payload is the hot path (`register` / `submit_page`); the static ones also
		// make subxt validate the embedded metadata's call hash against the live runtime.
		let score = ElectionScore { minimal_stake: 1, sum_stake: 2, sum_stake_squared: 3 };
		let (_, register) = MultiBlockTransaction::register_score(score)
			.expect("score encodes")
			.into_parts();
		let bail = runtime::tx().multi_block_election_signed().bail();
		let clear = runtime::tx().multi_block_election_signed().clear_old_round_data(0, 32);
		let prune = runtime::tx().staking().prune_era_step(0);

		// WHEN each call is signed with the params the miner builds for it
		// THEN signing succeeds, whatever transaction extensions the chain declares
		macro_rules! assert_signs {
			($label:literal, $call:expr) => {{
				let params = with_restrict_origins(
					ExtrinsicParamsBuilder::default().nonce(0).mortal(8),
					metadata,
				)
				.build();
				let signed = at_block.tx().create_signed(&$call, &*signer, params).await;
				let bytes = signed
					.unwrap_or_else(|e| panic!("{chain}: signing `{}` failed: {e}", $label))
					.encoded()
					.len();
				println!("  {chain:20} {:22} signed, {bytes} bytes", $label);
			}};
		}

		assert_signs!("register", register);
		assert_signs!("bail", bail);
		assert_signs!("clear_old_round_data", clear);
		assert_signs!("prune_era_step", prune);
	}
}
