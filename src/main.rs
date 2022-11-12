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

mod dry_run;
mod emergency_solution;
mod epm;
mod error;
mod helpers;
mod monitor;
mod opt;
mod prelude;
mod prometheus;
mod signer;
mod static_types;

use clap::Parser;
use futures::future::{BoxFuture, FutureExt};
use jsonrpsee::ws_client::WsClientBuilder;
use opt::Command;
use prelude::*;
use std::sync::Arc;
use tokio::sync::oneshot;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Error> {
	tracing_subscriber::fmt().with_env_filter(EnvFilter::from_default_env()).init();

	let Opt { uri, command, prometheus_port } = Opt::parse();
	log::debug!(target: LOG_TARGET, "attempting to connect to {:?}", uri);

	let rpc = loop {
		match WsClientBuilder::default()
			.max_request_body_size(u32::MAX)
			.request_timeout(std::time::Duration::from_secs(600))
			.build(&uri)
			.await
		{
			Ok(rpc) => break rpc,
			Err(e) => {
				log::warn!(
					target: LOG_TARGET,
					"failed to connect to client due to {:?}, retrying soon..",
					e,
				);
			},
		};
		tokio::time::sleep(std::time::Duration::from_millis(2_500)).await;
	};

	let api = SubxtClient::from_rpc_client(Arc::new(rpc)).await?;
	let runtime_version = api.rpc().runtime_version(None).await?;
	let chain = Chain::try_from(runtime_version)?;
	let _prometheus_handle = prometheus::run(prometheus_port.unwrap_or(DEFAULT_PROMETHEUS_PORT))
		.map_err(|e| log::warn!("Failed to start prometheus endpoint: {}", e));
	log::info!(target: LOG_TARGET, "Connected to chain: {}", chain);
	epm::update_metadata_constants(&api).await?;

	SHARED_CLIENT.set(api.clone()).expect("shared client only set once; qed");

	// Start a new tokio task to perform the runtime updates in the background.
	// if this fails then the miner will be stopped and has to be re-started.
	let (tx_upgrade, rx_upgrade) = oneshot::channel::<Error>();
	tokio::spawn(runtime_upgrade_task(api.clone(), tx_upgrade));

	let res = any_runtime!(chain, {
		let fut = match command {
			Command::Monitor(cfg) => monitor::monitor_cmd::<MinerConfig>(api, cfg).boxed(),
			Command::DryRun(cfg) => dry_run::dry_run_cmd::<MinerConfig>(api, cfg).boxed(),
			Command::EmergencySolution(cfg) =>
				emergency_solution::emergency_cmd::<MinerConfig>(api, cfg).boxed(),
		};

		run_command(fut, rx_upgrade).await
	});

	log::info!(target: LOG_TARGET, "round of execution finished. outcome = {:?}", res);
	res
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

/// Runs until the RPC connection fails or updating the metadata failed.
async fn runtime_upgrade_task(api: SubxtClient, tx: oneshot::Sender<Error>) {
	let updater = api.subscribe_to_updates();

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
				if let Err(e) = epm::update_metadata_constants(&api).await {
					let _ = tx.send(e.into());
					return
				}
				prometheus::on_runtime_upgrade();
				log::info!(target: LOG_TARGET, "upgrade to version: {} successful", version);
			},
			Err(e) => {
				log::warn!(target: LOG_TARGET, "upgrade to version: {} failed: {:?}", version, e);
			},
		}
	}
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
			"--prometheus-port",
			"9999",
			"monitor",
			"--seed-or-path",
			"//Alice",
			"--listen",
			"head",
			"seq-phragmen",
		])
		.unwrap();

		assert_eq!(
			opt,
			Opt {
				uri: "hi".to_string(),
				prometheus_port: Some(9999),
				command: Command::Monitor(MonitorConfig {
					listen: Listen::Head,
					solver: Solver::SeqPhragmen { iterations: 10 },
					submission_strategy: SubmissionStrategy::IfLeading,
					seed_or_path: "//Alice".to_string(),
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
				prometheus_port: None,
				command: Command::DryRun(DryRunConfig {
					at: None,
					solver: Solver::PhragMMS { iterations: 10 },
					force_snapshot: false,
					seed_or_path: "//Alice".to_string(),
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
				prometheus_port: None,
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
