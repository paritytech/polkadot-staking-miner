//! This module contains the static types or may be hardcoded types for supported runtimes
//! which can't be fetched or instantiated from the metadata or similar.
//!
//! These relies that configuration is corresponding to to the runtime and are error-prone
//! if something changes in the runtime, these needs to be updated manually.
//!
//! In most cases, these will be detected by the integration tests.

#[cfg(legacy)]
mod legacy;
#[cfg(legacy)]
pub use legacy::*;
#[cfg(experimental_multi_block)]
mod multi_block;
#[cfg(experimental_multi_block)]
pub use multi_block::*;

#[macro_export]
macro_rules! impl_atomic_u32_parameter_types {
	($mod:ident, $name:ident) => {
		mod $mod {
			use std::sync::atomic::{AtomicU32, Ordering};
			static VAL: AtomicU32 = AtomicU32::new(0);
			pub struct $name;

			impl $name {
				pub fn get() -> u32 {
					VAL.load(Ordering::SeqCst)
				}
			}
			impl<I: From<u32>> polkadot_sdk::frame_support::traits::Get<I> for $name {
				fn get() -> I {
					I::from(Self::get())
				}
			}

			impl $name {
				pub fn set(val: u32) {
					VAL.store(val, std::sync::atomic::Ordering::SeqCst);
				}
			}
		}
		pub use $mod::$name;
	};
}
