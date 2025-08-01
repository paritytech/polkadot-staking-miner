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

use crate::error::Error;

use std::{fmt, str::FromStr};

/// The chain being used.
#[derive(Debug, Copy, Clone)]
pub enum Chain {
	Westend,
	Kusama,
	Polkadot,
	SubstrateNode,
	StakingAsync,
}

impl fmt::Display for Chain {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let chain = match self {
			Self::Polkadot => "polkadot",
			Self::Kusama => "kusama",
			Self::Westend => "westend",
			Self::SubstrateNode => "node",
			Self::StakingAsync => "staking-async",
		};
		write!(f, "{chain}")
	}
}

impl std::str::FromStr for Chain {
	type Err = Error;

	fn from_str(s: &str) -> Result<Self, Error> {
		match s {
			"polkadot" => Ok(Self::Polkadot),
			"statemint" => Ok(Self::Polkadot), // Polkadot AH
			"paseo" => Ok(Self::Polkadot),     // Paseo AH
			"kusama" => Ok(Self::Kusama),
			"statemine" => Ok(Self::Kusama), // Kusama AH
			"westend" => Ok(Self::Westend),
			"westmint" => Ok(Self::Westend), // Westend AH
			"staking-async-parachain" => Ok(Self::StakingAsync),
			"staking-async-rc" => {
				unimplemented!("multi-block mining is not supported on relay chains")
			},
			"node" => Ok(Self::SubstrateNode),
			chain => Err(Error::InvalidChain(chain.to_string())),
		}
	}
}

impl TryFrom<&polkadot_sdk::sp_version::RuntimeVersion> for Chain {
	type Error = Error;

	// spec_name is guaranteed to exist on substrate-based chains, but the chain name
	// might not be recognized by our Chain enum implementation
	// (see https://docs.rs/sp-version/latest/sp_version/struct.RuntimeVersion.html)
	fn try_from(rv: &polkadot_sdk::sp_version::RuntimeVersion) -> Result<Self, Error> {
		let mut chain = rv.spec_name.to_string();
		chain.make_ascii_lowercase();
		Chain::from_str(&chain)
	}
}
