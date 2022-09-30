use crate::{opt::Solver, prelude::*, static_types};
use codec::Encode;
use frame_election_provider_support::{NposSolution, PhragMMS, SequentialPhragmen};
use frame_support::{weights::Weight, BoundedVec};
use pallet_election_provider_multi_phase::{RawSolution, SolutionOf, SolutionOrSnapshotSize};
use pin_project_lite::pin_project;
use runtime::runtime_types::pallet_election_provider_multi_phase::RoundSnapshot;
use scale_info::{PortableRegistry, TypeInfo};
use scale_value::scale::{decode_as_type, DecodeError, TypeId};
use sp_npos_elections::ElectionScore;
use std::{
	future::Future,
	pin::Pin,
	task::{Context, Poll},
	time::{Duration, Instant},
};
use subxt::{dynamic::Value, tx::DynamicTxPayload};

pub type BoundedVoters =
	Vec<(AccountId, VoteWeight, BoundedVec<AccountId, static_types::MaxVotesPerVoter>)>;
pub type Snapshot = (BoundedVoters, Vec<AccountId>, u32);

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

#[derive(Copy, Clone, Debug)]
struct EpmConstant {
	epm: &'static str,
	constant: &'static str,
}

impl EpmConstant {
	const fn new(constant: &'static str) -> Self {
		Self { epm: "ElectionProviderMultiPhase", constant }
	}

	const fn to_parts(&self) -> (&'static str, &'static str) {
		(self.epm, self.constant)
	}

	fn to_string(&self) -> String {
		format!("{}::{}", self.epm, self.constant)
	}
}

pub(crate) async fn read_metadata_constants(api: &SubxtClient) -> Result<(), Error> {
	const SIGNED_MAX_WEIGHT: EpmConstant = EpmConstant::new("SignedMaxWeight");
	const MAX_LENGTH: EpmConstant = EpmConstant::new("MinerMaxLength");
	const MAX_VOTES_PER_VOTER: EpmConstant = EpmConstant::new("MinerMaxVotesPerVoter");

	let max_weight = read_constant::<Weight>(api, SIGNED_MAX_WEIGHT)?;
	let max_length: u32 = read_constant(api, MAX_LENGTH)?;
	let max_votes_per_voter: u32 = read_constant(api, MAX_VOTES_PER_VOTER)?;

	log::trace!(
		target: LOG_TARGET,
		"updating metadata constant `{}`: {}",
		SIGNED_MAX_WEIGHT.to_string(),
		max_weight.ref_time()
	);
	log::trace!(
		target: LOG_TARGET,
		"updating metadata constant `{}`: {}",
		MAX_LENGTH.to_string(),
		max_length
	);
	log::trace!(
		target: LOG_TARGET,
		"updating metadata constant `{}`: {}",
		MAX_VOTES_PER_VOTER.to_string(),
		max_votes_per_voter
	);

	static_types::MaxWeight::set(max_weight);
	static_types::MaxLength::set(max_length);
	static_types::MaxVotesPerVoter::set(max_votes_per_voter);

	Ok(())
}

fn invalid_metadata_error<E: std::error::Error>(item: String, err: E) -> Error {
	Error::InvalidMetadata(format!("{} failed: {}", item, err))
}

fn read_constant<'a, T: serde::Deserialize<'a>>(
	api: &SubxtClient,
	constant: EpmConstant,
) -> Result<T, Error> {
	let (epm_name, constant) = constant.to_parts();

	let val = api
		.constants()
		.at(&subxt::dynamic::constant(epm_name, constant))
		.map_err(|e| invalid_metadata_error(constant.to_string(), e))?;

	scale_value::serde::from_value::<_, T>(val).map_err(|e| {
		Error::InvalidMetadata(format!("Decoding `{}` failed {}", std::any::type_name::<T>(), e))
	})
}

pub fn signed_solution_tx<S: NposSolution + Encode + TypeInfo + 'static>(
	solution: RawSolution<S>,
) -> Result<DynamicTxPayload<'static>, Error> {
	let scale_solution = to_scale_value(solution).map_err(|e| {
		Error::DynamicTransaction(format!("Failed to decode `RawSolution`: {:?}", e))
	})?;

	Ok(subxt::dynamic::tx("ElectionProviderMultiPhase", "submit", vec![scale_solution]))
}

pub fn unsigned_solution_tx<S: NposSolution + Encode + TypeInfo + 'static>(
	solution: RawSolution<S>,
	witness: SolutionOrSnapshotSize,
) -> Result<DynamicTxPayload<'static>, Error> {
	let scale_solution = to_scale_value(solution).map_err(|e| {
		Error::DynamicTransaction(format!("Failed to decode `RawSolution`: {:?}", e))
	})?;
	let scale_witness = to_scale_value(witness).map_err(|e| {
		Error::DynamicTransaction(format!("Failed to decode `SolutionOrSnapshotSize`: {:?}", e))
	})?;

	Ok(subxt::dynamic::tx(
		"ElectionProviderMultiPhase",
		"submit_unsigned",
		vec![scale_solution, scale_witness],
	))
}

fn make_type<T: scale_info::TypeInfo + 'static>() -> (TypeId, PortableRegistry) {
	let m = scale_info::MetaType::new::<T>();
	let mut types = scale_info::Registry::new();
	let id = types.register_type(&m);
	let portable_registry: PortableRegistry = types.into();

	(id.into(), portable_registry)
}

fn to_scale_value<T: scale_info::TypeInfo + 'static + Encode>(
	val: T,
) -> Result<Value, DecodeError> {
	let (ty_id, types) = make_type::<T>();

	let bytes = val.encode();

	decode_as_type(&mut bytes.as_ref(), ty_id, &types).map(|v| v.remove_context())
}
