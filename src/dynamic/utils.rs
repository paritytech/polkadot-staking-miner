use super::pallet_api::PalletItem;
use crate::{error::Error, prelude::ExtrinsicParamsBuilder};
use codec::Encode;
use scale_info::PortableRegistry;
use scale_value::scale::decode_as_type;
use subxt::{Metadata, dynamic::Value};

type TypeId = u32;

const RESTRICT_ORIGINS: &str = "RestrictOrigins";

pub fn with_restrict_origins(
	builder: ExtrinsicParamsBuilder,
	metadata: &Metadata,
) -> ExtrinsicParamsBuilder {
	use subxt::ext::scale_encode::EncodeAsType;

	let extrinsic = metadata.extrinsic();
	let newest_version = extrinsic.transaction_extension_version_to_use_for_encoding();
	let types = metadata.types();
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

#[cfg(test)]
mod test {
	use super::*;
	use codec::Decode;
	use subxt::ext::frame_metadata;

	fn metadata() -> frame_metadata::v16::RuntimeMetadataV16 {
		let bytes = std::fs::read("artifacts/multi_block.scale").unwrap();
		let frame_metadata::RuntimeMetadata::V16(metadata) =
			frame_metadata::RuntimeMetadataPrefixed::decode(&mut &*bytes).unwrap().1
		else {
			panic!("artifacts/multi_block.scale is not V16 metadata")
		};
		metadata
	}

	fn without_restrict_origins(
		mut metadata: frame_metadata::v16::RuntimeMetadataV16,
	) -> frame_metadata::v16::RuntimeMetadataV16 {
		for extension in &mut metadata.extrinsic.transaction_extensions {
			if extension.identifier == RESTRICT_ORIGINS {
				extension.identifier = "SomeOtherExtension".to_owned();
			}
		}
		metadata
	}

	fn with_restrict_origins_value_ty(
		mut metadata: frame_metadata::v16::RuntimeMetadataV16,
		ty: impl Fn(&scale_info::TypeDef<scale_info::form::PortableForm>) -> bool,
	) -> frame_metadata::v16::RuntimeMetadataV16 {
		let value_ty = metadata
			.types
			.types
			.iter()
			.find(|ty_| ty(&ty_.ty.type_def))
			.expect("metadata declares the type")
			.id;
		for extension in &mut metadata.extrinsic.transaction_extensions {
			if extension.identifier == RESTRICT_ORIGINS {
				extension.ty = value_ty.into();
			}
		}
		metadata
	}

	fn params_for(metadata: frame_metadata::v16::RuntimeMetadataV16) -> Vec<(String, Value)> {
		let metadata = Metadata::from_v16(metadata).unwrap();
		with_restrict_origins(ExtrinsicParamsBuilder::default(), &metadata)
			.build()
			.custom()
			.to_vec()
	}

	#[test]
	fn adds_restrict_origins_when_the_chain_declares_it() {
		assert_eq!(params_for(metadata()), [(RESTRICT_ORIGINS.to_owned(), Value::bool(true))]);
	}

	#[test]
	fn leaves_params_alone_when_the_chain_does_not_declare_it() {
		assert!(params_for(without_restrict_origins(metadata())).is_empty());
	}

	#[test]
	fn leaves_params_alone_when_the_extension_takes_no_bool() {
		let empty = params_for(with_restrict_origins_value_ty(
			metadata(),
			|def| matches!(def, scale_info::TypeDef::Composite(composite) if composite.fields.is_empty()),
		));
		assert!(empty.is_empty());

		let option = params_for(with_restrict_origins_value_ty(metadata(), |def| {
			matches!(def, scale_info::TypeDef::Variant(variant)
				if variant.variants.iter().any(|v| v.name == "Some"))
		}));
		assert!(option.is_empty());
	}
}
