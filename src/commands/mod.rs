//! Supported commands for the polkadot-staking-miner and related types.

use crate::macros::{cfg_experimental_multi_block, cfg_legacy};

mod types;

cfg_legacy! {
    mod legacy;
    pub use legacy::*;
}

cfg_experimental_multi_block! {
    mod multi_block;
    pub use multi_block::*;
}

pub use types::*;
