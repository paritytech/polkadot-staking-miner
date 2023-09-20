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

//! Wrappers or helpers for [`pallet_election_provider_multi_phase`].

use crate::{
	error::Error,
	helpers::{storage_at, RuntimeDispatchInfo},
	opt::{BalanceIterations, Balancing, Solver},
	prelude::*,
	static_types,
};

use std::collections::BTreeMap;

use codec::{Decode, Encode};
use frame_election_provider_support::{NposSolution, PhragMMS, SequentialPhragmen};
use frame_support::weights::Weight;
use pallet_election_provider_multi_phase::{RawSolution, ReadySolution, SolutionOrSnapshotSize};
use scale_info::{PortableRegistry, TypeInfo};
use scale_value::scale::{decode_as_type, TypeId};
use sp_core::Bytes;
use sp_npos_elections::ElectionScore;
use sp_runtime::Perbill;
use subxt::{dynamic::Value, rpc::rpc_params, tx::DynamicPayload};

const EPM_PALLET_NAME: &str = "ElectionProviderMultiPhase";

type MinerVoterOf =
	frame_election_provider_support::Voter<AccountId, crate::static_types::MaxVotesPerVoter>;

type RoundSnapshot = pallet_election_provider_multi_phase::RoundSnapshot<AccountId, MinerVoterOf>;

#[derive(Copy, Clone, Debug)]
struct EpmConstant {
	epm: &'static str,
	constant: &'static str,
}

impl EpmConstant {
	const fn new(constant: &'static str) -> Self {
		Self { epm: EPM_PALLET_NAME, constant }
	}

	const fn to_parts(self) -> (&'static str, &'static str) {
		(self.epm, self.constant)
	}
}

impl std::fmt::Display for EpmConstant {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_fmt(format_args!("{}::{}", self.epm, self.constant))
	}
}

/// Read the constants from the metadata and updates the static types.
pub(crate) async fn update_metadata_constants(api: &SubxtClient) -> Result<(), Error> {
	const SIGNED_MAX_WEIGHT: EpmConstant = EpmConstant::new("SignedMaxWeight");
	const MAX_LENGTH: EpmConstant = EpmConstant::new("MinerMaxLength");
	const MAX_VOTES_PER_VOTER: EpmConstant = EpmConstant::new("MinerMaxVotesPerVoter");
	// NOTE: `MaxWinners` is used instead of `MinerMaxWinners` to work with older metadata.
	const MAX_WINNERS: EpmConstant = EpmConstant::new("MaxWinners");

	fn log_metadata(metadata: EpmConstant, val: impl std::fmt::Display) {
		log::trace!(target: LOG_TARGET, "updating metadata constant `{metadata}`: {val}",);
	}

	let max_weight = read_constant::<Weight>(api, SIGNED_MAX_WEIGHT)?;
	let max_length: u32 = read_constant(api, MAX_LENGTH)?;
	let max_votes_per_voter: u32 = read_constant(api, MAX_VOTES_PER_VOTER)?;
	let max_winners: u32 = read_constant(api, MAX_WINNERS)?;

	log_metadata(SIGNED_MAX_WEIGHT, max_weight);
	log_metadata(MAX_LENGTH, max_length);
	log_metadata(MAX_VOTES_PER_VOTER, max_votes_per_voter);
	log_metadata(MAX_WINNERS, max_winners);

	static_types::MaxWeight::set(max_weight);
	static_types::MaxLength::set(max_length);
	static_types::MaxVotesPerVoter::set(max_votes_per_voter);
	static_types::MaxWinners::set(max_winners);

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
		.map_err(|e| invalid_metadata_error(constant.to_string(), e))?
		.to_value()?;

	scale_value::serde::from_value::<_, T>(val).map_err(|e| {
		Error::InvalidMetadata(format!("Decoding `{}` failed {}", std::any::type_name::<T>(), e))
	})
}

/// Helper to construct a set emergency solution transaction.
pub(crate) fn set_emergency_result<A: Encode + TypeInfo + 'static>(
	supports: frame_election_provider_support::Supports<A>,
) -> Result<DynamicPayload, Error> {
	let scale_result = to_scale_value(supports)
		.map_err(|e| Error::DynamicTransaction(format!("Failed to encode `Supports`: {:?}", e)))?;

	Ok(subxt::dynamic::tx(EPM_PALLET_NAME, "set_emergency_election_result", vec![scale_result]))
}

/// Helper to construct a signed solution transaction.
pub fn signed_solution<S: NposSolution + Encode + TypeInfo + 'static>(
	solution: RawSolution<S>,
) -> Result<DynamicPayload, Error> {
	let scale_solution = to_scale_value(solution).map_err(|e| {
		Error::DynamicTransaction(format!("Failed to encode `RawSolution`: {:?}", e))
	})?;

	Ok(subxt::dynamic::tx(EPM_PALLET_NAME, "submit", vec![scale_solution]))
}

/// Helper to construct a unsigned solution transaction.
pub fn unsigned_solution<S: NposSolution + Encode + TypeInfo + 'static>(
	solution: RawSolution<S>,
	witness: SolutionOrSnapshotSize,
) -> Result<DynamicPayload, Error> {
	let scale_solution = to_scale_value(solution)?;
	let scale_witness = to_scale_value(witness)?;

	Ok(subxt::dynamic::tx(EPM_PALLET_NAME, "submit_unsigned", vec![scale_solution, scale_witness]))
}

