// Copyright 2022 Parity Technologies (UK) Ltd.
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

//! Utils to interact with single block election system.

use super::utils::{storage_addr, tx};
use crate::{
	dynamic::{pallet_api, utils::to_scale_value},
	error::Error,
	helpers::{storage_at, RuntimeDispatchInfo},
	opt::{BalanceIterations, Balancing, Solver},
	prelude::{
		runtime, AccountId, Accuracy, ChainClient, Hash, SignedSubmission, LOG_TARGET,
		SHARED_CLIENT,
	},
	prometheus,
	static_types::{self},
};
use codec::{Decode, Encode};
use polkadot_sdk::{
	frame_election_provider_support::{self, Get, NposSolution, PhragMMS, SequentialPhragmen},
	frame_support::{weights::Weight, BoundedVec},
	pallet_election_provider_multi_phase::{
		self, unsigned::TrimmingStatus, Miner, MinerConfig, RawSolution, ReadySolution, SolutionOf,
		SolutionOrSnapshotSize,
	},
	sp_npos_elections::{ElectionScore, VoteWeight},
};
use scale_info::TypeInfo;
use std::{
	cmp::Reverse,
	collections::{BTreeSet, BinaryHeap},
	marker::PhantomData,
};
use subxt::{dynamic::Value, tx::DynamicPayload};

type MinerVoterOf =
	frame_election_provider_support::Voter<AccountId, crate::static_types::MaxVotesPerVoter>;
type RoundSnapshot = pallet_election_provider_multi_phase::RoundSnapshot<AccountId, MinerVoterOf>;
type Voters =
	Vec<(AccountId, VoteWeight, BoundedVec<AccountId, crate::static_types::MaxVotesPerVoter>)>;

#[derive(Debug)]
pub struct State {
	voters: Voters,
	/// `BinaryHeap` is max-heap, so sorting is descending (pop pops the largest weight).
	/// We want to drop voters with less VoteWeight, so we're making this heap a min-heap (pop pops the smallest weight).
	voters_by_stake: BinaryHeap<Reverse<(VoteWeight, usize)>>,
}

impl State {
	fn len(&self) -> usize {
		self.voters_by_stake.len()
	}

	fn to_voters(&self) -> Voters {
		self.voters.clone()
	}
}

/// Represent voters that may be trimmed
///
/// The trimming works by removing the voter with the least amount of stake.
///
/// It's using an internal `BTreeMap` to determine which voter to remove next
/// and the voters Vec can't be sorted because the EPM pallet will index into it
/// when checking the solution.
#[derive(Debug)]
pub struct TrimmedVoters<T> {
	state: State,
	_marker: PhantomData<T>,
}

impl<T> TrimmedVoters<T>
where
	T: MinerConfig<AccountId = AccountId, MaxVotesPerVoter = static_types::MaxVotesPerVoter>
		+ Send
		+ Sync
		+ 'static,
	T::Solution: Send,
{
	/// Create a new `TrimmedVotes`.
	pub async fn new(mut voters: Voters, desired_targets: u32) -> Result<Self, Error> {
		let mut voters_by_stake = BinaryHeap::new();
		let mut targets = BTreeSet::new();

		for (idx, (_voter, stake, supports)) in voters.iter().enumerate() {
			voters_by_stake.push(Reverse((*stake, idx)));
			targets.extend(supports.iter().cloned());
		}

		loop {
			let targets_len = targets.len() as u32;
			let active_voters = voters_by_stake.len() as u32;

			let est_weight: Weight = tokio::task::spawn_blocking(move || {
				T::solution_weight(active_voters, targets_len, active_voters, desired_targets)
			})
			.await?;

			let max_weight: Weight = T::MaxWeight::get();

			if est_weight.all_lt(max_weight) {
				return Ok(Self { state: State { voters, voters_by_stake }, _marker: PhantomData });
			}

			let Some(Reverse((_, idx))) = voters_by_stake.pop() else { break };

			let rm = voters[idx].0.clone();

			// Remove votes for an account.
			for (_voter, _stake, supports) in &mut voters {
				supports.retain(|a| a != &rm);
			}

			targets.remove(&rm);
		}

		Err(Error::Feasibility("Failed to pre-trim weight < T::MaxLength".to_string()))
	}

	/// Clone the state and trim it, so it get can be reverted.
	pub fn trim(&mut self, n: usize) -> Result<State, Error> {
		let mut voters = self.state.voters.clone();
		let mut voters_by_stake = self.state.voters_by_stake.clone();

		for _ in 0..n {
			let Some(Reverse((_, idx))) = voters_by_stake.pop() else {
				return Err(Error::Feasibility("Failed to pre-trim len".to_string()));
			};
			let rm = voters[idx].0.clone();

			// Remove votes for an account.
			for (_voter, _stake, supports) in &mut voters {
				supports.retain(|a| a != &rm);
			}
		}

		Ok(State { voters, voters_by_stake })
	}

	pub fn to_voters(&self) -> Voters {
		self.state.voters.clone()
	}

	pub fn len(&self) -> usize {
		self.state.len()
	}
}

