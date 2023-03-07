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

use crate::{epm, prelude::*};
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

	static REF_TIME: AtomicU64 = AtomicU64::new(0);
	static PROOF_SIZE: AtomicU64 = AtomicU64::new(0);
	pub struct MaxWeight;

	impl MaxWeight {
		pub fn get() -> Weight {
			Weight::from_parts(REF_TIME.load(Ordering::SeqCst), PROOF_SIZE.load(Ordering::SeqCst))
		}
	}

	impl frame_support::traits::Get<Weight> for MaxWeight {
		fn get() -> Weight {
			Self::get()
		}
	}

	impl MaxWeight {
		pub fn set(weight: Weight) {
			REF_TIME.store(weight.ref_time(), Ordering::SeqCst);
			PROOF_SIZE.store(weight.proof_size(), Ordering::SeqCst);
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
			let raw = RawSolution {
				solution: NposSolution16 {
					votes1: epm::mock_votes(
						active_voters,
						desired_targets.try_into().expect("Desired targets < u16::MAX"),
					),
					..Default::default()
				},
				..Default::default()
			};

			assert_eq!(raw.solution.voter_count(), active_voters as usize);
			assert_eq!(raw.solution.unique_targets().len(), desired_targets as usize);

			futures::executor::block_on(epm::runtime_api_solution_weight(
				raw,
				SolutionOrSnapshotSize { voters, targets },
			))
			.expect("solution_weight should work")
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
					votes1: epm::mock_votes(
						active_voters,
						desired_targets.try_into().expect("Desired targets < u16::MAX"),
					),
					..Default::default()
				},
				..Default::default()
			};

			assert_eq!(raw.solution.voter_count(), active_voters as usize);
			assert_eq!(raw.solution.unique_targets().len(), desired_targets as usize);

			futures::executor::block_on(epm::runtime_api_solution_weight(
				raw,
				SolutionOrSnapshotSize { voters, targets },
			))
			.expect("solution_weight should work")
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
					votes1: epm::mock_votes(
						active_voters,
						desired_targets.try_into().expect("Desired targets < u16::MAX"),
					),
					..Default::default()
				},
				..Default::default()
			};

			assert_eq!(raw.solution.voter_count(), active_voters as usize);
			assert_eq!(raw.solution.unique_targets().len(), desired_targets as usize);

			futures::executor::block_on(epm::runtime_api_solution_weight(
				raw,
				SolutionOrSnapshotSize { voters, targets },
			))
			.expect("solution_weight should work")
		}
	}
}