/// Helper to the signed submissions at the current block.
pub async fn signed_submission_at<S: NposSolution + Decode + TypeInfo + 'static>(
	idx: u32,
	block_hash: Option<Hash>,
	api: &SubxtClient,
) -> Result<Option<SignedSubmission<S>>, Error> {
	let scale_idx = Value::u128(idx as u128);
	let addr = subxt::dynamic::storage(EPM_PALLET_NAME, "SignedSubmissionsMap", vec![scale_idx]);

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
	api: &SubxtClient,
) -> Result<RoundSnapshot, Error> {
	let empty = Vec::<Value>::new();
	let addr = subxt::dynamic::storage(EPM_PALLET_NAME, "Snapshot", empty);

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

/// Helper to fetch snapshot data via RPC

/// and compute an NPos solution via [`pallet_election_provider_multi_phase`].
pub async fn fetch_snapshot_and_mine_solution<T>(
	api: &SubxtClient,
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

	let mut voters = snapshot.voters.clone();
	// The voters can't be sorted and this BTreeMap is used to lookup
	// each voter by stake instead of doing linear search in the `Vec`
	let mut voters_by_stake = {
		let mut map = BTreeMap::new();

		for (idx, (_voter, stake, _supports)) in voters.iter().enumerate() {
			map.insert(stake.clone(), idx);
		}

		map
	};

	log::info!(
		target: LOG_TARGET,
		"mine solution: desired_targets={}, voters={}, targets={}",
		desired_targets,
		voters.len(),
		snapshot.targets.len()
	);

	let mut first_mined_score = None;

	loop {
		let s = solver.clone();
		let v = voters.clone();
		let t = snapshot.targets.clone();

		let blocking_task =
			tokio::task::spawn_blocking(move || match s {
				Solver::SeqPhragmen { iterations } => {
					BalanceIterations::set(iterations);
					Miner::<T>::mine_solution_with_snapshot::<
						SequentialPhragmen<AccountId, Accuracy, Balancing>,
					>(v, t, desired_targets)
				},
				Solver::PhragMMS { iterations } => {
					BalanceIterations::set(iterations);
					Miner::<T>::mine_solution_with_snapshot::<
						PhragMMS<AccountId, Accuracy, Balancing>,
					>(v, t, desired_targets)
				},
			})
			.await;

		match blocking_task {
			Ok(Ok((solution, score, solution_or_snapshot_size, t))) if !t.is_trimmed() => {
				if solution.unique_targets().len() != desired_targets as usize {
					return Err(Error::Feasibility(format!(
						"Invalid winner count {}, expected {desired_targets}",
						solution.unique_targets().len()
					)));
				}

				// TODO: make this configurable
				let trimmed_score_bad = first_mined_score.map_or(false, |s: ElectionScore| {
					s.strict_threshold_better(score, Perbill::from_percent(10))
				});

				if trimmed_score_bad {
					return Err(Error::Feasibility("Pre-trimmed score is too bad".to_string()));
				}

				return Ok(MinedSolution {
					round,
					desired_targets,
					snapshot,
					minimum_untrusted_score,
					solution,
					score,
					solution_or_snapshot_size,
				});
			},
			Ok(Ok((solution, score, _, _))) => {
				if solution.unique_targets().len() != desired_targets as usize {
					return Err(Error::Feasibility(format!(
						"Invalid winner count {}, expected {desired_targets}",
						solution.unique_targets().len()
					)));
				}

				first_mined_score.get_or_insert(score);

				let Some((_, idx)) = voters_by_stake.pop_first() else {
					return Err(Error::Feasibility(
						"Couldn't pre-trim votes to prevent trimming".to_string(),
					));
				};
				let rm = voters[idx].0.clone();

				// Remove votes for an account.
				//
				// Ideally, we would just remove the voter from the voters and targets here but
				// it would modify the snapshot and the solution becomes invalid in the feasibility check.
				for (_voter, _stake, supports) in &mut voters {
					supports.retain(|a| a != &rm);
				}
			},
			Ok(Err(err)) => return Err(Error::Other(format!("{:?}", err))),
			Err(err) => return Err(err.into()),
		};
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
	pub fn feasibility_check(&self) -> Result<ReadySolution<AccountId, T::MaxWinners>, Error> {
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

fn make_type<T: scale_info::TypeInfo + 'static>() -> (TypeId, PortableRegistry) {
	let m = scale_info::MetaType::new::<T>();
	let mut types = scale_info::Registry::new();
	let id = types.register_type(&m);
	let portable_registry: PortableRegistry = types.into();

	(id.id, portable_registry)
}

fn to_scale_value<T: scale_info::TypeInfo + 'static + Encode>(val: T) -> Result<Value, Error> {
	let (ty_id, types) = make_type::<T>();

	let bytes = val.encode();

	decode_as_type(&mut bytes.as_ref(), ty_id, &types)
		.map(|v| v.remove_context())
		.map_err(|e| {
			Error::DynamicTransaction(format!(
				"Failed to decode {}: {:?}",
				std::any::type_name::<T>(),
				e
			))
		})
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

		let encoded_call = client.tx().call_data(&tx).unwrap();
		let encoded_len = encoded_call.len() as u32;

		buffer.extend(encoded_call);
		encoded_len.encode_to(&mut buffer);

		Bytes(buffer)
	};

	let bytes: Bytes = client
		.rpc()
		.request("state_call", rpc_params!["TransactionPaymentCallApi_query_call_info", call_data])
		.await?;

	let info: RuntimeDispatchInfo = Decode::decode(&mut bytes.0.as_ref())?;

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
