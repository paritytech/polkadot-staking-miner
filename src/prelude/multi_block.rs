use super::AccountId;
use crate::static_types;
use polkadot_sdk::{
	frame_election_provider_support, frame_support::BoundedVec,
	pallet_election_provider_multi_block::unsigned::miner::MinerConfig,
};

pub type TargetSnapshotPageOf<T> =
	BoundedVec<AccountId, <T as MinerConfig>::TargetSnapshotPerBlock>;
pub type VoterSnapshotPageOf<T> = BoundedVec<Voter, <T as MinerConfig>::VoterSnapshotPerBlock>;
pub type Voter = frame_election_provider_support::Voter<AccountId, static_types::MaxVotesPerVoter>;
pub type TargetSnapshotPage = BoundedVec<AccountId, static_types::TargetSnapshotPerBlock>;
pub type VoterSnapshotPage = BoundedVec<Voter, static_types::VoterSnapshotPerBlock>;

#[subxt::subxt(
	runtime_metadata_path = "artifacts/multi_block.scale",
	derive_for_all_types = "Clone, Debug, Eq, PartialEq",
	substitute_type(
		path = "pallet_election_provider_multi_block::types::Phase<Bn>",
		with = "::subxt::utils::Static<polkadot_sdk::pallet_election_provider_multi_block::types::Phase<Bn>>"
	)
)]
pub mod runtime {}
