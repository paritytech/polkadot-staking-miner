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

use crate::{dynamic::legacy as dynamic, macros::impl_u32_parameter_type, prelude::AccountId};
use polkadot_sdk::{
    frame_election_provider_support::{self, traits::NposSolution},
    frame_support::{self, traits::ConstU32, weights::Weight},
    pallet_election_provider_multi_phase::{self, RawSolution, SolutionOrSnapshotSize},
    sp_runtime,
};

mod max_weight {
    use polkadot_sdk::frame_support::weights::Weight;
    use std::sync::atomic::{AtomicU64, Ordering};

    static REF_TIME: AtomicU64 = AtomicU64::new(0);
    static PROOF_SIZE: AtomicU64 = AtomicU64::new(0);

    pub struct MaxWeight;

    impl MaxWeight {
        pub fn get() -> Weight {
            Weight::from_parts(
                REF_TIME.load(Ordering::SeqCst),
                PROOF_SIZE.load(Ordering::SeqCst),
            )
        }
    }

    impl polkadot_sdk::frame_support::traits::Get<Weight> for MaxWeight {
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

impl_u32_parameter_type!(max_length, MaxLength);
impl_u32_parameter_type!(max_votes, MaxVotesPerVoter);
impl_u32_parameter_type!(max_winners, MaxWinners);
pub use max_weight::MaxWeight;

pub mod node {
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
        type MaxBackersPerWinner = ConstU32<22500>;
        type Solution = NposSolution16;
        type MaxWinners = MaxWinners;

        fn solution_weight(
            voters: u32,
            targets: u32,
            active_voters: u32,
            desired_targets: u32,
        ) -> Weight {
            let Some(votes) = dynamic::mock_votes(
                active_voters,
                desired_targets
                    .try_into()
                    .expect("Desired targets < u16::MAX"),
            ) else {
                return Weight::MAX;
            };

            // Mock a RawSolution to get the correct weight without having to do the heavy work.
            let raw = RawSolution {
                solution: NposSolution16 {
                    votes1: votes,
                    ..Default::default()
                },
                ..Default::default()
            };

            if raw.solution.voter_count() != active_voters as usize
                || raw.solution.unique_targets().len() != desired_targets as usize
            {
                return Weight::MAX;
            }

            futures::executor::block_on(dynamic::runtime_api_solution_weight(
                raw,
                SolutionOrSnapshotSize { voters, targets },
            ))
            .expect("solution_weight should work")
        }
    }
}

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
        type MaxBackersPerWinner = ConstU32<22500>;
        type Solution = NposSolution16;
        type MaxWinners = MaxWinners;

        fn solution_weight(
            voters: u32,
            targets: u32,
            active_voters: u32,
            desired_targets: u32,
        ) -> Weight {
            let Some(votes) = dynamic::mock_votes(
                active_voters,
                desired_targets
                    .try_into()
                    .expect("Desired targets < u16::MAX"),
            ) else {
                return Weight::MAX;
            };

            // Mock a RawSolution to get the correct weight without having to do the heavy work.
            let raw = RawSolution {
                solution: NposSolution16 {
                    votes1: votes,
                    ..Default::default()
                },
                ..Default::default()
            };

            if raw.solution.voter_count() != active_voters as usize
                || raw.solution.unique_targets().len() != desired_targets as usize
            {
                return Weight::MAX;
            }

            futures::executor::block_on(dynamic::runtime_api_solution_weight(
                raw,
                SolutionOrSnapshotSize { voters, targets },
            ))
            .expect("solution_weight should work")
        }
    }
}

pub mod polkadot {
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
        type MaxBackersPerWinner = ConstU32<22500>;
        type Solution = NposSolution16;
        type MaxWinners = MaxWinners;

        fn solution_weight(
            voters: u32,
            targets: u32,
            active_voters: u32,
            desired_targets: u32,
        ) -> Weight {
            let Some(votes) = dynamic::mock_votes(
                active_voters,
                desired_targets
                    .try_into()
                    .expect("Desired targets < u16::MAX"),
            ) else {
                return Weight::MAX;
            };

            // Mock a RawSolution to get the correct weight without having to do the heavy work.
            let raw = RawSolution {
                solution: NposSolution16 {
                    votes1: votes,
                    ..Default::default()
                },
                ..Default::default()
            };

            if raw.solution.voter_count() != active_voters as usize
                || raw.solution.unique_targets().len() != desired_targets as usize
            {
                return Weight::MAX;
            }

            futures::executor::block_on(dynamic::runtime_api_solution_weight(
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
        type MaxBackersPerWinner = ConstU32<22500>;
        type Solution = NposSolution24;
        type MaxWinners = MaxWinners;

        fn solution_weight(
            voters: u32,
            targets: u32,
            active_voters: u32,
            desired_targets: u32,
        ) -> Weight {
            let Some(votes) = dynamic::mock_votes(
                active_voters,
                desired_targets
                    .try_into()
                    .expect("Desired targets < u16::MAX"),
            ) else {
                return Weight::MAX;
            };

            // Mock a RawSolution to get the correct weight without having to do the heavy work.
            let raw = RawSolution {
                solution: NposSolution24 {
                    votes1: votes,
                    ..Default::default()
                },
                ..Default::default()
            };

            if raw.solution.voter_count() != active_voters as usize
                || raw.solution.unique_targets().len() != desired_targets as usize
            {
                return Weight::MAX;
            }

            futures::executor::block_on(dynamic::runtime_api_solution_weight(
                raw,
                SolutionOrSnapshotSize { voters, targets },
            ))
            .expect("solution_weight should work")
        }
    }
}
