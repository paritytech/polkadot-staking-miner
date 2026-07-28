use crate::{
	macros::{
		impl_algorithm_parameter_type, impl_balancing_config_parameter_type,
		impl_u32_parameter_type,
	},
	prelude::{AccountId, Accuracy, Hash},
};
use polkadot_sdk::{
	frame_election_provider_support::{self, PerThing128, PhragMMS, SequentialPhragmen},
	frame_support, pallet_election_provider_multi_block as multi_block,
	sp_runtime::{PerU16, Percent, traits::ConstU32},
};

impl_u32_parameter_type!(pages, Pages);
impl_u32_parameter_type!(target_snapshot_per_block, TargetSnapshotPerBlock);
impl_u32_parameter_type!(voter_snapshot_per_block, VoterSnapshotPerBlock);
impl_u32_parameter_type!(max_winners_per_page, MaxWinnersPerPage);
impl_u32_parameter_type!(max_backers_per_winner, MaxBackersPerWinner);
impl_u32_parameter_type!(max_length, MaxLength);
impl_balancing_config_parameter_type!(balancing, BalancingIterations);
impl_algorithm_parameter_type!(algorithm, Algorithm);

pub struct DynamicSolver<AccountId, Accuracy, Balancing>(
	std::marker::PhantomData<(AccountId, Accuracy, Balancing)>,
);

impl<AccountId, Accuracy, Balancing> frame_election_provider_support::NposSolver
	for DynamicSolver<AccountId, Accuracy, Balancing>
where
	AccountId: frame_election_provider_support::IdentifierT,
	Accuracy: PerThing128,
	Balancing: frame_support::traits::Get<Option<polkadot_sdk::sp_npos_elections::BalancingConfig>>,
{
	type AccountId = AccountId;
	type Accuracy = Accuracy;
	type Error = polkadot_sdk::sp_npos_elections::Error;

	fn solve(
		winners: usize,
		targets: Vec<AccountId>,
		voters: Vec<(
			AccountId,
			polkadot_sdk::sp_npos_elections::VoteWeight,
			impl Clone + IntoIterator<Item = AccountId>,
		)>,
	) -> Result<polkadot_sdk::sp_npos_elections::ElectionResult<AccountId, Accuracy>, Self::Error>
	{
		match Algorithm::get() {
			crate::commands::types::ElectionAlgorithm::SeqPhragmen =>
				SequentialPhragmen::<AccountId, Accuracy, Balancing>::solve(
					winners, targets, voters,
				),
			crate::commands::types::ElectionAlgorithm::Phragmms =>
				PhragMMS::<AccountId, Accuracy, Balancing>::solve(winners, targets, voters),
		}
	}

	fn weight<T: frame_election_provider_support::WeightInfo>(
		voters: u32,
		targets: u32,
		vote_degree: u32,
	) -> polkadot_sdk::frame_election_provider_support::Weight {
		match Algorithm::get() {
			crate::commands::types::ElectionAlgorithm::SeqPhragmen =>
				SequentialPhragmen::<AccountId, Accuracy, Balancing>::weight::<T>(
					voters,
					targets,
					vote_degree,
				),
			crate::commands::types::ElectionAlgorithm::Phragmms =>
				PhragMMS::<AccountId, Accuracy, Balancing>::weight::<T>(
					voters,
					targets,
					vote_degree,
				),
		}
	}
}

pub mod node {
	use super::*;

	// TODO: validate config https://github.com/paritytech/polkadot-staking-miner/issues/994
	frame_election_provider_support::generate_solution_type!(
		#[compact]
		pub struct NposSolution16::<
			VoterIndex = u16,
			TargetIndex = u16,
			Accuracy = Percent,
			MaxVoters = VoterSnapshotPerBlock
		>(16)
	);

	#[derive(Debug)]
	pub struct MinerConfig;

