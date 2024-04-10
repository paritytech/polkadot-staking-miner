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

mod client;
mod epm;
mod error;
mod prelude;

use clap::Parser;
use error::Error;
use futures::future::{BoxFuture, FutureExt};
use prelude::*;
use std::str::FromStr;
use subxt::client::RuntimeVersion;
use tokio::sync::oneshot;
use tracing_subscriber::EnvFilter;

use crate::client::Client;

#[tokio::main]
async fn main() -> Result<(), Error> {
	tracing_subscriber::fmt().init();

	let client = Client::new("ws://127.0.0.1:9944").await?;

	let addr = runtime::storage().on_demand_assignment_provider().free_entries();
	let x = client.chain_api().storage().at_latest().await?.fetch(&addr).await?;

	println!("{:?}", x);

	Ok(())
}

#[cfg(target_family = "unix")]
async fn run_command(
	fut: BoxFuture<'_, Result<(), Error>>,
	rx_upgrade: oneshot::Receiver<Error>,
) -> Result<(), Error> {
	use tokio::signal::unix::{signal, SignalKind};

	let mut stream_int = signal(SignalKind::interrupt()).map_err(Error::Io)?;
	let mut stream_term = signal(SignalKind::terminate()).map_err(Error::Io)?;

	tokio::select! {
		_ = stream_int.recv() => {
			Ok(())
		}
		_ = stream_term.recv() => {
			Ok(())
		}
		res = rx_upgrade => {
			match res {
				Ok(err) => Err(err),
				Err(_) => unreachable!("A message is sent before the upgrade task is closed; qed"),
			}
		},
		res = fut => res,
	}
}

#[cfg(not(unix))]
async fn run_command(
	fut: BoxFuture<'_, Result<(), Error>>,
	rx_upgrade: oneshot::Receiver<Error>,
) -> Result<(), Error> {
	use tokio::signal::ctrl_c;
	select! {
		_ = ctrl_c() => {},
		res = rx_upgrade => {
			match res {
				Ok(err) => Err(err),
				Err(_) => unreachable!("A message is sent before the upgrade task is closed; qed"),
			}
		},
		res = fut => res,
	}
}

/*/// Runs until the RPC connection fails or updating the metadata failed.
async fn runtime_upgrade_task(client: ChainClient, tx: oneshot::Sender<Error>) {
	let updater = client.updater();

	let mut update_stream = match updater.runtime_updates().await {
		Ok(u) => u,
		Err(e) => {
			let _ = tx.send(e.into());
			return
		},
	};

	loop {
		// if the runtime upgrade subscription fails then try establish a new one and if it fails quit.
		let update = match update_stream.next().await {
			Some(Ok(update)) => update,
			_ => {
				log::warn!(target: LOG_TARGET, "Runtime upgrade subscription failed");
				update_stream = match updater.runtime_updates().await {
					Ok(u) => u,
					Err(e) => {
						let _ = tx.send(e.into());
						return
					},
				};
				continue
			},
		};

		let version = update.runtime_version().spec_version;
		match updater.apply_update(update) {
			Ok(()) => {
				if let Err(e) = epm::update_metadata_constants(&client) {
					let _ = tx.send(e);
					return
				}
				prometheus::on_runtime_upgrade();
				log::info!(target: LOG_TARGET, "upgrade to version: {} successful", version);
			},
			Err(e) => {
				log::debug!(target: LOG_TARGET, "upgrade to version: {} failed: {:?}", version, e);
			},
		}
	}
}*/

#[cfg(test)]
mod tests {
	use super::*;
	use commands::monitor;

	#[test]
	fn cli_monitor_works() {
		let opt = Opt::try_parse_from([
			env!("CARGO_PKG_NAME"),
			"--uri",
			"hi",
			"--prometheus-port",
			"9999",
			"monitor",
			"--seed-or-path",
			"//Alice",
			"--listen",
			"head",
			"--delay",
			"12",
			"seq-phragmen",
		])
		.unwrap();

		assert_eq!(
			opt,
			Opt {
				uri: "hi".to_string(),
				prometheus_port: 9999,
				log: "info".to_string(),
				command: Command::Monitor(commands::MonitorConfig {
					listen: monitor::Listen::Head,
					solver: opt::Solver::SeqPhragmen { iterations: 10 },
					submission_strategy: monitor::SubmissionStrategy::IfLeading,
					seed_or_path: "//Alice".to_string(),
					delay: 12,
					dry_run: false,
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
			"dry-run",
			"--seed-or-path",
			"//Alice",
			"phrag-mms",
		])
		.unwrap();

		assert_eq!(
			opt,
			Opt {
				uri: "hi".to_string(),
				prometheus_port: 9999,
				log: "info".to_string(),
				command: Command::DryRun(commands::DryRunConfig {
					at: None,
					solver: opt::Solver::PhragMMS { iterations: 10 },
					force_snapshot: false,
					force_winner_count: None,
					seed_or_path: Some("//Alice".to_string()),
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
				prometheus_port: 9999,
				log: "info".to_string(),
				command: Command::EmergencySolution(commands::EmergencySolutionConfig {
					at: None,
					force_winner_count: Some(99),
					solver: opt::Solver::PhragMMS { iterations: 1337 },
				}),
			}
		);
	}
}
