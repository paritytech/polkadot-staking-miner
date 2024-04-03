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
mod commands;
mod epm;
mod error;
mod helpers;
mod opt;
mod prelude;
mod prometheus;
mod signer;
mod static_types;

use clap::Parser;
use codec::Decode;
use error::Error;
use futures::future::{BoxFuture, FutureExt};
use prelude::*;
use std::str::FromStr;
use subxt::backend::rpc::RpcSubscription;
use tokio::sync::oneshot;
use tracing_subscriber::EnvFilter;

use crate::{client::Client, opt::RuntimeVersion};

#[derive(Debug, Clone, Parser)]
#[cfg_attr(test, derive(PartialEq))]
#[clap(author, version, about)]
pub struct Opt {
	/// The `ws` node to connect to.
	#[clap(long, short, default_value = DEFAULT_URI, env = "URI")]
	pub uri: String,

	#[clap(subcommand)]
	pub command: Command,

	/// The prometheus endpoint TCP port.
	#[clap(long, short, env = "PROMETHEUS_PORT", default_value_t = DEFAULT_PROMETHEUS_PORT)]
	pub prometheus_port: u16,

	/// Sets a custom logging filter. Syntax is `<target>=<level>`, e.g. -lpolkadot-staking-miner=debug.
	///
	/// Log levels (least to most verbose) are error, warn, info, debug, and trace.
	/// By default, all targets log `info`. The global log level can be set with `-l<level>`.
	#[clap(long, short, default_value = "info")]
	pub log: String,
}

#[derive(Debug, Clone, Parser)]
#[cfg_attr(test, derive(PartialEq))]
pub enum Command {
	/// Monitor for the phase being signed, then compute.
	Monitor(commands::MonitorConfig),
	/// Just compute a solution now, and don't submit it.
	DryRun(commands::DryRunConfig),
	/// Provide a solution that can be submitted to the chain as an emergency response.
	EmergencySolution(commands::EmergencySolutionConfig),
	/// Check if the staking-miner metadata is compatible to a remote node.
	Info,
}

// A helper to use different MinerConfig depending on chain.
macro_rules! any_runtime {
	($chain:tt, $($code:tt)*) => {
		match $chain {
			$crate::opt::Chain::Polkadot => {
				#[allow(unused)]
				use $crate::static_types::polkadot::{MinerConfig, SIGNED_PHASE_LENGTH};
				$($code)*
			},
			$crate::opt::Chain::Kusama => {
				#[allow(unused)]
				use $crate::static_types::kusama::{MinerConfig, SIGNED_PHASE_LENGTH};
				$($code)*
			},
			$crate::opt::Chain::Westend => {
				#[allow(unused)]
				use $crate::static_types::westend::{MinerConfig, SIGNED_PHASE_LENGTH};
				$($code)*
			},
		}
	};
}

#[tokio::main]
async fn main() -> Result<(), Error> {
	let Opt { uri, command, prometheus_port, log } = Opt::parse();
	let filter = EnvFilter::from_default_env().add_directive(log.parse()?);
	tracing_subscriber::fmt().with_env_filter(filter).init();

	let client = Client::new(&uri).await?;
	let runtime_version: RuntimeVersion =
		client.rpc().state_get_runtime_version(None).await?.into();
	let chain = opt::Chain::from_str(&runtime_version.spec_name)?;
	let _prometheus_handle = prometheus::run(prometheus_port)
		.map_err(|e| log::warn!("Failed to start prometheus endpoint: {}", e));
	log::info!(target: LOG_TARGET, "Connected to chain: {}", chain);
	epm::update_metadata_constants(client.chain_api())?;

	SHARED_CLIENT.set(client.clone()).expect("shared client only set once; qed");

	// Start a new tokio task to perform the runtime updates in the background.
	// if this fails then the miner will be stopped and has to be re-started.
	let (tx_upgrade, rx_upgrade) = oneshot::channel::<Error>();
	tokio::spawn(runtime_upgrade_task(client.clone(), tx_upgrade, runtime_version.spec_version));

	let res = any_runtime!(chain, {
		let fut = match command {
			Command::Monitor(cfg) =>
				commands::monitor_cmd::<MinerConfig>(client, cfg, SIGNED_PHASE_LENGTH).boxed(),
			Command::DryRun(cfg) => commands::dry_run_cmd::<MinerConfig>(client, cfg).boxed(),
			Command::EmergencySolution(cfg) =>
				commands::emergency_solution_cmd::<MinerConfig>(client, cfg).boxed(),
			Command::Info => async {
				let is_compat = if runtime::is_codegen_valid_for(&client.chain_api().metadata()) {
					"YES"
				} else {
					"NO"
				};

				let remote_node = serde_json::to_string_pretty(&runtime_version)
					.expect("Serialize is infallible; qed");

				eprintln!("Remote_node:\n{remote_node}");
				eprintln!("Compatible: {is_compat}");

				Ok(())
			}
			.boxed(),
		};

		run_command(fut, rx_upgrade).await
	});

	log::debug!(target: LOG_TARGET, "round of execution finished. outcome = {:?}", res);
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
async fn runtime_upgrade_task(client: Client, tx: oneshot::Sender<Error>, mut spec_version: u32) {
	use sp_core::storage::StorageChangeSet;

	async fn new_update_stream(
		client: &Client,
	) -> Result<RpcSubscription<StorageChangeSet<Hash>>, subxt::Error> {
		use sp_core::Bytes;
		use subxt::rpc_params;

		let storage_key = Bytes(runtime::storage().system().last_runtime_upgrade().to_root_bytes());

		client
			.raw_rpc()
			.subscribe(
				"state_subscribeStorage",
				rpc_params![vec![storage_key]],
				"state_unsubscribeStorage",
			)
			.await
	}

	let mut update_stream = match new_update_stream(&client).await {
		Ok(s) => s,
		Err(e) => {
			_ = tx.send(e.into());
			return;
		},
	};

	let close_err = loop {
		let change_set = match update_stream.next().await {
			Some(Ok(changes)) => changes,
			Some(Err(err)) => break err.into(),
			None => {
				update_stream = match new_update_stream(&client).await {
					Ok(sub) => sub,
					Err(err) => break err.into(),
				};
				continue;
			},
		};

		let at = change_set.block;
		assert!(change_set.changes.len() < 2, "Only one storage change per runtime upgrade");
		let Some(bytes) = change_set.changes.get(0).and_then(|v| v.1.clone()) else { continue };
		let next: runtime::runtime_types::frame_system::LastRuntimeUpgradeInfo =
			match Decode::decode(&mut bytes.0.as_ref()) {
				Ok(n) => n,
				Err(e) => break e.into(),
			};

		if next.spec_version > spec_version {
			let metadata = match client.rpc().state_get_metadata(Some(at)).await {
				Ok(m) => m,
				Err(err) => break err.into(),
			};

			let runtime_version = match client.rpc().state_get_runtime_version(Some(at)).await {
				Ok(r) => r,
				Err(err) => break err.into(),
			};

			client.chain_api().set_metadata(metadata);
			client.chain_api().set_runtime_version(subxt::backend::RuntimeVersion {
				spec_version: runtime_version.spec_version,
				transaction_version: runtime_version.transaction_version,
			});

			spec_version = next.spec_version;
			prometheus::on_runtime_upgrade();

			log::info!(target: LOG_TARGET, "Runtime upgraded to v{spec_version}");
			if let Err(e) = epm::update_metadata_constants(client.chain_api()) {
				break e;
			}
		}
	};

	let _ = tx.send(close_err.into());
}

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
