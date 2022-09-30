use crate::{prelude::*, static_types};
use codec::{Decode, Encode};
use frame_election_provider_support::NposSolution;
use frame_support::weights::Weight;
use pallet_election_provider_multi_phase::{RawSolution, SolutionOrSnapshotSize};
use pallet_transaction_payment::RuntimeDispatchInfo;
use scale_info::{PortableRegistry, TypeInfo};
use scale_value::scale::{decode_as_type, encode_as_type, TypeId};
use sp_core::Bytes;
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

pub fn signed_solution<S: NposSolution + Encode + TypeInfo + 'static>(
	solution: RawSolution<S>,
) -> Result<DynamicTxPayload<'static>, Error> {
	let scale_solution = to_scale_value(solution).map_err(|e| {
		Error::DynamicTransaction(format!("Failed to decode `RawSolution`: {:?}", e))
	})?;

	Ok(subxt::dynamic::tx(EPM_PALLET_NAME, "submit", vec![scale_solution]))
}

pub fn unsigned_solution<S: NposSolution + Encode + TypeInfo + 'static>(
	solution: RawSolution<S>,
	witness: SolutionOrSnapshotSize,
) -> Result<DynamicTxPayload<'static>, Error> {
	let scale_solution = to_scale_value(solution)?;
	let scale_witness = to_scale_value(witness)?;

	Ok(subxt::dynamic::tx(EPM_PALLET_NAME, "submit_unsigned", vec![scale_solution, scale_witness]))
}

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
///
/// Panics: if the RPC call fails or if decoding the response as a `Weight` fails.
pub fn runtime_api_solution_weight<S: Encode + NposSolution + TypeInfo + 'static>(
	raw_solution: RawSolution<S>,
	witness: SolutionOrSnapshotSize,
) -> Weight {
	let tx =
		unsigned_solution(raw_solution, witness).expect("Failed to create dynamic transaction");

	futures::executor::block_on(async {
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
			.request(
				"state_call",
				rpc_params!["TransactionPaymentCallApi_query_call_info", call_data],
			)
			.await
			.unwrap();

		let info: RuntimeDispatchInfo<Balance> = Decode::decode(&mut bytes.0.as_ref()).unwrap();

		log::trace!(
			target: LOG_TARGET,
			"Received weight of `Solution Extrinsic` from remote node: {:?}",
			info.weight
		);

		info.weight
	})
}
