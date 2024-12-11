use frame_support::traits::ConstU32;

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

impl_atomic_u32_parameter_types!(pages, Pages);
impl_atomic_u32_parameter_types!(max_votes_per_voter, MaxVotesPerVoter);
impl_atomic_u32_parameter_types!(target_snapshot_per_block, TargetSnapshotPerBlock);
impl_atomic_u32_parameter_types!(voter_snapshot_per_block, VoterSnapshotPerBlock);
impl_atomic_u32_parameter_types!(max_winners_per_page, MaxWinnersPerPage);
impl_atomic_u32_parameter_types!(max_backers_per_winner, MaxBackersPerWinner);

pub mod staking_dev {
	use super::*;
	use crate::prelude::*;

	frame_election_provider_support::generate_solution_type!(
		#[compact]
		pub struct NposSolution16::<
			VoterIndex = u32,
			TargetIndex = u16,
			Accuracy = sp_runtime::PerU16,
			MaxVoters = ConstU32::<22500> // TODO: fetch
		>(16)
	);

	#[derive(Debug)]
	pub struct MinerConfig;
	impl pallet_election_provider_multi_block::unsigned::miner::Config for MinerConfig {
		type AccountId = AccountId;
		type Solution = NposSolution16;
		type Solver = Solver;
		type Pages = Pages;
		type MaxVotesPerVoter = MaxVotesPerVoter;
		type MaxWinnersPerPage = MaxWinnersPerPage;
		type MaxBackersPerWinner = MaxBackersPerWinner;
		type VoterSnapshotPerBlock = VoterSnapshotPerBlock;
		type TargetSnapshotPerBlock = TargetSnapshotPerBlock;
		type MaxWeight = (); // TODO
		type MaxLength = (); // TODO
	}
}
