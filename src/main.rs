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
//! Simple bot capable of monitoring Polkadot / Kusama / Westend Asset Hub chain and submitting
//! solutions to the `pallet-election-provider-multi-block`.
//! See `help` for more information.
//!
//! # Implementation Notes:
//!
//! The miner is designed to operate 24/7. However, if it encounters unrecoverable errors (e.g.
//! RPC or IO errors), it will crash.
//! In a production environment, run it with the `restart = true` setting, which will report the
//! crash and resume work afterward.
#![cfg_attr(docsrs, feature(doc_cfg))]

mod client;
mod commands;
mod dynamic;
mod error;
mod macros;
mod opt;
mod prelude;
mod prometheus;
mod runtime;
mod signer;
mod static_types;
mod utils;

use clap::Parser;
use codec::Decode;
use error::Error;
use futures::future::{BoxFuture, FutureExt};
use tokio::sync::oneshot;
use tracing_subscriber::EnvFilter;

use crate::{
	client::Client,
	dynamic::update_metadata_constants,
	prelude::{ChainClient, DEFAULT_PROMETHEUS_PORT, DEFAULT_URI, LOG_TARGET, SHARED_CLIENT},
};

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

	/// Sets a custom logging filter. Syntax is `<target>=<level>`, e.g.
	/// -lpolkadot-staking-miner=debug.
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
	Monitor(commands::types::MultiBlockMonitorConfig),
	/// Check if the staking-miner metadata is compatible to a remote node.
	Info,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
	let Opt { uri, command, prometheus_port, log } = Opt::parse();
	let filter = EnvFilter::from_default_env().add_directive(log.parse()?);
	tracing_subscriber::fmt().with_env_filter(filter).init();

	let client = Client::new(&uri).await?;

	let version_bytes = client
		.chain_api()
		.runtime_api()
		.at_latest()
		.await?
		.call_raw("Core_version", None)
		.await?;
	let runtime_version: polkadot_sdk::sp_version::RuntimeVersion =
		Decode::decode(&mut &version_bytes[..])?;

	let chain = opt::Chain::try_from(&runtime_version)?;
	if let Err(e) = prometheus::run(prometheus_port).await {
		log::warn!("Failed to start prometheus endpoint: {e}");
	}
	log::info!(target: LOG_TARGET, "Connected to chain: {chain}");

	SHARED_CLIENT.set(client.clone()).expect("shared client only set once; qed");

	// Start a new tokio task to perform the runtime updates in the background.
	// if this fails then the miner will be stopped and has to be re-started.
	let (tx_upgrade, rx_upgrade) = oneshot::channel::<Error>();
	tokio::spawn(runtime_upgrade_task(client.chain_api().clone(), tx_upgrade));

	update_metadata_constants(client.chain_api())?;

	let fut = match command {
		Command::Info => async {
			// Create a simple map for serialization since SDK RuntimeVersion doesn't derive
			// Serialize. All these field must exist on substrate-based chains
			// (see https://docs.rs/sp-version/latest/sp_version/struct.RuntimeVersion.html)
			let runtime_info = serde_json::json!({
				"spec_name": runtime_version.spec_name.to_string(),
				"impl_name": runtime_version.impl_name.to_string(),
				"spec_version": runtime_version.spec_version,
				"impl_version": runtime_version.impl_version,
				"authoring_version": runtime_version.authoring_version,
				"transaction_version": runtime_version.transaction_version
			});
			let remote_node =
				serde_json::to_string_pretty(&runtime_info).expect("Serialize is infallible; qed");

			eprintln!("Remote_node:\n{remote_node}");

			Ok(())
		}
		.boxed(),
		Command::Monitor(cfg) => {
			macros::for_multi_block_runtime!(chain, {
				commands::multi_block::monitor_cmd::<MinerConfig>(client, cfg).boxed()
			})
		},
	};

	let res = run_command(fut, rx_upgrade).await;

	log::debug!(target: LOG_TARGET, "round of execution finished. outcome = {res:?}");
	res
}

