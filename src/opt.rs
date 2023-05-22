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

use crate::{error::Error, prelude::Hash};

use clap::*;
use sp_npos_elections::BalancingConfig;
use std::{fmt, str::FromStr};

/// The type of solver to use.
// A common option across multiple commands.
#[derive(Debug, Clone, Parser)]
#[cfg_attr(test, derive(PartialEq))]
pub enum Solver {
	SeqPhragmen {
		#[clap(long, default_value = "10")]
		iterations: usize,
	},
	PhragMMS {
		#[clap(long, default_value = "10")]
		iterations: usize,
	},
}

frame_support::parameter_types! {
	/// Number of balancing iterations for a solution algorithm. Set based on the [`Solvers`] CLI
	/// config.
	pub static BalanceIterations: usize = 10;
	pub static Balancing: Option<BalancingConfig> = Some( BalancingConfig { iterations: BalanceIterations::get(), tolerance: 0 } );
}

/// The chain being used.
#[derive(Debug, Copy, Clone)]
pub enum Chain {
	Westend,
	Kusama,
	Polkadot,
}

impl fmt::Display for Chain {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let chain = match self {
			Self::Polkadot => "polkadot",
			Self::Kusama => "kusama",
			Self::Westend => "westend",
		};
		write!(f, "{}", chain)
	}
}

impl std::str::FromStr for Chain {
	type Err = Error;

	fn from_str(s: &str) -> Result<Self, Error> {
		match s {
			"polkadot" => Ok(Self::Polkadot),
			"kusama" => Ok(Self::Kusama),
			"westend" => Ok(Self::Westend),
			chain => Err(Error::InvalidChain(chain.to_string())),
		}
	}
}

impl TryFrom<subxt::rpc::types::RuntimeVersion> for Chain {
	type Error = Error;

	fn try_from(rv: subxt::rpc::types::RuntimeVersion) -> Result<Self, Error> {
		let json = rv
			.other
			.get("specName")
			.expect("RuntimeVersion must have specName; qed")
			.clone();
		let mut chain =
			serde_json::from_value::<String>(json).expect("specName must be String; qed");
		chain.make_ascii_lowercase();
		Chain::from_str(&chain)
	}
}

/// Represents a block hash which may be used to query chain at given state.
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum BlockHash {
	/// Use the latest block.
	Latest,
	/// Query the chain at the specified `block_hash`.
	At(Hash),
}

impl FromStr for BlockHash {
	type Err = String;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		let block = Hash::from_str(s).map_err(|e| e.to_string())?;
		Ok(Self::At(block))
	}
}

impl fmt::Display for BlockHash {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::Latest => write!(f, "Block hash is latest"),
			Self::At(h) => write!(f, "Block hash is `0x{}`", h),
		}
	}
}

impl From<BlockHash> for Option<Hash> {
	fn from(b: BlockHash) -> Option<Hash> {
		match b {
			BlockHash::At(h) => Some(h),
			BlockHash::Latest => None,
		}
	}
}
