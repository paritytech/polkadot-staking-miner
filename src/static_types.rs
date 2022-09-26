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

			impl<I: From<u32>> frame_support::traits::Get<I> for $name {
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

mod max_weight {
	use frame_support::weights::Weight;
	use std::sync::atomic::{AtomicU64, Ordering};

	static VAL: AtomicU64 = AtomicU64::new(0);
	pub struct MaxWeight;

	impl MaxWeight {
		pub fn get() -> Weight {
			Weight::from_ref_time(VAL.load(Ordering::SeqCst))
		}
	}

	impl frame_support::traits::Get<Weight> for MaxWeight {
		fn get() -> Weight {
			Self::get()
		}
	}

	impl MaxWeight {
		pub fn set(weight: Weight) {
			VAL.store(weight.ref_time(), std::sync::atomic::Ordering::SeqCst);
		}
	}
}

impl_atomic_u32_parameter_types!(max_length, MaxLength);
impl_atomic_u32_parameter_types!(max_votes, MaxVotesPerVoter);
pub use max_weight::MaxWeight;
