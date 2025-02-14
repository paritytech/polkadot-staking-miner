// Copyright 2021-2022 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

//! Types that we don't fetch from a particular runtime and just assume that they are constant all
//! of the place.
//!
//! It is actually easy to convert the rest as well, but it'll be a lot of noise in our codebase,
//! needing to sprinkle `any_runtime` in a few extra places.

/// Signed submission type.
pub type SignedSubmission<S> = polkadot_sdk::pallet_election_provider_multi_phase::SignedSubmission<
    super::AccountId,
    Balance,
    S,
>;

/// Balance type.
pub type Balance = u128;

#[subxt::subxt(
    runtime_metadata_path = "artifacts/metadata.scale",
    derive_for_all_types = "Clone, Debug, Eq, PartialEq",
    substitute_type(
        path = "sp_npos_elections::ElectionScore",
        with = "::subxt::utils::Static<polkadot_sdk::sp_npos_elections::ElectionScore>"
    ),
    substitute_type(
        path = "pallet_election_provider_multi_phase::Phase<Bn>",
        with = "::subxt::utils::Static<polkadot_sdk::pallet_election_provider_multi_phase::Phase<Bn>>"
    )
)]
pub mod runtime {}
