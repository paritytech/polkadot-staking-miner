mod client;
mod commands;
mod epm;
mod error;
mod helpers;
mod opts;
mod prelude;
mod signer;
mod static_types;

use crate::{commands::*, opts::*};
use client::Client;
use error::Error;
use prelude::*;

use clap::Parser;
use futures::future::{BoxFuture, FutureExt};
use std::str::FromStr;
use tokio::sync::oneshot;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Clone, Parser)]
#[cfg_attr(test, derive(PartialEq))]
#[clap(author, version, about)]
pub struct Opt {
	/// The `ws` node to connect to.
	#[clap(long, short, default_value = DEFAULT_URI, env = "URI")]
	pub uri: String,

	#[clap(subcommand)]
	pub command: Command,

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
	Monitor(monitor::MonitorConfig),
}

#[tokio::main]
async fn main() -> Result<(), Error> {
	let Opt { uri, command, log } = Opt::parse();
	let filter = EnvFilter::from_default_env().add_directive(log.parse()?);
	tracing_subscriber::fmt().with_env_filter(filter).init();

	let client = Client::new(&uri).await?;
	let runtime_version: RuntimeVersion =
		client.rpc().state_get_runtime_version(None).await?.into();
	let chain = Chain::from_str(&runtime_version.spec_name)?;

	log::info!(target: LOG_TARGET, "Connected to chain {:?}", chain);
	epm::update_metadata_constants(client.chain_api())?;

	SHARED_CLIENT.set(client.clone()).expect("shared client only set once; qed");

	// Start a new tokio task to perform the runtime updates in the background.
	// if this fails then the miner will be stopped and has to be re-started.
	let (tx_upgrade, rx_upgrade) = oneshot::channel::<Error>();
	tokio::spawn(runtime_upgrade_task(client.chain_api().clone(), tx_upgrade));

	let fut = match command {
		Command::Monitor(cfg) => commands::monitor_cmd(client, cfg).boxed(),
		// TODO: other commands
	};

	let res = run_command(fut, rx_upgrade).await;
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
						return;
					},
				};
				continue;
			},
		};

		let version = update.runtime_version().spec_version;
		match updater.apply_update(update) {
			Ok(()) => {
				if let Err(e) = epm::update_metadata_constants(&client) {
					let _ = tx.send(e);
					return;
				}
				//prometheus::on_runtime_upgrade();
				log::info!(target: LOG_TARGET, "upgrade to version: {} successful", version);
			},
			Err(e) => {
				log::debug!(target: LOG_TARGET, "upgrade to version: {} failed: {:?}", version, e);
			},
		}
	}
}
