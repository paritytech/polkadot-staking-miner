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

use crate::{helpers::RuntimeDispatchInfo, opt::Solver, prelude::*, static_types};
use codec::{Decode, Encode};
use frame_election_provider_support::{NposSolution, PhragMMS, SequentialPhragmen};
use frame_support::{weights::Weight, BoundedVec};
use pallet_election_provider_multi_phase::{RawSolution, SolutionOf, SolutionOrSnapshotSize};
use runtime::runtime_types::pallet_election_provider_multi_phase::RoundSnapshot;
use scale_info::{PortableRegistry, TypeInfo};
use scale_value::scale::{decode_as_type, encode_as_type, TypeId};
use sp_core::Bytes;
use sp_npos_elections::ElectionScore;
use subxt::{dynamic::Value, rpc::rpc_params, tx::DynamicTxPayload};

const EPM_PALLET_NAME: &str = "ElectionProviderMultiPhase";

#[derive(Copy, Clone, Debug)]
struct EpmConstant {
	epm: &'static str,
	constant: &'static str,
}

impl EpmConstant {
	const fn new(constant: &'static str) -> Self {
		Self { epm: EPM_PALLET_NAME, constant }
	}

	const fn to_parts(&self) -> (&'static str, &'static str) {
		(self.epm, self.constant)
	}

	fn to_string(&self) -> String {
		format!("{}::{}", self.epm, self.constant)
	}
}

/// Read the constants from the metadata and updates the static types.
pub(crate) async fn update_metadata_constants(api: &SubxtClient) -> Result<(), Error> {
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

/// Helper to construct a signed solution transaction.
pub fn signed_solution<S: NposSolution + Encode + TypeInfo + 'static>(
	solution: RawSolution<S>,
) -> Result<DynamicTxPayload<'static>, Error> {
	let scale_solution = to_scale_value(solution).map_err(|e| {
		Error::DynamicTransaction(format!("Failed to decode `RawSolution`: {:?}", e))
	})?;

	Ok(subxt::dynamic::tx(EPM_PALLET_NAME, "submit", vec![scale_solution]))
}

/// Helper to construct a unsigned solution transaction.
pub fn unsigned_solution<S: NposSolution + Encode + TypeInfo + 'static>(
	solution: RawSolution<S>,
	witness: SolutionOrSnapshotSize,
) -> Result<DynamicTxPayload<'static>, Error> {
	let scale_solution = to_scale_value(solution)?;
	let scale_witness = to_scale_value(witness)?;

	Ok(subxt::dynamic::tx(EPM_PALLET_NAME, "submit_unsigned", vec![scale_solution, scale_witness]))
}

/// Helper to the signed submissions at the current block.
pub async fn signed_submission_at<S: NposSolution + Decode + TypeInfo + 'static>(
	idx: u32,
	at: Hash,
	api: &SubxtClient,
) -> Result<Option<SignedSubmission<S>>, Error> {
	let scale_idx = Value::u128(idx as u128);
	let addr = subxt::dynamic::storage(EPM_PALLET_NAME, "SignedSubmissionsMap", vec![scale_idx]);

	match api.storage().fetch(&addr, Some(at)).await {
		Ok(Some(val)) => {
			let val = val.remove_context();
			let v = encode_scale_value::<SignedSubmission<S>>(&val)?;
			let submissions = Decode::decode(&mut v.as_ref())?;
			Ok(Some(submissions))
		},
		Ok(None) => Ok(None),
		Err(err) => Err(err.into()),
	}
}

/// Helper to fetch snapshot data via RPC
/// and compute an NPos solution via [`pallet_election_provider_multi_phase`].
pub async fn fetch_snapshot_and_mine_solution<T>(
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

fn make_type<T: scale_info::TypeInfo + 'static>() -> (TypeId, PortableRegistry) {
	let m = scale_info::MetaType::new::<T>();
	let mut types = scale_info::Registry::new();
	let id = types.register_type(&m);
	let portable_registry: PortableRegistry = types.into();

	(id.into(), portable_registry)
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

fn encode_scale_value<T: TypeInfo + 'static>(val: &Value) -> Result<Vec<u8>, Error> {
	let (ty_id, types) = make_type::<T>();

	let mut bytes = Vec::new();

	encode_as_type(val, ty_id, &types, &mut bytes).map_err(|e| {
		Error::DynamicTransaction(format!(
			"Failed to encode {}: {:?}",
			std::any::type_name::<T>(),
			e
		))
	})?;
	Ok(bytes)
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
