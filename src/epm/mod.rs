//! This module contains helper functions, utils to decode storage and APIs to
//! interact with election system and related pallets in polkadot-sdk.

#[cfg(legacy)]
mod legacy;
#[cfg(experimental_multi_block)]
mod multi_block;
#[allow(dead_code, unused)]
pub(crate) mod pallet_api;
pub(crate) mod utils;

#[cfg(legacy)]
pub(crate) use legacy::*;
#[cfg(experimental_multi_block)]
pub(crate) use multi_block::*;
