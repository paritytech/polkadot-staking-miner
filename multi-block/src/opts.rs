use crate::error::Error;

use serde::{Deserialize, Serialize};
use sp_runtime::DeserializeOwned;
use std::{collections::HashMap, fmt, str::FromStr};

use subxt::backend::legacy::rpc_methods as subxt_rpc;

/// The chain being used.
#[derive(Debug, Copy, Clone)]
pub enum Chain {
	Westend,
	Kusama,
	Polkadot,
	Rococo,
}

impl fmt::Display for Chain {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let chain = match self {
			Self::Polkadot => "polkadot",
			Self::Kusama => "kusama",
			Self::Westend => "westend",
			Self::Rococo => "staking-dev",
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
			"staking-dev" => Ok(Self::Rococo),
			chain => Err(Error::InvalidChain(chain.to_string())),
		}
	}
}

impl TryFrom<subxt_rpc::RuntimeVersion> for Chain {
	type Error = Error;

	fn try_from(rv: subxt_rpc::RuntimeVersion) -> Result<Self, Error> {
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

// This is infallible because all these field must exist on substrate-based chains
// and is based on <https://docs.rs/sp-version/latest/sp_version/struct.RuntimeVersion.html>
impl From<subxt_rpc::RuntimeVersion> for RuntimeVersion {
	fn from(rv: subxt_rpc::RuntimeVersion) -> Self {
		let mut spec_name: String = get_val_unchecked("specName", &rv.other);
		let impl_name: String = get_val_unchecked("implName", &rv.other);
		let impl_version: u32 = get_val_unchecked("implVersion", &rv.other);
		let authoring_version: u32 = get_val_unchecked("authoringVersion", &rv.other);
		let state_version: u8 = get_val_unchecked("stateVersion", &rv.other);

		let spec_version = rv.spec_version;
		let transaction_version = rv.transaction_version;

		spec_name.make_ascii_lowercase();

		Self {
			spec_name,
			impl_name,
			impl_version,
			spec_version,
			transaction_version,
			authoring_version,
			state_version,
		}
	}
}

#[derive(Deserialize, Serialize, PartialEq, Debug, Clone)]
pub struct RuntimeVersion {
	pub spec_name: String,
	pub impl_name: String,
	pub spec_version: u32,
	pub impl_version: u32,
	pub authoring_version: u32,
	pub transaction_version: u32,
	pub state_version: u8,
}

fn get_val_unchecked<T: DeserializeOwned>(val: &str, rv: &HashMap<String, serde_json::Value>) -> T {
	let json = rv.get(val).expect("`{val}` must exist; qed").clone();
	serde_json::from_value::<T>(json).expect("T must be Deserialize; qed")
}