/// Helper to construct a set emergency solution transaction.
pub(crate) fn set_emergency_result<A: Encode + TypeInfo + 'static>(
	supports: frame_election_provider_support::Supports<A>,
) -> Result<DynamicPayload, Error> {
	let scale_result = to_scale_value(supports)
		.map_err(|e| Error::DynamicTransaction(format!("Failed to encode `Supports`: {:?}", e)))?;

	Ok(tx(pallet_api::election_provider_multi_phase::tx::EMERGENCY, vec![scale_result]))
}

/// Helper to construct a signed solution transaction.
pub fn signed_solution<S: NposSolution + Encode + TypeInfo + 'static>(
	solution: RawSolution<S>,
) -> Result<DynamicPayload, Error> {
	let scale_solution = to_scale_value(solution).map_err(|e| {
		Error::DynamicTransaction(format!("Failed to encode `RawSolution`: {:?}", e))
	})?;

	Ok(tx(pallet_api::election_provider_multi_phase::tx::SUBMIT, vec![scale_solution]))
}

/// Helper to construct a unsigned solution transaction.
pub fn unsigned_solution<S: NposSolution + Encode + TypeInfo + 'static>(
	solution: RawSolution<S>,
	witness: SolutionOrSnapshotSize,
) -> Result<DynamicPayload, Error> {
	let scale_solution = to_scale_value(solution)?;
	let scale_witness = to_scale_value(witness)?;

	Ok(tx(
		pallet_api::election_provider_multi_phase::tx::SUBMIT_UNSIGNED,
		vec![scale_solution, scale_witness],
	))
}

/// Helper to the signed submissions at the current block.
pub async fn signed_submission_at<S: NposSolution + Decode + TypeInfo + 'static>(
	idx: u32,
	block_hash: Option<Hash>,
	api: &ChainClient,
) -> Result<Option<SignedSubmission<S>>, Error> {
	let scale_idx = Value::u128(idx as u128);
	let addr = storage_addr(
		pallet_api::election_provider_multi_phase::storage::SIGNED_SUBMISSIONS_MAP,
		vec![scale_idx],
	);

	let storage = storage_at(block_hash, api).await?;

	match storage.fetch(&addr).await {
		Ok(Some(val)) => {
			let submissions = Decode::decode(&mut val.encoded())?;
			Ok(Some(submissions))
		},
		Ok(None) => Ok(None),
		Err(err) => Err(err.into()),
	}
}

