/// Macro to mimic a polkadot-sdk runtime parameter type
///
/// Internally, it uses an AtomicU32 to store the value which
/// can be accessed and set using the get and set APIs
///
/// Example:
/// impl_u32_parameter_type(module_name, Type);
macro_rules! impl_u32_parameter_type {
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

/// Macro to create a BalancingConfig parameter type for the sequential phragmen solver
///
/// Internally, it uses an AtomicUsize to store the iterations value.
/// The tolerance is hardcoded to 0 for now.
///
/// Example:
/// impl_balancing_config_parameter_type(module_name, Type);
macro_rules! impl_balancing_config_parameter_type {
	($mod:ident, $name:ident) => {
		mod $mod {
			use std::sync::atomic::{AtomicUsize, Ordering};
			static ITERATIONS: AtomicUsize = AtomicUsize::new(10);
			pub struct $name;

			impl $name {
				pub fn set(iterations: usize) {
					ITERATIONS.store(iterations, Ordering::SeqCst);
				}
			}

			impl
				polkadot_sdk::frame_support::traits::Get<
					Option<polkadot_sdk::sp_npos_elections::BalancingConfig>,
				> for $name
			{
				fn get() -> Option<polkadot_sdk::sp_npos_elections::BalancingConfig> {
					Some(polkadot_sdk::sp_npos_elections::BalancingConfig {
						iterations: ITERATIONS.load(Ordering::SeqCst),
						tolerance: 0,
					})
				}
			}
		}
		pub use $mod::$name;
	};
}

// A helper to configure to correct MinerConfig depending on chain.
#[allow(unused_macros)]
macro_rules! for_multi_block_runtime {
    ($chain:tt, $($code:tt)*) => {
        match $chain {
            $crate::opt::Chain::Polkadot => {
                #[allow(unused)]
                use $crate::static_types::multi_block::polkadot::MinerConfig;
                $($code)*
            },
            $crate::opt::Chain::Kusama => {
                #[allow(unused)]
                use $crate::static_types::multi_block::kusama::MinerConfig;
                $($code)*
            },
            $crate::opt::Chain::Westend => {
                #[allow(unused)]
                use $crate::static_types::multi_block::westend::MinerConfig;
                $($code)*
            },
            $crate::opt::Chain::StakingAsync => {
                #[allow(unused)]
                use $crate::static_types::multi_block::staking_async::MinerConfig;
                $($code)*
            }
            $crate::opt::Chain::SubstrateNode => {
                #[allow(unused)]
                use $crate::static_types::multi_block::node::MinerConfig;
                $($code)*
            }
        }
    };
}

/// Macro to mimic a polkadot-sdk runtime parameter type for ElectionAlgorithm
macro_rules! impl_algorithm_parameter_type {
	($mod:ident, $name:ident) => {
		mod $mod {
			use crate::commands::types::ElectionAlgorithm;
			use std::sync::atomic::{AtomicU8, Ordering};
			static VAL: AtomicU8 = AtomicU8::new(0); // 0 = SeqPhragmen, 1 = Phragmms
			pub struct $name;

			impl $name {
				pub fn get() -> ElectionAlgorithm {
					match VAL.load(Ordering::SeqCst) {
						0 => ElectionAlgorithm::SeqPhragmen,
						1 => ElectionAlgorithm::Phragmms,
						_ => unreachable!(),
					}
				}
				pub fn set(val: ElectionAlgorithm) {
					let v = match val {
						ElectionAlgorithm::SeqPhragmen => 0,
						ElectionAlgorithm::Phragmms => 1,
					};
					VAL.store(v, Ordering::SeqCst);
				}
			}
		}
		pub use $mod::$name;
	};
}

#[allow(unused)]
pub(crate) use {
	for_multi_block_runtime, impl_algorithm_parameter_type, impl_balancing_config_parameter_type,
	impl_u32_parameter_type,
};