	// TODO: validate config https://github.com/paritytech/polkadot-staking-miner/issues/994
	impl multi_block::unsigned::miner::MinerConfig for MinerConfig {
		type AccountId = AccountId;
		type Solution = NposSolution16;
		type Solver = DynamicSolver<AccountId, Accuracy, BalancingIterations>;
		type Pages = Pages;
		type MaxVotesPerVoter = ConstU32<16>;
		type MaxWinnersPerPage = MaxWinnersPerPage;
		type MaxBackersPerWinner = MaxBackersPerWinner;
		type MaxBackersPerWinnerFinal = ConstU32<{ u32::MAX }>;
		type VoterSnapshotPerBlock = VoterSnapshotPerBlock;
		type TargetSnapshotPerBlock = TargetSnapshotPerBlock;
		type MaxLength = MaxLength;
		type Hash = Hash;
	}
}

pub mod polkadot {
	use super::*;

	// TODO: validate config https://github.com/paritytech/polkadot-staking-miner/issues/994
	frame_election_provider_support::generate_solution_type!(
		#[compact]
		pub struct NposSolution16::<
			VoterIndex = u32,
			TargetIndex = u16,
			Accuracy = PerU16,
			MaxVoters = VoterSnapshotPerBlock
		>(16)
	);

	#[derive(Debug)]
	pub struct MinerConfig;

	// TODO: validate config https://github.com/paritytech/polkadot-staking-miner/issues/994
	impl multi_block::unsigned::miner::MinerConfig for MinerConfig {
		type AccountId = AccountId;
		type Solution = NposSolution16;
		type Solver = DynamicSolver<AccountId, Accuracy, BalancingIterations>;
		type Pages = Pages;
		type MaxVotesPerVoter = ConstU32<16>;
		type MaxWinnersPerPage = MaxWinnersPerPage;
		type MaxBackersPerWinner = MaxBackersPerWinner;
		type MaxBackersPerWinnerFinal = ConstU32<{ u32::MAX }>;
		type VoterSnapshotPerBlock = VoterSnapshotPerBlock;
		type TargetSnapshotPerBlock = TargetSnapshotPerBlock;
		type MaxLength = MaxLength;
		type Hash = Hash;
	}
}

pub mod kusama {
	use super::*;

	// TODO: validate config https://github.com/paritytech/polkadot-staking-miner/issues/994
	frame_election_provider_support::generate_solution_type!(
		#[compact]
		pub struct NposSolution24::<
			VoterIndex = u32,
			TargetIndex = u16,
			Accuracy = PerU16,
			MaxVoters = VoterSnapshotPerBlock
		>(24)
	);

	#[derive(Debug)]
	pub struct MinerConfig;

	// TODO: validate config https://github.com/paritytech/polkadot-staking-miner/issues/994
	impl multi_block::unsigned::miner::MinerConfig for MinerConfig {
		type AccountId = AccountId;
		type Solution = NposSolution24;
		type Solver = DynamicSolver<AccountId, Accuracy, BalancingIterations>;
		type Pages = Pages;
		type MaxVotesPerVoter = ConstU32<24>;
		type MaxWinnersPerPage = MaxWinnersPerPage;
		type MaxBackersPerWinner = MaxBackersPerWinner;
		type MaxBackersPerWinnerFinal = ConstU32<{ u32::MAX }>;
		type VoterSnapshotPerBlock = VoterSnapshotPerBlock;
		type TargetSnapshotPerBlock = TargetSnapshotPerBlock;
		type MaxLength = MaxLength;
		type Hash = Hash;
	}
}

pub mod westend {
	use super::*;

	// TODO: validate config https://github.com/paritytech/polkadot-staking-miner/issues/994
	frame_election_provider_support::generate_solution_type!(
		#[compact]
		pub struct NposSolution16::<
			VoterIndex = u32,
			TargetIndex = u16,
			Accuracy = PerU16,
			MaxVoters = VoterSnapshotPerBlock
		>(16)
	);

