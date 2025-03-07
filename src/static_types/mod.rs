//! This module contains the static types or may be hardcoded types for supported runtimes
//! which can't be fetched or instantiated from the metadata or similar.
//!
//! These relies that configuration is corresponding to to the runtime and are error-prone
//! if something changes in the runtime, these needs to be updated manually.
//!
//! In most cases, these will be detected by the integration tests.

pub mod legacy;
pub mod multi_block;