/// Helper to get the signed submissions at the current state.
pub async fn snapshot_at(
	block_hash: Option<Hash>,
	api: &ChainClient,
) -> Result<RoundSnapshot, Error> {
	let empty = Vec::<Value>::new();
	let addr = storage_addr(pallet_api::election_provider_multi_phase::storage::SNAPSHOT, empty);

	let storage = storage_at(block_hash, api).await?;

	match storage.fetch(&addr).await {
		Ok(Some(val)) => {
			let snapshot = Decode::decode(&mut val.encoded())?;
			Ok(snapshot)
		},
		Ok(None) => Err(Error::EmptySnapshot),
		Err(err) => Err(err.into()),
	}
}

pub async fn mine_solution<T>(
	solver: Solver,
	targets: Vec<AccountId>,
	voters: Voters,
	desired_targets: u32,
) -> Result<(SolutionOf<T>, ElectionScore, SolutionOrSnapshotSize, TrimmingStatus), Error>
where
	T: MinerConfig<AccountId = AccountId, MaxVotesPerVoter = static_types::MaxVotesPerVoter>
		+ Send
		+ Sync
		+ 'static,
	T::Solution: Send,
{
	match tokio::task::spawn_blocking(move || match solver {
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
	.await
	{
		Ok(Ok(s)) => Ok(s),
		Err(e) => Err(e.into()),
		Ok(Err(e)) => Err(Error::Other(format!("{:?}", e))),
	}
}

/// Helper to fetch snapshot data via RPC
/// and compute an NPos solution via [`pallet_election_provider_multi_phase`].
pub async fn fetch_snapshot_and_mine_solution<T>(
	api: &ChainClient,
	block_hash: Option<Hash>,
	solver: Solver,
	round: u32,
	forced_desired_targets: Option<u32>,
) -> Result<MinedSolution<T>, Error>
where
	T: MinerConfig<AccountId = AccountId, MaxVotesPerVoter = static_types::MaxVotesPerVoter>
		+ Send
		+ Sync
		+ 'static,
	T::Solution: Send,
{
	let snapshot = snapshot_at(block_hash, api).await?;
	let storage = storage_at(block_hash, api).await?;

	let desired_targets = match forced_desired_targets {
		Some(x) => x,
		None => storage
			.fetch(&runtime::storage().election_provider_multi_phase().desired_targets())
			.await?
			.expect("Snapshot is non-empty; `desired_target` should exist; qed"),
	};

	let minimum_untrusted_score = storage
		.fetch(&runtime::storage().election_provider_multi_phase().minimum_untrusted_score())
		.await?
		.map(|score| score.0);

	let mut voters = TrimmedVoters::<T>::new(snapshot.voters.clone(), desired_targets).await?;

	let (solution, score, solution_or_snapshot_size, trim_status) = mine_solution::<T>(
		solver.clone(),
		snapshot.targets.clone(),
		voters.to_voters(),
		desired_targets,
	)
	.await?;

	if !trim_status.is_trimmed() {
		return Ok(MinedSolution {
			round,
			desired_targets,
			snapshot,
			minimum_untrusted_score,
			solution,
			score,
			solution_or_snapshot_size,
		});
	}

	prometheus::on_trim_attempt();

	let mut l = 1;
	let mut h = voters.len();
	let mut best_solution = None;

	while l <= h {
		let mid = ((h - l) / 2) + l;

		let next_state = voters.trim(mid)?;

		let (solution, score, solution_or_snapshot_size, trim_status) = mine_solution::<T>(
			solver.clone(),
			snapshot.targets.clone(),
			next_state.to_voters(),
			desired_targets,
		)
		.await?;

		if !trim_status.is_trimmed() {
			best_solution = Some((solution, score, solution_or_snapshot_size));
			h = mid - 1;
		} else {
			l = mid + 1;
		}
	}

	if let Some((solution, score, solution_or_snapshot_size)) = best_solution {
		prometheus::on_trim_success();

		Ok(MinedSolution {
			round,
			desired_targets,
			snapshot,
			minimum_untrusted_score,
			solution,
			score,
			solution_or_snapshot_size,
		})
	} else {
		Err(Error::Feasibility("Failed pre-trim length".to_string()))
	}
}

/// The result of calling [`fetch_snapshot_and_mine_solution`].
pub struct MinedSolution<T: MinerConfig> {
	round: u32,
	desired_targets: u32,
	snapshot: RoundSnapshot,
	minimum_untrusted_score: Option<ElectionScore>,
	solution: T::Solution,
	score: ElectionScore,
	solution_or_snapshot_size: SolutionOrSnapshotSize,
}

impl<T> MinedSolution<T>
where
	T: MinerConfig<AccountId = AccountId, MaxVotesPerVoter = static_types::MaxVotesPerVoter>
		+ Send
		+ Sync
		+ 'static,
	T::Solution: Send,
{
	pub fn solution(&self) -> T::Solution {
		self.solution.clone()
	}

	pub fn score(&self) -> ElectionScore {
		self.score
	}

	pub fn size(&self) -> SolutionOrSnapshotSize {
		self.solution_or_snapshot_size
	}

	/// Check that this solution is feasible
	///
	/// Returns a [`pallet_election_provider_multi_phase::ReadySolution`] if the check passes.
	pub fn feasibility_check(
		&self,
	) -> Result<ReadySolution<AccountId, T::MaxWinners, T::MaxBackersPerWinner>, Error> {
		match Miner::<T>::feasibility_check(
			RawSolution { solution: self.solution.clone(), score: self.score, round: self.round },
			pallet_election_provider_multi_phase::ElectionCompute::Signed,
			self.desired_targets,
			self.snapshot.clone(),
			self.round,
			self.minimum_untrusted_score,
		) {
			Ok(ready_solution) => Ok(ready_solution),
			Err(e) => {
				log::error!(target: LOG_TARGET, "Solution feasibility error {:?}", e);
				Err(Error::Feasibility(format!("{:?}", e)))
			},
		}
	}
}

impl<T: MinerConfig> std::fmt::Debug for MinedSolution<T> {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("MinedSolution")
			.field("round", &self.round)
			.field("desired_targets", &self.desired_targets)
			.field("score", &self.score)
			.finish()
	}
}

/// Fetch the weight for `RawSolution` from a remote node
pub async fn runtime_api_solution_weight<S: Encode + NposSolution + TypeInfo + 'static>(
	raw_solution: RawSolution<S>,
	witness: SolutionOrSnapshotSize,
) -> Result<Weight, Error> {
	let tx = unsigned_solution(raw_solution, witness)?;

	let client = SHARED_CLIENT.get().expect("shared client is configured as start; qed");

	let call_data = {
		let mut buffer = Vec::new();

		let encoded_call = client.chain_api().tx().call_data(&tx).unwrap();
		let encoded_len = encoded_call.len() as u32;

		buffer.extend(encoded_call);
		encoded_len.encode_to(&mut buffer);

		buffer
	};

	let bytes = client
		.rpc()
		.state_call("TransactionPaymentCallApi_query_call_info", Some(&call_data), None)
		.await?;

	let info: RuntimeDispatchInfo = Decode::decode(&mut bytes.as_ref())?;

	log::trace!(
		target: LOG_TARGET,
		"Received weight of `Solution Extrinsic` from remote node: {:?}",
		info.weight
	);

	Ok(info.weight)
}

/// Helper to mock the votes based on `voters` and `desired_targets`.
pub fn mock_votes(voters: u32, desired_targets: u16) -> Option<Vec<(u32, u16)>> {
	if voters >= desired_targets as u32 {
		Some((0..voters).zip((0..desired_targets).cycle()).collect())
	} else {
		None
	}
}

#[cfg(test)]
#[test]
fn mock_votes_works() {
	assert_eq!(mock_votes(3, 2), Some(vec![(0, 0), (1, 1), (2, 0)]));
	assert_eq!(mock_votes(3, 3), Some(vec![(0, 0), (1, 1), (2, 2)]));
	assert_eq!(mock_votes(2, 3), None);
}
