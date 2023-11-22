// Copyright 2021-2022 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

//! Types that we don't fetch from a particular runtime and just assume that they are constant all
//! of the place.
//!
//! It is actually easy to convert the rest as well, but it'll be a lot of noise in our codebase,
//! needing to sprinkle `any_runtime` in a few extra places.

use subxt::backend::rpc::RpcClient;

// re-exports.
pub use frame_election_provider_support::VoteWeight;
pub use pallet_election_provider_multi_phase::{Miner, MinerConfig};
pub use subxt::ext::sp_core;
/// The account id type.
pub type AccountId = sp_runtime::AccountId32;
/// The header type. We re-export it here, but we can easily get it from block as well.
pub type Header =
	subxt::config::substrate::SubstrateHeader<u32, subxt::config::substrate::BlakeTwo256>;
/// The header type. We re-export it here, but we can easily get it from block as well.
pub type Hash = sp_core::H256;
/// Balance type
pub type Balance = u128;
pub use subxt::ext::sp_runtime::traits::{Block as BlockT, Header as HeaderT};

/// Default URI to connect to.
///
/// This will never work on a remote node, so we might as well try a local node.
pub const DEFAULT_URI: &str = "ws://127.0.0.1:9944";
/// Default port to start the prometheus server on.
pub const DEFAULT_PROMETHEUS_PORT: u16 = 9999;
/// The logging target.
pub const LOG_TARGET: &str = "staking-miner";

/// The key pair type being used. We "strongly" assume sr25519 for simplicity.
pub type Pair = sp_core::sr25519::Pair;

/// The accuracy that we use for election computation.
pub type Accuracy = sp_runtime::Perbill;

/// Extrinsics params used on all chains.
pub use subxt::config::polkadot::PolkadotExtrinsicParamsBuilder as ExtrinsicParams;

pub type SubxtRpcClient = subxt::backend::legacy::LegacyRpcMethods<subxt::PolkadotConfig>;
/// Subxt client used by the staking miner on all chains.
pub type ChainClient = subxt::OnlineClient<subxt::PolkadotConfig>;

/// Config used by the staking-miner
pub type Config = subxt::PolkadotConfig;

/// Submission type used by the staking miner.
pub type SignedSubmission<S> =
	pallet_election_provider_multi_phase::SignedSubmission<AccountId, Balance, S>;

#[subxt::subxt(
	runtime_metadata_path = "artifacts/metadata.scale",
	derive_for_all_types = "Clone, Debug, Eq, PartialEq",
	derive_for_type(
		path = "pallet_election_provider_multi_phase::RoundSnapshot",
		derive = "Default"
	),
	substitute_type(
		path = "sp_npos_elections::ElectionScore",
		with = "::subxt::utils::Static<::sp_npos_elections::ElectionScore>"
	),
	substitute_type(
		path = "pallet_election_provider_multi_phase::Phase<Bn>",
		with = "::subxt::utils::Static<::pallet_election_provider_multi_phase::Phase<Bn>>"
	)
)]
pub mod runtime {}

pub static SHARED_CLIENT: once_cell::sync::OnceCell<ChainClient> = once_cell::sync::OnceCell::new();

#[derive(Clone)]
pub struct Client {
	/// Access to typed rpc calls from subxt.
	subxt_rpc: SubxtRpcClient,
	/// Access to chain APIs such as storage, events etc.
	chain_api: ChainClient,
	/// Raw RPC client.
	raw_rpc: RpcClient,
}

impl Client {
	pub async fn new(rpc: RpcClient) -> Self {
		let subxt_rpc = SubxtRpcClient::new(rpc.clone());
		let chain_api = ChainClient::from_rpc_client(rpc.clone()).await.unwrap();
		Self { chain_api, subxt_rpc, raw_rpc: rpc }
	}

	pub fn subxt_rpc(&self) -> &SubxtRpcClient {
		&self.subxt_rpc
	}

	pub fn chain_api(&self) -> &ChainClient {
		&self.chain_api
	}

	// This is exposed until a new version of subxt is released.
	pub async fn rpc_system_account_next_index<T>(
		&self,
		account_id: &T,
	) -> Result<u64, subxt::Error>
	where
		T: serde::Serialize,
	{
		self.raw_rpc
			.request("system_accountNextIndex", subxt::rpc_params![&account_id])
			.await
	}
}