#[cfg(target_family = "unix")]
async fn run_command(
	fut: BoxFuture<'_, Result<(), Error>>,
	rx_upgrade: oneshot::Receiver<Error>,
) -> Result<(), Error> {
	use tokio::signal::unix::{SignalKind, signal};

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
async fn runtime_upgrade_task(client: ChainClient, tx: oneshot::Sender<Error>) {
	let updater = client.updater();

	let mut update_stream = match updater.runtime_updates().await {
		Ok(u) => u,
		Err(e) => {
			let _ = tx.send(e.into());
			return;
		},
	};

	// Health check interval - check RPC connection every 1 hour
	const HEALTH_CHECK_INTERVAL_SECS: u64 = 60 * 60;
	let mut health_check_interval =
		tokio::time::interval(std::time::Duration::from_secs(HEALTH_CHECK_INTERVAL_SECS));
	health_check_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

	loop {
		// Use select to handle both runtime updates and periodic health checks
		tokio::select! {
			maybe_update = update_stream.next() => {
				match maybe_update {
					Some(Ok(update)) => {
						// Process the update
						let version = update.runtime_version().spec_version;
						match updater.apply_update(update) {
							Ok(()) => {
								if let Err(e) = dynamic::update_metadata_constants(&client) {
									let _ = tx.send(e);
									return;
								}
								prometheus::on_runtime_upgrade();
								log::info!(target: LOG_TARGET, "upgrade to version: {} successful", version);
							},
							Err(e) => {
								log::trace!(target: LOG_TARGET, "upgrade to version: {} failed: {:?}", version, e);
							},
						}
					},
					Some(Err(e)) => {
						if e.is_disconnected_will_reconnect() {
							log::warn!(target: LOG_TARGET, "Runtime upgrade subscription disconnected, but will reconnect automatically");
							continue;
						}
						log::error!(target: LOG_TARGET, "Runtime upgrade subscription error: {e:?}");
						let _ = tx.send(e.into());
						return;
					},
					None => {
						log::error!(target: LOG_TARGET, "Runtime upgrade subscription stream ended. Connection is dead. Shutting down.");
						let _ = tx.send(Error::Other("Runtime upgrade subscription stream ended".into()));
						return;
					},
				}
			},
			_ = health_check_interval.tick() => {
				log::trace!(target: LOG_TARGET, "Runtime upgrade subscription: periodic RPC health check");

				// Try to get the current block number as a health check
				match client.blocks().at_latest().await {
					Ok(_) => {
						log::trace!(target: LOG_TARGET, "RPC health check OK");
					},
					Err(e) => {
						log::warn!(target: LOG_TARGET, "RPC health check failed: {e:?} - recreating runtime upgrade subscription");
						crate::prometheus::on_updater_subscription_stall();

						// Recreate the subscription
						match updater.runtime_updates().await {
							Ok(new_stream) => {
								update_stream = new_stream;
								log::info!(target: LOG_TARGET, "Successfully recreated runtime upgrade subscription after health check failure");
							},
							Err(e) => {
								log::error!(target: LOG_TARGET, "Failed to recreate runtime upgrade subscription: {e:?}");
								let _ = tx.send(e.into());
								return;
							},
						}
					}
				}
			},
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::commands::types::{MultiBlockMonitorConfig, SubmissionStrategy};

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
			"--do-reduce",
		])
		.unwrap();

		assert_eq!(
			opt,
			Opt {
				uri: "hi".to_string(),
				prometheus_port: 9999,
				log: "info".to_string(),
				command: Command::Monitor(MultiBlockMonitorConfig {
					seed_or_path: "//Alice".to_string(),
					submission_strategy: SubmissionStrategy::IfLeading, // Default
					do_reduce: true,
					chunk_size: 0,               // Default
					min_signed_phase_blocks: 10, // Default
					shady: false,                // Default
				}),
			}
		);
	}

	#[test]
	fn cli_monitor_default_works() {
		let opt = Opt::try_parse_from([
			env!("CARGO_PKG_NAME"),
			"--uri",
			"hi",
			"monitor",
			"--seed-or-path",
			"//Alice",
			// No --do-reduce flag
		])
		.unwrap();

		assert_eq!(
			opt.command,
			Command::Monitor(MultiBlockMonitorConfig {
				seed_or_path: "//Alice".to_string(),
				submission_strategy: SubmissionStrategy::IfLeading,
				do_reduce: false,            // Default
				chunk_size: 0,               // Default
				min_signed_phase_blocks: 10, // Default
				shady: false,                // Default
			})
		);
	}

	#[test]
	fn cli_monitor_with_chunk_size_works() {
		let opt = Opt::try_parse_from([
			env!("CARGO_PKG_NAME"),
			"--uri",
			"hi",
			"monitor",
			"--seed-or-path",
			"//Alice",
			"--chunk-size",
			"4",
		])
		.unwrap();

		assert_eq!(
			opt.command,
			Command::Monitor(MultiBlockMonitorConfig {
				seed_or_path: "//Alice".to_string(),
				submission_strategy: SubmissionStrategy::IfLeading,
				do_reduce: false,            // Default
				chunk_size: 4,               // Explicitly set
				min_signed_phase_blocks: 10, // Default
				shady: false,                // Default
			})
		);
	}

	#[test]
	fn cli_monitor_with_min_signed_phase_blocks_works() {
		let opt = Opt::try_parse_from([
			env!("CARGO_PKG_NAME"),
			"--uri",
			"hi",
			"monitor",
			"--seed-or-path",
			"//Alice",
			"--min-signed-phase-blocks",
			"5",
		])
		.unwrap();

		assert_eq!(
			opt.command,
			Command::Monitor(MultiBlockMonitorConfig {
				seed_or_path: "//Alice".to_string(),
				submission_strategy: SubmissionStrategy::IfLeading,
				do_reduce: false,           // Default
				chunk_size: 0,              // Default
				min_signed_phase_blocks: 5, // Explicitly set
				shady: false,               // Default
			})
		);
	}

	#[test]
	fn cli_monitor_with_shady_works() {
		let opt = Opt::try_parse_from([
			env!("CARGO_PKG_NAME"),
			"--uri",
			"hi",
			"monitor",
			"--seed-or-path",
			"//Alice",
			"--shady",
		])
		.unwrap();

		assert_eq!(
			opt.command,
			Command::Monitor(MultiBlockMonitorConfig {
				seed_or_path: "//Alice".to_string(),
				submission_strategy: SubmissionStrategy::IfLeading,
				do_reduce: false,            // Default
				chunk_size: 0,               // Default
				min_signed_phase_blocks: 10, // Default
				shady: true,                 // Explicitly set
			})
		);
	}
}
