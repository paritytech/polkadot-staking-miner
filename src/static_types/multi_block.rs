use crate::{
	impl_atomic_u32_parameter_types,
	prelude::{AccountId, Accuracy, Hash},
};
use polkadot_sdk::{
	frame_election_provider_support, frame_support,
	pallet_election_provider_multi_block as multi_block,
	sp_runtime::{traits::ConstU32, PerU16},
};

impl_atomic_u32_parameter_types!(pages, Pages);
impl_atomic_u32_parameter_types!(target_snapshot_per_block, TargetSnapshotPerBlock);
impl_atomic_u32_parameter_types!(voter_snapshot_per_block, VoterSnapshotPerBlock);
impl_atomic_u32_parameter_types!(max_winners_per_page, MaxWinnersPerPage);
impl_atomic_u32_parameter_types!(max_backers_per_winner, MaxBackersPerWinner);
impl_atomic_u32_parameter_types!(max_length, MaxLength);

pub mod polkadot {
	use super::*;
	use frame_election_provider_support::SequentialPhragmen;

	frame_election_provider_support::generate_solution_type!(
		#[compact]
		pub struct NposSolution16::<
			VoterIndex = u32,
			TargetIndex = u16,
			Accuracy = PerU16,
			MaxVoters = ConstU32::<22500> // TODO: fetch
		>(16)
	);

	#[derive(Debug)]
	pub struct MinerConfig;

	impl multi_block::unsigned::miner::MinerConfig for MinerConfig {
		type AccountId = AccountId;
		type Solution = NposSolution16;
		// TODO: make it configurable via CLI/data from the node.
		type Solver = SequentialPhragmen<AccountId, Accuracy>;
		type Pages = Pages;
		// TODO: make it configurable via CLI/data from the node.
		type MaxVotesPerVoter = ConstU32<16>;
		type MaxWinnersPerPage = MaxWinnersPerPage;
		type MaxBackersPerWinner = MaxBackersPerWinner;
		// TODO: make it configurable via CLI/data from the node.
		type MaxBackersPerWinnerFinal = ConstU32<{ u32::MAX }>;
		type VoterSnapshotPerBlock = VoterSnapshotPerBlock;
		type TargetSnapshotPerBlock = TargetSnapshotPerBlock;
		type MaxLength = MaxLength;
		type Hash = Hash;
	}
}

pub mod kusama {
	use super::*;
	use frame_election_provider_support::SequentialPhragmen;

	frame_election_provider_support::generate_solution_type!(
		#[compact]
		pub struct NposSolution24::<
			VoterIndex = u32,
			TargetIndex = u16,
			Accuracy = PerU16,
			MaxVoters = ConstU32::<12500> // TODO: fetch/validate
		>(24)
	);

	#[derive(Debug)]
	pub struct MinerConfig;

	impl multi_block::unsigned::miner::MinerConfig for MinerConfig {
		type AccountId = AccountId;
		type Solution = NposSolution24;
		// TODO: make it configurable via CLI/data from the node.
		type Solver = SequentialPhragmen<AccountId, Accuracy>;
		type Pages = Pages;
		// TODO: make it configurable via CLI/data from the node.
		type MaxVotesPerVoter = ConstU32<24>;
		type MaxWinnersPerPage = MaxWinnersPerPage;
		type MaxBackersPerWinner = MaxBackersPerWinner;
		// TODO: make it configurable via CLI/data from the node.
		type MaxBackersPerWinnerFinal = ConstU32<{ u32::MAX }>;
		type VoterSnapshotPerBlock = VoterSnapshotPerBlock;
		type TargetSnapshotPerBlock = TargetSnapshotPerBlock;
		type MaxLength = MaxLength;
		type Hash = Hash;
	}
}

pub mod westend {
	use super::*;
	use frame_election_provider_support::SequentialPhragmen;

	frame_election_provider_support::generate_solution_type!(
		#[compact]
		pub struct NposSolution16::<
			VoterIndex = u32,
			TargetIndex = u16,
			Accuracy = PerU16,
			MaxVoters = ConstU32::<22500> // TODO: fetch/validate
		>(16)
	);

	#[derive(Debug)]
	pub struct MinerConfig;

	impl multi_block::unsigned::miner::MinerConfig for MinerConfig {
		type AccountId = AccountId;
		type Solution = NposSolution16;
		// TODO: make it configurable via CLI/data from the node.
		type Solver = SequentialPhragmen<AccountId, Accuracy>;
		type Pages = Pages;
		// TODO: make it configurable via CLI/data from the node.
		type MaxVotesPerVoter = ConstU32<16>;
		type MaxWinnersPerPage = MaxWinnersPerPage;
		type MaxBackersPerWinner = MaxBackersPerWinner;
		// TODO: make it configurable via CLI/data from the node.
		type MaxBackersPerWinnerFinal = ConstU32<{ u32::MAX }>;
		type VoterSnapshotPerBlock = VoterSnapshotPerBlock;
		type TargetSnapshotPerBlock = TargetSnapshotPerBlock;
		type MaxLength = MaxLength;
		type Hash = Hash;
	}
}
