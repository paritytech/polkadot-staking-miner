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

use crate::{
	error::Error,
	prelude::{ChainClient, Hash, Storage},
};

/// Get the storage API with a specific block hash.
///
/// If block_hash is None, it will use the latest finalized block.
pub async fn storage_at(block_hash: Option<Hash>, api: &ChainClient) -> Result<Storage, Error> {
	match block_hash {
		Some(hash) => Ok(api.storage().at(hash)),
		None => Ok(api.storage().at_latest().await?),
	}
}
