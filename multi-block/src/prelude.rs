use crate::static_types;
use frame_support::BoundedVec;
pub use subxt::{ext::sp_core, OnlineClient, PolkadotConfig};

pub type Config = subxt::PolkadotConfig;
pub type Hash = sp_core::H256;

pub type RpcClient = subxt::backend::legacy::LegacyRpcMethods<subxt::PolkadotConfig>;
pub type ChainClient = subxt::OnlineClient<subxt::PolkadotConfig>;

pub const DEFAULT_URI: &str = "ws://127.0.0.1:9944";
pub const LOG_TARGET: &str = "polkadot-staking-miner-mb";

pub type AccountId = sp_runtime::AccountId32;

pub type TargetSnapshotPage = BoundedVec<AccountId, static_types::TargetSnapshotPerBlock>;
pub type VoterSnapshotPage = BoundedVec<AccountId, static_types::TargetSnapshotPerBlock>;

pub type Header =
	subxt::config::substrate::SubstrateHeader<u32, subxt::config::substrate::BlakeTwo256>;

pub type Pair = sp_core::sr25519::Pair;

pub type Storage = subxt::storage::Storage<PolkadotConfig, OnlineClient<PolkadotConfig>>;

#[subxt::subxt(
	runtime_metadata_path = "metadata.scale",
	derive_for_all_types = "Clone, Debug, Eq, PartialEq",
	//substitute_type(
	//	path = "pallet_election_provider_multi_block::types::Phase<Bn>",
	//	with = "::subxt::utils::Static<::pallet_election_provider_multi_block::types::Phase<Bn>>"
	//)
)]
pub mod runtime {}

pub static SHARED_CLIENT: once_cell::sync::OnceCell<crate::client::Client> =
	once_cell::sync::OnceCell::new();
