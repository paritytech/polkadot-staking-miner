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
    pub const fn to_parts(&self) -> (&'static str, &'static str) {
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

    pub const fn to_parts(&self) -> (&'static str, &'static str) {
        self.inner.to_parts()
    }

    pub fn fetch(&self, api: &ChainClient) -> Result<T, Error> {
        let (pallet, variant) = self.inner.to_parts();

        let val = api
            .constants()
            .at(&subxt::dynamic::constant(pallet, variant))
            .map_err(|e| invalid_metadata_error(variant.to_string(), e))?
            .to_value()
            .map_err(|e| Error::Subxt(e.into()))?;

        let val = scale_value::serde::from_value::<_, T>(val).map_err(|e| {
            Error::InvalidMetadata(format!(
                "Decoding `{}` failed {}",
                std::any::type_name::<T>(),
                e
            ))
        })?;

        log::trace!(target: LOG_TARGET, "updating metadata constant `{}`: {val}", self.inner);
        Ok(val)
    }
}

pub mod multi_block {
    pub const NAME: &str = "MultiBlock";

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

pub mod election_provider_multi_phase {
    pub const NAME: &str = "ElectionProviderMultiPhase";

    pub mod constants {
        use super::{super::*, *};
        use polkadot_sdk::frame_support::weights::Weight;

        pub const MAX_VOTES_PER_VOTER: PalletConstant<u32> =
            PalletConstant::new(NAME, "MinerMaxVotesPerVoter");
        pub const MAX_LENGTH: PalletConstant<u32> = PalletConstant::new(NAME, "MinerMaxLength");
        pub const MAX_BACKERS_PER_WINNER: PalletConstant<u32> =
            PalletConstant::new(NAME, "MaxBackersPerWinner");
        pub const MAX_WEIGHT: PalletConstant<Weight> = PalletConstant::new(NAME, "SignedMaxWeight");
        // NOTE: `MaxWinners` is used instead of `MinerMaxWinners` to work with older metadata.
        pub const MAX_WINNERS: PalletConstant<u32> = PalletConstant::new(NAME, "MaxWinners");
    }

    pub mod storage {
        use super::{super::*, *};
        pub const SIGNED_SUBMISSIONS_MAP: PalletItem =
            PalletItem::new(NAME, "SignedSubmissionsMap");
        pub const SNAPSHOT: PalletItem = PalletItem::new(NAME, "Snapshot");
    }

    pub mod tx {
        use super::{super::*, *};
        pub const SUBMIT_UNSIGNED: PalletItem = PalletItem::new(NAME, "submit_unsigned");
        pub const SUBMIT: PalletItem = PalletItem::new(NAME, "submit");
        pub const EMERGENCY: PalletItem = PalletItem::new(NAME, "set_emergency_election_result");
    }
}

pub mod multi_block_signed {
    pub const NAME: &str = "MultiBlockSigned";

    pub mod tx {
        use super::{super::*, *};
        pub const SUBMIT_PAGE: PalletItem = PalletItem::new(NAME, "submit_page");
        pub const REGISTER: PalletItem = PalletItem::new(NAME, "register");
    }
}
