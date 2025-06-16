use crate::{
    dynamic::utils::invalid_metadata_error,
    error::Error,
    prelude::{ChainClient, LOG_TARGET},
};
use serde::de::DeserializeOwned;
use std::marker::PhantomData;

/// Represent a pallet item which can be a constant, storage or transaction.
#[derive(Copy, Clone, Debug)]
pub struct PalletItem {
    pallet: &'static str,
    variant: &'static str,
}

impl std::fmt::Display for PalletItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("{}::{}", self.pallet, self.variant))
    }
}

impl PalletItem {
    pub const fn new(pallet: &'static str, variant: &'static str) -> Self {
        Self { pallet, variant }
    }
    pub const fn into_parts(self) -> (&'static str, &'static str) {
        (self.pallet, self.variant)
    }
}

/// Represents a constant in a pallet.
#[derive(Copy, Clone, Debug)]
pub struct PalletConstant<T> {
    inner: PalletItem,
    _marker: PhantomData<T>,
}

impl<T: DeserializeOwned + std::fmt::Display> PalletConstant<T> {
    pub const fn new(pallet: &'static str, variant: &'static str) -> Self {
        Self {
            inner: PalletItem::new(pallet, variant),
            _marker: PhantomData,
        }
    }

    pub fn fetch(&self, api: &ChainClient) -> Result<T, Error> {
        let (pallet, variant) = self.inner.into_parts();

        let val = api
            .constants()
            .at(&subxt::dynamic::constant(pallet, variant))
            .map_err(|e| invalid_metadata_error(variant.to_string(), e))?
            .to_value()
            .map_err(|e| Error::Subxt(e.into()))?;

        let val = scale_value::serde::from_value::<_, T>(val).map_err(|e| {
            Error::InvalidMetadata(format!(
                "Decoding constant {pallet}:{variant} type: `{}` couldn't be decoded {}",
                std::any::type_name::<T>(),
                e
            ))
        })?;

        log::trace!(target: LOG_TARGET, "updating metadata constant `{}`: {val}", self.inner);
        Ok(val)
    }
}

pub mod multi_block {
    pub const NAME: &str = "MultiBlockElection";

    pub mod constants {
        use super::{super::*, *};
        pub const PAGES: PalletConstant<u32> = PalletConstant::new(NAME, "Pages");
        pub const TARGET_SNAPSHOT_PER_BLOCK: PalletConstant<u32> =
            PalletConstant::new(NAME, "TargetSnapshotPerBlock");
        pub const VOTER_SNAPSHOT_PER_BLOCK: PalletConstant<u32> =
            PalletConstant::new(NAME, "VoterSnapshotPerBlock");
    }

    pub mod storage {
        use super::{super::*, *};
        pub const PAGED_TARGET_SNAPSHOT: PalletItem = PalletItem::new(NAME, "PagedTargetSnapshot");
        pub const PAGED_TARGET_SNAPSHOT_HASH: PalletItem =
            PalletItem::new(NAME, "PagedTargetSnapshotHash");
        pub const PAGED_VOTER_SNAPSHOT: PalletItem = PalletItem::new(NAME, "PagedVoterSnapshot");
        pub const PAGED_VOTER_SNAPSHOT_HASH: PalletItem =
            PalletItem::new(NAME, "PagedVoterSnapshotHash");
    }
}

pub mod multi_block_verifier {
    pub const NAME: &str = "MultiBlockElectionVerifier";

    pub mod constants {
        use super::{super::*, *};
        pub const MAX_WINNERS_PER_PAGE: PalletConstant<u32> =
            PalletConstant::new(NAME, "MaxWinnersPerPage");
        pub const MAX_BACKERS_PER_WINNER: PalletConstant<u32> =
            PalletConstant::new(NAME, "MaxBackersPerWinner");
    }
}

pub mod multi_block_signed {
    pub const NAME: &str = "MultiBlockElectionSigned";

    pub mod tx {
        use super::{super::*, *};
        pub const SUBMIT_PAGE: PalletItem = PalletItem::new(NAME, "submit_page");
        pub const REGISTER: PalletItem = PalletItem::new(NAME, "register");
    }
}

pub mod system {
    use codec::Decode;
    use serde::Deserialize;

    const NAME: &str = "System";

    /// Simplified version of the `BlockLength` type to get the total block length.
    #[derive(Decode, Deserialize, Debug)]
    pub struct BlockLength {
        max: PerDispatchClass,
    }

    impl BlockLength {
        pub fn total(&self) -> u32 {
            let mut total = 0_u32;
            total = total.saturating_add(self.max.normal);
            total = total.saturating_add(self.max.operational);
            total = total.saturating_add(self.max.mandatory);
            total
        }
    }

    impl std::fmt::Display for BlockLength {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.total())
        }
    }

    #[derive(Decode, Deserialize, Debug)]
    pub struct PerDispatchClass {
        normal: u32,
        operational: u32,
        mandatory: u32,
    }

    pub mod constants {
        use super::{super::*, *};
        pub const BLOCK_LENGTH: PalletConstant<BlockLength> =
            PalletConstant::new(NAME, "BlockLength");
    }
}
