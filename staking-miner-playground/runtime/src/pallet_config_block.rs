//! A simple pallet that makes it possible to configure the block length and block weight.

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

use frame_support::{
	dispatch::DispatchResult,
	weights::{constants::WEIGHT_REF_TIME_PER_SECOND, Weight},
};
use sp_std::marker::PhantomData;

pub use pallet::*;

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use frame_support::pallet_prelude::{ValueQuery, *};
	use frame_system::pallet_prelude::*;

	#[pallet::config]
	pub trait Config: frame_system::Config {
		type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
	}

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		SetBlockLength(u32),
		SetBlockWeight(u64),
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn on_initialize(_n: T::BlockNumber) -> Weight {
			Weight::zero()
		}
		fn on_finalize(_n: T::BlockNumber) {}
		fn offchain_worker(_n: T::BlockNumber) {}
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		#[pallet::weight(0)]
		#[pallet::call_index(0)]
		pub fn set_block_length(_origin: OriginFor<T>, length: u32) -> DispatchResult {
			frame_support::log::trace!("ConfigBlock::set_block_length: {length}");
			BlockLength::<T>::put(length);
			Self::deposit_event(Event::SetBlockLength(length));
			Ok(())
		}

		#[pallet::weight(0)]
		#[pallet::call_index(1)]
		pub fn set_block_weight(_origin: OriginFor<T>, weight: u64) -> DispatchResult {
			frame_support::log::trace!("ConfigBlock::set_block_weight: {weight}");
			BlockWeight::<T>::put(weight);
			Self::deposit_event(Event::SetBlockWeight(weight));
			Ok(())
		}
	}

	#[pallet::type_value]
	pub fn DefaultBlockLength() -> u32 {
		4 * 1024
	}

	#[pallet::type_value]
	pub fn DefaultBlockWeight() -> u64 {
		WEIGHT_REF_TIME_PER_SECOND / 100
	}

	#[pallet::storage]
	#[pallet::whitelist_storage]
	#[pallet::getter(fn block_length)]
	pub(super) type BlockLength<T: Config> = StorageValue<_, u32, ValueQuery, DefaultBlockLength>;

	#[pallet::storage]
	#[pallet::getter(fn block_weight)]
	pub(super) type BlockWeight<T: Config> = StorageValue<_, u64, ValueQuery, DefaultBlockWeight>;

	#[pallet::genesis_config]
	pub struct GenesisConfig<T> {
		pub block_weight: u64,
		pub block_length: u32,
		_marker: PhantomData<T>,
	}

	#[cfg(feature = "std")]
	impl<T> Default for GenesisConfig<T> {
		fn default() -> Self {
			Self {
				block_weight: DefaultBlockWeight::get(),
				block_length: DefaultBlockLength::get(),
				_marker: PhantomData,
			}
		}
	}

	#[pallet::genesis_build]
	impl<T: Config> GenesisBuild<T> for GenesisConfig<T> {
		fn build(&self) {
			<BlockLength<T>>::put(self.block_length);
			<BlockWeight<T>>::put(self.block_weight);
		}
	}
}
