//! Dynamic utilities.
//! This is a minimal version for the dummy miner.

use crate::error::Error;

pub fn invalid_metadata_error<E: std::error::Error>(item: String, err: E) -> Error {
	Error::InvalidMetadata(format!("{item} failed: {err}"))
}
