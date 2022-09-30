use crate::{epm_dynamic, helpers::mock_votes, prelude::*};
use frame_election_provider_support::traits::NposSolution;
use frame_support::{traits::ConstU32, weights::Weight};
use pallet_election_provider_multi_phase::{RawSolution, SolutionOrSnapshotSize};

macro_rules! impl_atomic_u32_parameter_types {
	($mod:ident, $name:ident) => {
		mod $mod {
			use std::sync::atomic::{AtomicU32, Ordering};

			static VAL: AtomicU32 = AtomicU32::new(0);

			pub struct $name;

			impl $name {
				pub fn get() -> u32 {
					VAL.load(Ordering::SeqCst)
				}
			}

			impl<I: From<u32>> frame_support::traits::Get<I> for $name {
				fn get() -> I {
					I::from(Self::get())
				}
			}

			impl $name {
				pub fn set(val: u32) {
					VAL.store(val, std::sync::atomic::Ordering::SeqCst);
				}
			}
		}

		pub use $mod::$name;
	};
}

mod max_weight {
	use frame_support::weights::Weight;
	use std::sync::atomic::{AtomicU64, Ordering};

	static VAL: AtomicU64 = AtomicU64::new(0);
	pub struct MaxWeight;

	impl MaxWeight {
		pub fn get() -> Weight {
			Weight::from_ref_time(VAL.load(Ordering::SeqCst))
		}
	}

	impl frame_support::traits::Get<Weight> for MaxWeight {
		fn get() -> Weight {
			Self::get()
		}
	}

	impl MaxWeight {
		pub fn set(weight: Weight) {
			VAL.store(weight.ref_time(), std::sync::atomic::Ordering::SeqCst);
		}
	}
}

impl_atomic_u32_parameter_types!(max_length, MaxLength);
impl_atomic_u32_parameter_types!(max_votes, MaxVotesPerVoter);
pub use max_weight::MaxWeight;

pub mod westend {
	use super::*;

	// SYNC
	frame_election_provider_support::generate_solution_type!(
		#[compact]
		pub struct NposSolution16::<
			VoterIndex = u32,
			TargetIndex = u16,
			Accuracy = sp_runtime::PerU16,
			MaxVoters = ConstU32::<22500>
		>(16)
	);

	#[derive(Debug)]
	pub struct MinerConfig;
	impl pallet_election_provider_multi_phase::unsigned::MinerConfig for MinerConfig {
		type AccountId = AccountId;
		type MaxLength = MaxLength;
		type MaxWeight = MaxWeight;
		type MaxVotesPerVoter = MaxVotesPerVoter;
		type Solution = NposSolution16;

		fn solution_weight(
			voters: u32,
			targets: u32,
			active_voters: u32,
			desired_targets: u32,
		) -> Weight {
			// Mock a RawSolution to get the correct weight without having to do the heavy work.
			let raw_solution = RawSolution {
				solution: NposSolution16 {
					votes1: mock_votes(
						active_voters,
						desired_targets.try_into().expect("Desired targets < u16::MAX"),
					),
					..Default::default()
				},
				..Default::default()
			};

			assert_eq!(raw_solution.solution.voter_count(), active_voters as usize);
			assert_eq!(raw_solution.solution.unique_targets().len(), desired_targets as usize);

			epm_dynamic::runtime_api_solution_weight(
				raw_solution,
				SolutionOrSnapshotSize { voters, targets },
			)
		}
	}
}

pub mod polkadot {
	use super::*;
	use frame_support::traits::ConstU32;
	use pallet_election_provider_multi_phase::{RawSolution, SolutionOrSnapshotSize};

	// SYNC
	frame_election_provider_support::generate_solution_type!(
		#[compact]
		pub struct NposSolution16::<
			VoterIndex = u32,
			TargetIndex = u16,
			Accuracy = sp_runtime::PerU16,
			MaxVoters = ConstU32::<22500>
		>(16)
	);

	#[derive(Debug)]
	pub struct MinerConfig;
	impl pallet_election_provider_multi_phase::unsigned::MinerConfig for MinerConfig {
		type AccountId = AccountId;
		type MaxLength = MaxLength;
		type MaxWeight = MaxWeight;
		type MaxVotesPerVoter = MaxVotesPerVoter;
		type Solution = NposSolution16;

		fn solution_weight(
			voters: u32,
			targets: u32,
			active_voters: u32,
			desired_targets: u32,
		) -> Weight {
			// Mock a RawSolution to get the correct weight without having to do the heavy work.
			let raw = RawSolution {
				solution: NposSolution16 {
					votes1: mock_votes(
						active_voters,
						desired_targets.try_into().expect("Desired targets < u16::MAX"),
					),
					..Default::default()
				},
				..Default::default()
			};

			assert_eq!(raw.solution.voter_count(), active_voters as usize);
			assert_eq!(raw.solution.unique_targets().len(), desired_targets as usize);

			epm_dynamic::runtime_api_solution_weight(
				raw,
				SolutionOrSnapshotSize { voters, targets },
			)
		}
	}
}

pub mod kusama {
	use super::*;

	// SYNC
	frame_election_provider_support::generate_solution_type!(
		#[compact]
		pub struct NposSolution24::<
			VoterIndex = u32,
			TargetIndex = u16,
			Accuracy = sp_runtime::PerU16,
			MaxVoters = ConstU32::<12500>
		>(24)
	);

	#[derive(Debug)]
	pub struct MinerConfig;
	impl pallet_election_provider_multi_phase::unsigned::MinerConfig for MinerConfig {
		type AccountId = AccountId;
		type MaxLength = MaxLength;
		type MaxWeight = MaxWeight;
		type MaxVotesPerVoter = MaxVotesPerVoter;
		type Solution = NposSolution24;

		fn solution_weight(
			voters: u32,
			targets: u32,
			active_voters: u32,
			desired_targets: u32,
		) -> Weight {
			// Mock a RawSolution to get the correct weight without having to do the heavy work.
			let raw = RawSolution {
				solution: NposSolution24 {
					votes1: mock_votes(
						active_voters,
						desired_targets.try_into().expect("Desired targets < u16::MAX"),
					),
					..Default::default()
				},
				..Default::default()
			};

			assert_eq!(raw.solution.voter_count(), active_voters as usize);
			assert_eq!(raw.solution.unique_targets().len(), desired_targets as usize);

			epm_dynamic::runtime_api_solution_weight(
				raw,
				SolutionOrSnapshotSize { voters, targets },
			)
		}
	}
}
