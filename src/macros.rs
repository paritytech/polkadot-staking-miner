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

macro_rules! cfg_feature {
	($feature:literal, $($item:item)*) => {
		$(
			#[cfg(feature = $feature)]
			#[cfg_attr(docsrs, doc(cfg(feature = $feature)))]
			$item
		)*
	}
}

/// Macro to conditionally include items based on the `legacy` feature.
macro_rules! cfg_legacy {
	($($item:item)*) => {
		crate::macros::cfg_feature!("legacy", $($item)*);
	};
}

/// Macro to conditionally include items based on the `experimental-multi-block` feature.
macro_rules! cfg_experimental_multi_block {
    ($($item:item)*) => {
        crate::macros::cfg_feature!("experimental-multi-block", $($item)*);
    };
}

pub(crate) use {cfg_experimental_multi_block, cfg_feature, cfg_legacy, impl_u32_parameter_type};
