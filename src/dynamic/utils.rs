//! Dynamic utilities.

use crate::error::Error;

/// Helper function to create metadata error from subxt error
pub fn invalid_metadata_error(item: String, e: subxt::Error) -> Error {
	Error::InvalidMetadata(format!("Failed to access metadata item {item}: {e}"))
}
