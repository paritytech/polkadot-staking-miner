use crate::{error::Error, prelude::*};
use sp_core::Pair as _;

// A signer type, parameterized for using with `subxt`.
pub type PairSigner = subxt::tx::PairSigner<subxt::PolkadotConfig, sp_core::sr25519::Pair>;

// Signer wrapper.
//
// NOTE: both `Pair` and `PairSigner` are stored here so it can be cloned
// which is hack around that PairSigner !Clone.
pub struct Signer {
	pair: Pair,
	signer: PairSigner,
}

impl std::fmt::Display for Signer {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "{}", self.signer.account_id())
	}
}

impl Clone for Signer {
	fn clone(&self) -> Self {
		Self { pair: self.pair.clone(), signer: PairSigner::new(self.pair.clone()) }
	}
}

impl Signer {
	pub fn new(mut seed_or_path: &str) -> Result<Self, Error> {
		seed_or_path = seed_or_path.trim();

		let seed = match std::fs::read(seed_or_path) {
			Ok(s) => String::from_utf8(s).map_err(|e| Error::Other(e.to_string()))?,
			Err(_) => seed_or_path.to_string(),
		};

		let seed = seed.trim();
		let pair = Pair::from_string(seed, None).map_err(Error::Crypto)?;
		let signer = PairSigner::new(pair.clone());

		Ok(Self { pair, signer })
	}
}

impl std::ops::Deref for Signer {
	type Target = PairSigner;

	fn deref(&self) -> &Self::Target {
		&self.signer
	}
}

impl std::ops::DerefMut for Signer {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.signer
	}
}
