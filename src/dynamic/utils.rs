use super::pallet_api::PalletItem;
use crate::error::Error;
use codec::Encode;
use scale_info::PortableRegistry;
use scale_value::scale::decode_as_type;
use subxt::dynamic::Value;

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
        .map_err(|e| decode_error::<T>(e))
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

pub fn decode_error<T>(err: impl std::error::Error) -> Error {
    Error::DynamicTransaction(format!(
        "Failed to decode {}: {:?}",
        std::any::type_name::<T>(),
        err
    ))
}
