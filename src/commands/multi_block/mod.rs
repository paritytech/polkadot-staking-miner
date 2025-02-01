use super::types::{DryRunConfig, EmergencySolutionConfig};
use crate::{client::Client, error::Error};

mod monitor;

pub async fn emergency_solution_cmd<T>(
	_client: Client,
	_config: EmergencySolutionConfig,
) -> Result<(), Error> {
	todo!("Not supported for multi-block miner yet");
}

pub async fn dry_run_cmd<T>(_client: Client, _config: DryRunConfig) -> Result<(), Error> {
	todo!("Not supported for multi-block miner yet");
}

pub use monitor::*;