	#[derive(Debug)]
	pub struct MinerConfig;

	// TODO: validate config https://github.com/paritytech/polkadot-staking-miner/issues/994
	impl multi_block::unsigned::miner::MinerConfig for MinerConfig {
		type AccountId = AccountId;
		type Solution = NposSolution16;
		type Solver = DynamicSolver<AccountId, Accuracy, BalancingIterations>;
		type Pages = Pages;
		type MaxVotesPerVoter = ConstU32<16>;
		type MaxWinnersPerPage = MaxWinnersPerPage;
		type MaxBackersPerWinner = MaxBackersPerWinner;
		type MaxBackersPerWinnerFinal = ConstU32<{ u32::MAX }>;
		type VoterSnapshotPerBlock = VoterSnapshotPerBlock;
		type TargetSnapshotPerBlock = TargetSnapshotPerBlock;
		type MaxLength = MaxLength;
		type Hash = Hash;
	}
}

/// This is used to test against staking-async runtimes from the SDK.
pub mod staking_async {
	use super::*;

	// TODO: validate config https://github.com/paritytech/polkadot-staking-miner/issues/994
	frame_election_provider_support::generate_solution_type!(
		#[compact]
		pub struct NposSolution16::<
			VoterIndex = u32,
			TargetIndex = u16,
			Accuracy = PerU16,
			MaxVoters = VoterSnapshotPerBlock
		>(16)
	);

	#[derive(Debug)]
	pub struct MinerConfig;

	// TODO: validate config https://github.com/paritytech/polkadot-staking-miner/issues/994
	impl multi_block::unsigned::miner::MinerConfig for MinerConfig {
		type AccountId = AccountId;
		type Solution = NposSolution16;
		type Solver = DynamicSolver<AccountId, Accuracy, BalancingIterations>;
		type Pages = Pages;
		type MaxVotesPerVoter = ConstU32<16>;
		type MaxWinnersPerPage = MaxWinnersPerPage;
		type MaxBackersPerWinner = MaxBackersPerWinner;
		type MaxBackersPerWinnerFinal = ConstU32<{ u32::MAX }>;
		type VoterSnapshotPerBlock = VoterSnapshotPerBlock;
		type TargetSnapshotPerBlock = TargetSnapshotPerBlock;
		type MaxLength = MaxLength;
		type Hash = Hash;
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use polkadot_sdk::{
		frame_election_provider_support::{Error, NposSolution},
		sp_npos_elections::Assignment,
	};

	/// The solution's voter bound must follow the chain's `VoterSnapshotPerBlock`, the way the
	/// runtimes derive their own. A bound below the chain's page size rejects every full voter page
	/// with `TooManyVoters`, so it is pinned to the constant rather than to a literal.
	#[test]
	fn max_voters_follows_the_chain_voter_page_size() {
		let voters: Vec<AccountId> = (0..8u8).map(|i| AccountId::from([i; 32])).collect();
		let assignments: Vec<Assignment<AccountId, PerU16>> = voters
			.iter()
			.map(|who| Assignment {
				who: who.clone(),
				distribution: vec![(voters[0].clone(), PerU16::one())],
			})
			.collect();
		let encode = |voter_count: usize| {
			westend::NposSolution16::from_assignment(
				&assignments[..voter_count],
				|who| voters.iter().position(|v| v == who).map(|i| i as u32),
				|_| Some(0u16),
			)
		};

		// GIVEN a chain whose snapshot pages hold five voters
		VoterSnapshotPerBlock::set(5);

		// THEN a full page encodes and one voter more is rejected
		assert!(encode(5).is_ok());
		assert!(matches!(encode(6), Err(Error::TooManyVoters)));

		// WHEN the runtime raises the page size, the bound moves with it instead of going stale
		VoterSnapshotPerBlock::set(6);
		assert!(encode(6).is_ok());
	}
}
