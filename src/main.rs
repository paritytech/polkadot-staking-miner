// Copyright 2021 Parity Technologies (UK) Ltd.
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

//! # Polkadot Staking Miner.
//!
//! Simple bot capable of monitoring a polkadot (and cousins) chain and submitting solutions to the
//! `pallet-election-provider-multi-phase`. See `--help` for more details.
//!
//! # Implementation Notes:
//!
//! - First draft: Be aware that this is the first draft and there might be bugs, or undefined
//!   behaviors. Don't attach this bot to an account with lots of funds.
//! - Quick to crash: The bot is written so that it only continues to work if everything goes well.
//!   In case of any failure (RPC, logic, IO), it will crash. This was a decision to simplify the
//!   development. It is intended to run this bot with a `restart = true` way, so that it reports it
//!   crash, but resumes work thereafter.

mod chain;
mod dry_run;
mod emergency_solution;
mod error;
mod helpers;
mod monitor;
mod opt;
mod prelude;
mod signer;

use clap::Parser;
use jsonrpsee::ws_client::WsClientBuilder;
use opt::Command;
use prelude::*;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Error> {
	tracing_subscriber::fmt().with_env_filter(EnvFilter::from_default_env()).init();

	let Opt { uri, seed_or_path, command } = Opt::parse();
	log::debug!(target: LOG_TARGET, "attempting to connect to {:?}", uri);

	let rpc = WsClientBuilder::default().max_request_body_size(u32::MAX).build(uri).await?;

	let client = subxt::ClientBuilder::new().set_client(rpc).build().await?;
	let runtime_version = client.rpc().runtime_version(None).await?;
	let pair_signer = signer::signer_pair_from_string(&seed_or_path)?;
	let chain = Chain::try_from(runtime_version)?;

	log::info!(target: LOG_TARGET, "Connected to chain: {}", chain);

	let outcome = any_runtime!(chain, {
		let api: RuntimeApi = client.to_runtime_api();

		tls_update_runtime_constants(&api);

		// Start a new tokio task to perform the runtime updates in the background.
		let update_client = api.client.updates();
		tokio::spawn(async move {
			let result = update_client.perform_runtime_updates().await;
			log::error!(target: LOG_TARGET, "Runtime update failed with result: {:?}", result);
		});

		match command {
			Command::Monitor(cfg) => monitor_cmd(api, cfg, pair_signer).await,
			Command::DryRun(cfg) => dry_run_cmd(api, cfg, pair_signer).await,
			Command::EmergencySolution(cfg) => emergency_cmd(api, cfg, pair_signer).await,
		}
	});

	log::info!(target: LOG_TARGET, "round of execution finished. outcome = {:?}", outcome);
	outcome
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn cli_monitor_works() {
		let opt = Opt::try_parse_from([
			env!("CARGO_PKG_NAME"),
			"--uri",
			"hi",
			"--seed-or-path",
			"//Alice",
			"monitor",
			"--listen",
			"head",
			"seq-phragmen",
		])
		.unwrap();

		assert_eq!(
			opt,
			Opt {
				uri: "hi".to_string(),
				seed_or_path: "//Alice".to_string(),
				command: Command::Monitor(MonitorConfig {
					listen: "head".to_string(),
					solver: Solver::SeqPhragmen { iterations: 10 },
					submission_strategy: SubmissionStrategy::IfLeading,
				}),
			}
		);
	}

	#[test]
	fn cli_dry_run_works() {
		let opt = Opt::try_parse_from([
			env!("CARGO_PKG_NAME"),
			"--uri",
			"hi",
			"--seed-or-path",
			"//Alice",
			"dry-run",
			"phrag-mms",
		])
		.unwrap();

		assert_eq!(
			opt,
			Opt {
				uri: "hi".to_string(),
				seed_or_path: "//Alice".to_string(),
				command: Command::DryRun(DryRunConfig {
					at: None,
					solver: Solver::PhragMMS { iterations: 10 },
					force_snapshot: false,
				}),
			}
		);
	}

	#[test]
	fn cli_emergency_works() {
		let opt = Opt::try_parse_from([
			env!("CARGO_PKG_NAME"),
			"--uri",
			"hi",
			"--seed-or-path",
			"//Alice",
			"emergency-solution",
			"99",
			"phrag-mms",
			"--iterations",
			"1337",
		])
		.unwrap();

		assert_eq!(
			opt,
			Opt {
				uri: "hi".to_string(),
				seed_or_path: "//Alice".to_string(),
				command: Command::EmergencySolution(EmergencySolutionConfig {
					take: Some(99),
					at: None,
					solver: Solver::PhragMMS { iterations: 1337 }
				}),
			}
		);
	}

	#[test]
	fn submission_strategy_from_str_works() {
		use std::str::FromStr;

		assert_eq!(SubmissionStrategy::from_str("if-leading"), Ok(SubmissionStrategy::IfLeading));
		assert_eq!(SubmissionStrategy::from_str("always"), Ok(SubmissionStrategy::Always));
		assert_eq!(
			SubmissionStrategy::from_str("  percent-better 99   "),
			Ok(SubmissionStrategy::ClaimBetterThan(Accuracy::from_percent(99)))
		);
	}
}
