use crate::{opt::Solver, prelude::*, static_types};
use frame_election_provider_support::{PhragMMS, SequentialPhragmen};
use frame_support::BoundedVec;
use pallet_election_provider_multi_phase::{SolutionOf, SolutionOrSnapshotSize};
use pin_project_lite::pin_project;
use runtime::runtime_types::pallet_election_provider_multi_phase::RoundSnapshot;
use sp_npos_elections::ElectionScore;
use std::{
	future::Future,
	pin::Pin,
	task::{Context, Poll},
	time::{Duration, Instant},
};

pin_project! {
	pub struct Timed<Fut>
		where
		Fut: Future,
	{
		#[pin]
		inner: Fut,
		start: Option<Instant>,
	}
}

impl<Fut> Future for Timed<Fut>
where
	Fut: Future,
{
	type Output = (Fut::Output, Duration);

	fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
		let this = self.project();
		let start = this.start.get_or_insert_with(Instant::now);

		match this.inner.poll(cx) {
			Poll::Pending => Poll::Pending,
			Poll::Ready(v) => {
				let elapsed = start.elapsed();
				Poll::Ready((v, elapsed))
			},
		}
	}
}

pub trait TimedFuture: Sized + Future {
	fn timed(self) -> Timed<Self> {
		Timed { inner: self, start: None }
	}
}

impl<F: Future> TimedFuture for F {}

pub async fn mine_solution<T>(
	api: &SubxtClient,
	hash: Option<Hash>,
	solver: Solver,
) -> Result<(SolutionOf<T>, ElectionScore, SolutionOrSnapshotSize), Error>
where
	T: MinerConfig<AccountId = AccountId, MaxVotesPerVoter = static_types::MaxVotesPerVoter>
		+ Send
		+ Sync
		+ 'static,
	T::Solution: Send,
{
	let (voters, targets, desired_targets) = snapshot::<T>(&api, hash).await?;

	let blocking_task = tokio::task::spawn_blocking(move || match solver {
		Solver::SeqPhragmen { iterations } => {
			BalanceIterations::set(iterations);
			Miner::<T>::mine_solution_with_snapshot::<
				SequentialPhragmen<AccountId, Accuracy, Balancing>,
			>(voters, targets, desired_targets)
		},
		Solver::PhragMMS { iterations } => {
			BalanceIterations::set(iterations);
			Miner::<T>::mine_solution_with_snapshot::<PhragMMS<AccountId, Accuracy, Balancing>>(
				voters,
				targets,
				desired_targets,
			)
		},
	})
	.await;

	match blocking_task {
		Ok(Ok(res)) => Ok(res),
		Ok(Err(err)) => Err(Error::Other(format!("{:?}", err))),
		Err(err) => Err(Error::Other(format!("{:?}", err))),
	}
}

pub async fn snapshot<T: MinerConfig + Send + Sync + 'static>(
	api: &SubxtClient,
	hash: Option<Hash>,
) -> Result<Snapshot, Error>
where
	T: MinerConfig<AccountId = AccountId> + Send + Sync + 'static,
	T::Solution: Send,
{
	let RoundSnapshot { voters, targets } = api
		.storage()
		.fetch(&runtime::storage().election_provider_multi_phase().snapshot(), hash)
		.await?
		.unwrap_or_default();

	let desired_targets = api
		.storage()
		.fetch(&runtime::storage().election_provider_multi_phase().desired_targets(), hash)
		.await?
		.unwrap_or_default();

	let voters: Vec<_> = voters
	.into_iter()
	.map(|(a, b, mut c)| {
		let mut bounded_vec: BoundedVec<AccountId, static_types::MaxVotesPerVoter> = BoundedVec::default();
		// If this fails just crash the task.
		bounded_vec.try_append(&mut c.0).unwrap_or_else(|_| panic!("BoundedVec capacity: {} failed; `MinerConfig::MaxVotesPerVoter` is different from the chain data; this is a bug please file an issue", static_types::MaxVotesPerVoter::get()));
		(a, b, bounded_vec)
	})
	.collect();

	Ok((voters, targets, desired_targets))
}

pub fn mock_votes(voters: u32, desired_targets: u16) -> Vec<(u32, u16)> {
	assert!(voters >= desired_targets as u32);
	(0..voters).zip((0..desired_targets).cycle()).collect()
}

#[cfg(test)]
#[test]
fn mock_votes_works() {
	assert_eq!(mock_votes(3, 2), vec![(0, 0), (1, 1), (2, 0)]);
	assert_eq!(mock_votes(3, 3), vec![(0, 0), (1, 1), (2, 2)]);
}
