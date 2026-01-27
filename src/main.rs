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
	prelude::{DEFAULT_PROMETHEUS_PORT, DEFAULT_URI, LOG_TARGET, SHARED_CLIENT},
};

#[derive(Debug, Clone, Parser)]
#[cfg_attr(test, derive(PartialEq))]
#[clap(author, version, about)]
pub struct Opt {
	/// The `ws` node(s) to connect to. Multiple URIs can be comma-separated for failover.
	/// Example: "wss://rpc1.example.com,wss://rpc2.example.com"
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
	/// Run election prediction
	Predict(commands::types::PredictConfig),
}

#[tokio::main]
async fn main() -> Result<(), Error> {
	let Opt { uri, command, prometheus_port, log } = Opt::parse();
	let filter = EnvFilter::from_default_env().add_directive(log.parse()?);
	tracing_subscriber::fmt().with_env_filter(filter).init();

	// Start prometheus endpoint early so metrics are available during connection attempts.
	if let Err(e) = prometheus::run(prometheus_port).await {
		log::warn!("Failed to start prometheus endpoint: {e}");
	}
	// Initialize the timestamp so that if connection hangs, the stall detection alert can fire.
	prometheus::set_last_block_processing_time();

	let client = Client::new(&uri).await?;

	let version_bytes = client
		.chain_api()
		.await
		.runtime_api()
		.at_latest()
		.await?
		.call_raw("Core_version", None)
		.await?;
	let runtime_version: polkadot_sdk::sp_version::RuntimeVersion =
		Decode::decode(&mut &version_bytes[..])?;

	let chain = opt::Chain::try_from(&runtime_version)?;
	log::info!(target: LOG_TARGET, "Connected to chain: {chain}");

	SHARED_CLIENT.set(client.clone()).expect("shared client only set once; qed");

	// Start a new tokio task to perform the runtime updates in the background.
	// If this fails then the miner will be stopped and has to be re-started.
	// The upgrade task receives the Client wrapper so it can participate in failover.
	let (tx_upgrade, rx_upgrade) = oneshot::channel::<Error>();
	tokio::spawn(runtime_upgrade_task(client.clone(), tx_upgrade));

	update_metadata_constants(&*client.chain_api().await)?;

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
		Command::Predict(cfg) => {
			macros::for_multi_block_runtime!(chain, {
				commands::predict::predict_cmd::<MinerConfig>(client, cfg).boxed()
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

/// Recreate the runtime updates subscription with failover support.
async fn recreate_runtime_updates_with_failover(
	client: &Client,
	context: &str,
) -> Result<subxt::client::RuntimeUpdaterStream<subxt::PolkadotConfig>, Error> {
	let result = {
		let chain_api = client.chain_api().await;
		chain_api.updater().runtime_updates().await
	};
	match result {
		Ok(stream) => {
			log::info!(target: LOG_TARGET, "Successfully recreated runtime upgrade subscription {context}");
			Ok(stream)
		},
		Err(e) => {
			log::warn!(target: LOG_TARGET, "Failed to recreate subscription {context}: {e:?}, attempting failover...");
			if let Err(failover_err) = client.reconnect().await {
				log::error!(target: LOG_TARGET, "Runtime failover failed: {failover_err:?}");
				return Err(e.into());
			}
			let retry_result = {
				let chain_api = client.chain_api().await;
				chain_api.updater().runtime_updates().await
			};
			match retry_result {
				Ok(stream) => {
					log::info!(target: LOG_TARGET, "Successfully recreated runtime upgrade subscription after failover {context}");
					Ok(stream)
				},
				Err(e2) => {
					log::error!(target: LOG_TARGET, "Failed to recreate subscription after failover {context}: {e2:?}");
					Err(e2.into())
				},
			}
		},
	}
}

/// Runs until the RPC connection fails or updating the metadata failed.
/// Uses the Client wrapper to participate in failover when RPC connection fails.
async fn runtime_upgrade_task(client: Client, tx: oneshot::Sender<Error>) {
	/// Maximum number of consecutive subscription recreation attempts before giving up.
	const MAX_SUBSCRIPTION_RECREATION_ATTEMPTS: u32 = 3;

	let chain_api = client.chain_api().await;
	let updater = chain_api.updater();

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

	let mut subscription_recreation_attempts = 0u32;

	loop {
		// Use select to handle both runtime updates and periodic health checks
		tokio::select! {
			maybe_update = update_stream.next() => {
				match maybe_update {
					Some(Ok(update)) => {
						// Reset recreation attempts on successful update
						subscription_recreation_attempts = 0;
						// Process the update - get fresh chain_api in case of failover
						let chain_api = client.chain_api().await;
						let version = update.runtime_version().spec_version;
						match chain_api.updater().apply_update(update) {
							Ok(()) => {
								if let Err(e) = dynamic::update_metadata_constants(&chain_api) {
									let _ = tx.send(e);
									return;
								}
								prometheus::on_runtime_upgrade();
								log::info!(target: LOG_TARGET, "upgrade to version: {version} successful");
							},
							Err(e) => {
								log::trace!(target: LOG_TARGET, "upgrade to version: {version} failed: {e:?}");
							},
						}
					},
					Some(Err(e)) => {
						if e.is_disconnected_will_reconnect() {
							log::warn!(target: LOG_TARGET, "Runtime upgrade subscription disconnected, but will reconnect automatically");
							continue;
						}
						log::warn!(target: LOG_TARGET, "Runtime upgrade subscription error: {e:?}, attempting recreation...");
						crate::prometheus::on_updater_subscription_stall();

						subscription_recreation_attempts += 1;
						if subscription_recreation_attempts > MAX_SUBSCRIPTION_RECREATION_ATTEMPTS {
							log::error!(target: LOG_TARGET, "Exceeded maximum subscription recreation attempts ({MAX_SUBSCRIPTION_RECREATION_ATTEMPTS}), exiting");
							let _ = tx.send(e.into());
							return;
						}

						match recreate_runtime_updates_with_failover(&client, "after error").await
						{
							Ok(new_stream) => update_stream = new_stream,
							Err(err) => {
								let _ = tx.send(err);
								return;
							},
						}
					},
					None => {
						log::warn!(target: LOG_TARGET, "Runtime upgrade subscription stream ended, attempting recreation...");
						crate::prometheus::on_updater_subscription_stall();

						subscription_recreation_attempts += 1;
						if subscription_recreation_attempts > MAX_SUBSCRIPTION_RECREATION_ATTEMPTS {
							log::error!(target: LOG_TARGET, "Exceeded maximum subscription recreation attempts ({MAX_SUBSCRIPTION_RECREATION_ATTEMPTS}), exiting");
							let _ = tx.send(Error::Other("Runtime upgrade subscription stream ended".into()));
							return;
						}

						match recreate_runtime_updates_with_failover(
							&client,
							"after stream ended",
						)
						.await
						{
							Ok(new_stream) => update_stream = new_stream,
							Err(err) => {
								let _ = tx.send(err);
								return;
							},
						}
					},
				}
			},
			_ = health_check_interval.tick() => {
				log::trace!(target: LOG_TARGET, "Runtime upgrade subscription: periodic RPC health check");

				// Try to get the current block number as a health check
				// Note: We must drop the read guard before calling reconnect() to avoid deadlock
				let health_check_result = {
					let chain_api = client.chain_api().await;
					chain_api.blocks().at_latest().await
				};
				match health_check_result {
					Ok(_) => {
						log::trace!(target: LOG_TARGET, "RPC health check OK");
					},
					Err(e) => {
						log::warn!(target: LOG_TARGET, "RPC health check failed: {e:?} - recreating runtime upgrade subscription");
						crate::prometheus::on_updater_subscription_stall();

						subscription_recreation_attempts += 1;
						if subscription_recreation_attempts > MAX_SUBSCRIPTION_RECREATION_ATTEMPTS {
							log::error!(target: LOG_TARGET, "Exceeded maximum subscription recreation attempts ({MAX_SUBSCRIPTION_RECREATION_ATTEMPTS}), exiting");
							let _ = tx.send(e.into());
							return;
						}

						match recreate_runtime_updates_with_failover(
							&client,
							"after health check failure",
						)
						.await
						{
							Ok(new_stream) => update_stream = new_stream,
							Err(err) => {
								let _ = tx.send(err);
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
	use crate::commands::types::{ElectionAlgorithm, MultiBlockMonitorConfig, SubmissionStrategy};

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
					balancing_iterations: 10,    // Default
					algorithm: ElectionAlgorithm::SeqPhragmen,
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
				balancing_iterations: 10,    // Default
				algorithm: ElectionAlgorithm::SeqPhragmen,
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
				balancing_iterations: 10,    // Default
				algorithm: ElectionAlgorithm::SeqPhragmen,
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
				balancing_iterations: 10,   // Default
				algorithm: ElectionAlgorithm::SeqPhragmen,
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
				balancing_iterations: 10,    // Default
				algorithm: ElectionAlgorithm::SeqPhragmen,
			})
		);
	}
}
