use super::AccountId;
use polkadot_sdk::{
    frame_election_provider_support, frame_support::BoundedVec,
    pallet_election_provider_multi_block::unsigned::miner::MinerConfig,
};

pub type TargetSnapshotPageOf<T> =
    BoundedVec<AccountId, <T as MinerConfig>::TargetSnapshotPerBlock>;
pub type VoterSnapshotPageOf<T> = BoundedVec<Voter<T>, <T as MinerConfig>::VoterSnapshotPerBlock>;
pub type Voter<T> =
    frame_election_provider_support::Voter<AccountId, <T as MinerConfig>::MaxVotesPerVoter>;
pub type TargetSnapshotPage<T> =
    BoundedVec<<T as MinerConfig>::AccountId, <T as MinerConfig>::TargetSnapshotPerBlock>;
pub type VoterSnapshotPage<T> = BoundedVec<Voter<T>, <T as MinerConfig>::VoterSnapshotPerBlock>;

#[subxt::subxt(
    runtime_metadata_path = "artifacts/multi_block.scale",
    derive_for_all_types = "Clone, Debug, Eq, PartialEq",
    substitute_type(
        path = "sp_npos_elections::ElectionScore",
        with = "::subxt::utils::Static<polkadot_sdk::sp_npos_elections::ElectionScore>"
    ),
    substitute_type(
        path = "pallet_election_provider_multi_block::types::Phase<Bn>",
        with = "::subxt::utils::Static<polkadot_sdk::pallet_election_provider_multi_block::types::Phase<Bn>>"
    )
)]
pub mod runtime {}
