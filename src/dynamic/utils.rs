use super::pallet_api::PalletItem;
use crate::{error::Error, prelude::ExtrinsicParamsBuilder};
use codec::Encode;
use scale_info::PortableRegistry;
use scale_value::scale::decode_as_type;
use subxt::{Metadata, dynamic::Value};

type TypeId = u32;

const RESTRICT_ORIGINS: &str = "RestrictOrigins";

/// Supply the `RestrictOrigins` value of `pallet-origin-restriction`, which subxt has no typed
/// support for, on the chains that declare it.
///
/// Conditional because `custom_extension` rejects a name the runtime does not declare, and the
/// miner signs on Asset Hubs both with and without the extension. Every version up to the encoding
/// one is probed, mirroring what subxt accepts, since encoding may pick any declared version.
///
/// Declaring it is not enough: `frame-decode` skips empty-valued extensions and defaults
/// `Option<_>` ones by itself, so supplying a value for those shapes fails at signing where plain
/// subxt succeeds. Hence the encode probe rather than a name match.
///
/// Always `true`: `false` asserts "no restriction check needed", and a restricted origin sending
/// that is rejected with `InvalidTransaction::Call`.
///
/// NOTE: still a stopgap, the name and the value are hardcoded here, and would go away if subxt
/// ever grows typed support for the extension. `custom_extension` also cannot supply an
/// authorization extension (see https://github.com/paritytech/subxt/issues/2276), which
/// `RestrictOrigins` is not today.
pub fn with_restrict_origins(
	builder: ExtrinsicParamsBuilder,
	metadata: &Metadata,
) -> ExtrinsicParamsBuilder {
	use subxt::ext::scale_encode::EncodeAsType;

	let extrinsic = metadata.extrinsic();
	let newest_version = extrinsic.transaction_extension_version_to_use_for_encoding();
	let types = metadata.types();
	// Only the value type is probed: subxt rejects a non-empty implicit itself, with a clear error.
	let takes_a_bool = (0..=newest_version)
		.filter_map(|version| extrinsic.transaction_extensions_by_version(version))
		.flatten()
		.filter(|extension| extension.identifier() == RESTRICT_ORIGINS)
		.any(|extension| {
			Value::bool(true)
				.encode_as_type_to(extension.extra_ty(), types, &mut Vec::new())
				.is_ok()
		});

	if takes_a_bool { builder.custom_extension(RESTRICT_ORIGINS, true) } else { builder }
}

pub fn invalid_metadata_error<E: std::error::Error>(item: String, err: E) -> Error {
	Error::InvalidMetadata(format!("{item} failed: {err}"))
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
		.map_err(|e| decode_error::<T>(e))
}

/// Create a dynamic storage address (without keys baked in).
/// Keys are passed separately to `fetch`/`try_fetch`/`iter`.
pub fn storage_addr(storage: PalletItem) -> subxt::storage::DynamicAddress {
	let (pallet, variant) = storage.into_parts();
	subxt::dynamic::storage(pallet, variant)
}

pub fn tx(
	tx: PalletItem,
	call_data: impl Into<scale_value::Composite<()>>,
) -> subxt::tx::DynamicPayload<scale_value::Composite<()>> {
	let (pallet, variant) = tx.into_parts();
	subxt::dynamic::tx(pallet, variant, call_data.into())
}

pub fn decode_error<T>(err: impl std::error::Error) -> Error {
	Error::DynamicTransaction(format!("Failed to decode {}: {:?}", std::any::type_name::<T>(), err))
}
