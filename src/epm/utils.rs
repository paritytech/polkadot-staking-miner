use super::pallet_api::{self, PalletItem};
use crate::{
	error::Error,
	prelude::{runtime, Config},
};
use codec::Encode;
use polkadot_sdk::{
	pallet_election_provider_multi_block::unsigned::miner::MinerConfig,
	sp_npos_elections::ElectionScore,
};
use scale_info::PortableRegistry;
use scale_value::scale::decode_as_type;
use subxt::{blocks::ExtrinsicEvents, dynamic::Value, tx::DynamicPayload};

type TypeId = u32;

pub fn invalid_metadata_error<E: std::error::Error>(item: String, err: E) -> Error {
	Error::InvalidMetadata(format!("{} failed: {}", item, err))
}

pub fn make_type<T: scale_info::TypeInfo + 'static>() -> (TypeId, PortableRegistry) {
	let m = scale_info::MetaType::new::<T>();
	let mut types = scale_info::Registry::new();
	let id = types.register_type(&m);
	let portable_registry: PortableRegistry = types.into();

	(id.id, portable_registry)
}

pub fn to_scale_value<T: scale_info::TypeInfo + 'static + Encode>(val: T) -> Result<Value, Error> {
	let (ty_id, types) = make_type::<T>();
	let bytes = val.encode();

	decode_as_type(&mut bytes.as_ref(), ty_id, &types)
		.map(|v| v.remove_context())
		.map_err(|e| dynamic_decode_error::<T>(e))
}

pub fn storage_addr<P: subxt::storage::StorageKey>(
	storage: PalletItem,
	params: P,
) -> subxt::storage::DynamicAddress<P> {
	let (pallet, variant) = storage.to_parts();
	subxt::dynamic::storage(pallet, variant, params)
}

pub fn tx(
	tx: PalletItem,
	call_data: impl Into<scale_value::Composite<()>>,
) -> subxt::tx::DynamicPayload {
	let (pallet, variant) = tx.to_parts();
	subxt::dynamic::tx(pallet, variant, call_data)
}

pub fn dynamic_decode_error<T>(err: impl std::error::Error) -> Error {
	Error::DynamicTransaction(format!("Failed to decode {}: {:?}", std::any::type_name::<T>(), err))
}

/// A multi-block transaction.
pub enum TransactionKind {
	RegisterScore,
	SubmitPage,
}

impl TransactionKind {
	pub fn as_str(&self) -> &'static str {
		match self {
			Self::RegisterScore => "register score",
			Self::SubmitPage => "submit page",
		}
	}

	/// Check if the transaction is in the given events.
	pub fn in_events(&self, evs: &ExtrinsicEvents<Config>) -> Result<bool, Error> {
		match self {
			Self::RegisterScore =>
				evs.has::<runtime::multi_block_signed::events::Registered>().map_err(Into::into),
			Self::SubmitPage =>
				evs.has::<runtime::multi_block_signed::events::Stored>().map_err(Into::into),
		}
	}
}

pub struct MultiBlockTransaction {
	kind: TransactionKind,
	tx: DynamicPayload,
}

impl MultiBlockTransaction {
	/// Create a transaction to register a score.
	pub fn register_score(score: ElectionScore) -> Result<Self, Error> {
		let scale_score =
			to_scale_value(score).map_err(|err| dynamic_decode_error::<ElectionScore>(err))?;

		Ok(Self {
			kind: TransactionKind::RegisterScore,
			tx: tx(pallet_api::multi_block_signed::tx::REGISTER, vec![scale_score]),
		})
	}

	/// Create a new transaction to submit a page.
	pub fn submit_page<T: MinerConfig + 'static>(
		page: u32,
		maybe_solution: Option<T::Solution>,
	) -> Result<Self, Error> {
		let scale_page =
			to_scale_value(page).map_err(|err| dynamic_decode_error::<T::Pages>(err))?;
		let scale_solution = to_scale_value(maybe_solution)
			.map_err(|err| dynamic_decode_error::<T::Solution>(err))?;

		Ok(Self {
			kind: TransactionKind::SubmitPage,
			tx: tx(
				pallet_api::multi_block_signed::tx::SUBMIT_PAGE,
				vec![scale_page, scale_solution],
			),
		})
	}

	pub fn to_parts(self) -> (TransactionKind, DynamicPayload) {
		(self.kind, self.tx)
	}
}
