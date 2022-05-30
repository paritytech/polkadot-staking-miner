use crate::{chain, prelude::*, BalanceIterations, Balancing, Solver};
use frame_election_provider_support::{PhragMMS, SequentialPhragmen};
use frame_support::BoundedVec;
use pallet_election_provider_multi_phase::{SolutionOf, SolutionOrSnapshotSize};
use sp_npos_elections::ElectionScore;

macro_rules! mine_solution_for {
	($runtime:tt) => {
		paste::paste! {
				/// The monitor command.
				pub(crate) async fn [<mine_solution_$runtime>](
					api: &chain::$runtime::RuntimeApi,
					hash: Option<Hash>,
					solver: Solver
				) -> Result<(SolutionOf<chain::$runtime::Config>, ElectionScore, SolutionOrSnapshotSize), Error> {

						let (voters, targets, desired_targets) = [<snapshot_$runtime>](&api, hash).await?;

						match solver {
							Solver::SeqPhragmen { iterations } => {
								BalanceIterations::set(iterations);
								Miner::<chain::$runtime::Config>::mine_solution_with_snapshot::<
									SequentialPhragmen<AccountId, Accuracy, Balancing>,
								>(voters, targets, desired_targets)
							},
							Solver::PhragMMS { iterations } => {
								BalanceIterations::set(iterations);
								Miner::<chain::$runtime::Config>::mine_solution_with_snapshot::<PhragMMS<AccountId, Accuracy, Balancing>>(
									voters,
									targets,
									desired_targets,
								)
							},
						}
						.map_err(|e| Error::Other(format!("{:?}", e)))
				}
		}
	};
}

macro_rules! snapshot_for { ($runtime:tt) => {
	paste::paste! {
	pub(crate) async fn [<snapshot_$runtime>](api: &chain::$runtime::RuntimeApi, hash: Option<Hash>) -> Result<crate::chain::$runtime::epm::Snapshot, Error> {
		use crate::chain::$runtime::{static_types, epm::RoundSnapshot};

		let RoundSnapshot { voters, targets } = api
			.storage()
			.election_provider_multi_phase()
			.snapshot(hash)
			.await?
			.unwrap_or_default();

		let desired_targets = api
			.storage()
			.election_provider_multi_phase()
			.desired_targets(hash)
			.await?
			.unwrap_or_default();


		let voters: Vec<_> = voters
			.into_iter()
			.map(|(a, b, mut c)| {
				let mut bounded_vec = BoundedVec::default();
				// If this fails just crash the task.
				bounded_vec.try_append(&mut c.0).unwrap_or_else(|_| panic!("BoundedVec capacity: {} failed; `MinerConfig::MinerMaxLength` is different from the chain data; this is a bug please file an issue", static_types::MaxVotesPerVoter::get()));
				(a, b, bounded_vec)
			})
			.collect();

			Ok((voters, targets, desired_targets))
	}
}}}

mine_solution_for!(polkadot);
mine_solution_for!(kusama);
mine_solution_for!(westend);
snapshot_for!(polkadot);
snapshot_for!(kusama);
snapshot_for!(westend);
