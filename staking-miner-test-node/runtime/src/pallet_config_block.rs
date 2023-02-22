//! A simple pallet that makes it possible to configure the block length and block weight.

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

use frame_support::{
	dispatch::DispatchResult,
	weights::{constants::WEIGHT_PER_SECOND, Weight},
};
use sp_std::marker::PhantomData;

// Re-export pallet items so that they can be accessed from the crate namespace.
pub use pallet::*;

// Definition of the pallet logic, to be aggregated at runtime definition through
// `construct_runtime`.
#[frame_support::pallet]
pub mod pallet {
	// Import various types used to declare pallet in scope.
	use super::*;
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;

	/// Our pallet's configuration trait. All our types and constants go in here. If the
	/// pallet is dependent on specific other pallets, then their configuration traits
	/// should be added to our implied traits list.
	///
	/// `frame_system::Config` should always be included.
	#[pallet::config]
	pub trait Config: frame_system::Config {
		type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
	}

	// Simple declaration of the `Pallet` type. It is placeholder we use to implement traits and
	// method.
	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		SetBlockLength(u32),
		SetBlockWeight(u64),
	}

	// Pallet implements [`Hooks`] trait to define some logic to execute in some context.
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
		pub fn set_block_length(_origin: OriginFor<T>, length: u32) -> DispatchResult {
			frame_support::log::info!("ConfigBlock::set_block_length: {length}");
			BlockLength::<T>::put(length);
			Self::deposit_event(Event::SetBlockLength(length));
			Ok(())
		}

		#[pallet::weight(0)]
		pub fn set_block_weight(_origin: OriginFor<T>, weight: u64) -> DispatchResult {
			frame_support::log::info!("ConfigBlock::set_block_weight: {weight}");
			BlockWeight::<T>::put(weight);
			Self::deposit_event(Event::SetBlockWeight(weight));
			Ok(())
		}
	}

	#[pallet::storage]
	#[pallet::getter(fn block_length)]
	pub(super) type BlockLength<T: Config> = StorageValue<_, u32>;

	#[pallet::storage]
	#[pallet::getter(fn block_weight)]
	pub(super) type BlockWeight<T: Config> = StorageValue<_, u64>;

	// The genesis config type.
	#[pallet::genesis_config]
	pub struct GenesisConfig<T> {
		pub block_weight: u64,
		pub block_length: u32,
		_marker: PhantomData<T>,
	}

	// The default value for the genesis config type.
	#[cfg(feature = "std")]
	impl<T> Default for GenesisConfig<T> {
		fn default() -> Self {
			Self {
				block_weight: (WEIGHT_PER_SECOND / 100).ref_time(),
				block_length: 4 * 1024,
				_marker: PhantomData,
			}
		}
	}

	// The build of genesis for the pallet.
	#[pallet::genesis_build]
	impl<T: Config> GenesisBuild<T> for GenesisConfig<T> {
		fn build(&self) {
			<BlockLength<T>>::put(self.block_length);
			<BlockWeight<T>>::put(self.block_weight);
		}
	}
}
