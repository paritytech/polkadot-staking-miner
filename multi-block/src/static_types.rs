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

impl_atomic_u32_parameter_types!(pages, Pages);
impl_atomic_u32_parameter_types!(target_snapshot_per_block, TargetSnapshotPerBlock);
impl_atomic_u32_parameter_types!(voter_snapshot_per_block, VoterSnapshotPerBlock);
