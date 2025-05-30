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

// A helper to configure to correct MinerConfig depending on chain.
#[allow(unused_macros)]
macro_rules! for_legacy_runtime {
    ($chain:tt, $($code:tt)*) => {
        match $chain {
            $crate::opt::Chain::Polkadot => {
                #[allow(unused)]
                use $crate::static_types::legacy::polkadot::MinerConfig;
                $($code)*
            },
            $crate::opt::Chain::Kusama => {
                #[allow(unused)]
                use $crate::static_types::legacy::kusama::MinerConfig;
                $($code)*
            },
            $crate::opt::Chain::Westend => {
                #[allow(unused)]
                use $crate::static_types::legacy::westend::MinerConfig;
                $($code)*
            },
            $crate::opt::Chain::StakingAsync => {
                #[allow(unused)]
                use $crate::static_types::legacy::staking_async::MinerConfig;
                $($code)*
            }
            $crate::opt::Chain::SubstrateNode => {
                #[allow(unused)]
                use $crate::static_types::legacy::node::MinerConfig;
                $($code)*
            }
        }
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

#[allow(unused)]
pub(crate) use {for_legacy_runtime, for_multi_block_runtime, impl_u32_parameter_type};
