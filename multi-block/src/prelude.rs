use crate::static_types;

use frame_election_provider_support::SequentialPhragmen;
use frame_support::BoundedVec;
use pallet_election_provider_multi_block::unsigned::miner;

pub use subxt::{ext::sp_core, OnlineClient, PolkadotConfig};

pub type Config = subxt::PolkadotConfig;
pub type Hash = sp_core::H256;

pub type RpcClient = subxt::backend::legacy::LegacyRpcMethods<subxt::PolkadotConfig>;
pub type ChainClient = subxt::OnlineClient<subxt::PolkadotConfig>;

pub const DEFAULT_URI: &str = "ws://127.0.0.1:9944";
pub const LOG_TARGET: &str = "polkadot-staking-miner-mb";

pub type AccountId = sp_runtime::AccountId32;

pub type Solver = SequentialPhragmen<AccountId, sp_runtime::PerU16, ()>;

pub type SolutionPageOf<T> =
	BoundedVec<<T as miner::Config>::Solution, <T as miner::Config>::Pages>;

pub type TargetSnapshotPageOf<T> =
	BoundedVec<AccountId, <T as miner::Config>::TargetSnapshotPerBlock>;
pub type VoterSnapshotPageOf<T> = BoundedVec<Voter, <T as miner::Config>::VoterSnapshotPerBlock>;

pub type Voter = frame_election_provider_support::Voter<AccountId, static_types::MaxVotesPerVoter>;

pub type TargetSnapshotPage = BoundedVec<AccountId, static_types::TargetSnapshotPerBlock>;
pub type VoterSnapshotPage = BoundedVec<Voter, static_types::VoterSnapshotPerBlock>;

//pub type AllVoterSnapshotOf<T> = BoundedVec<VoterSnapshotPageOf<T>, <T as miner::Config>::Pages>;
//pub type AllVoterSnapshot = BoundedVec<VoterSnapshotPage, static_types::Pages>;

/*
pub type VoterOf<D> =
	Voter<<D as ElectionDataProvider>::AccountId, <D as ElectionDataProvider>::MaxVotesPerVoter>;
pub(crate) type VoterOf<T> =
	frame_election_provider_support::VoterOf<<T as crate::Config>::DataProvider>;
pub(crate) type VoterPageOf<T> =
	BoundedVec<VoterOf<T>, <T as crate::Config>::VoterSnapshotPerBlock>;
*/

pub type Header =
	subxt::config::substrate::SubstrateHeader<u32, subxt::config::substrate::BlakeTwo256>;

pub type Pair = sp_core::sr25519::Pair;

pub type Storage = subxt::storage::Storage<PolkadotConfig, OnlineClient<PolkadotConfig>>;

// TODO: move under opts to expose to caller.
use sp_npos_elections::BalancingConfig;
frame_support::parameter_types! {
	pub static BalanceIterations: usize = 10;
	pub static Balancing: Option<BalancingConfig> = Some( BalancingConfig { iterations: BalanceIterations::get(), tolerance: 0 });
}

#[subxt::subxt(
	runtime_metadata_path = "metadata.scale",
	derive_for_all_types = "Clone, Debug, Eq, PartialEq"
)]
pub mod runtime {}

pub static SHARED_CLIENT: once_cell::sync::OnceCell<crate::client::Client> =
	once_cell::sync::OnceCell::new();
