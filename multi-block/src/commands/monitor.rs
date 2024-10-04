use crate::{client::Client, error::Error, prelude::Hash, prelude::*, signer::Signer};

use std::{str::FromStr, sync::Arc};
use tokio::sync::Mutex;
use clap::Parser;
use subxt::{
	PolkadotConfig, OnlineClient,
	backend::rpc::RpcSubscription,
	config::Header as _,
};
use crate::runtime::runtime_types::pallet_election_provider_multi_block::types::*;
use crate::static_types::Pages;

type Storage = subxt::storage::Storage<PolkadotConfig, OnlineClient<PolkadotConfig>>;

#[derive(Debug, Clone, Parser)]
pub struct MonitorConfig {
	#[clap(long)]
	pub at: Option<Hash>,

	#[clap(long, short, env = "SEED")]
	pub seed_or_path: String,

	#[clap(long, value_enum, default_value_t = Listen::Finalized)]
	pub listen: Listen,
}

#[cfg_attr(test, derive(PartialEq))]
#[derive(clap::ValueEnum, Debug, Copy, Clone)]
pub enum Listen {
	Finalized,
	Head,
}

pub async fn monitor_cmd(client: Client, config: MonitorConfig) -> Result<(), Error> {
	let signer = Signer::new(&config.seed_or_path)?;
	let account_info = {
		let addr = runtime::storage().system().account(signer.account_id());
		client
			.chain_api()
			.storage()
			.at_latest()
			.await?
			.fetch(&addr)
			.await?
			.ok_or(Error::AccountDoesNotExists)?
	};

	log::info!(target: LOG_TARGET, "Loaded account {} {{ nonce: {}, free_balance: {}, reserved_balance: {}, frozen_balance: {} }}",
		signer,
		account_info.nonce,
		account_info.data.free,
		account_info.data.reserved,
		account_info.data.frozen,
	);

	let mut subscription = heads_subscription(client.rpc(), config.listen).await?;
	let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Error>();
	let submit_lock = Arc::new(Mutex::new(()));

	let mut target_snapshot: Vec<u32> = Default::default();
	let mut voter_snapshot_paged: Vec<u32> = Default::default();

	let n_pages = Pages::get();
	let mut last_round_submitted = Default::default();

	loop {
		let at = tokio::select! {
			maybe_rp = subscription.next() => {
				match maybe_rp {
					Some(Ok(r)) => r,
					Some(Err(e)) => {
						log::error!(target: LOG_TARGET, "subscription failed to decode Header {:?}, this is bug please file an issue", e);
						return Err(e.into());
					}
					// The subscription was dropped, should only happen if:
					//	- the connection was closed.
					//	- the subscription could not keep up with the server.
					None => {
						log::warn!(target: LOG_TARGET, "subscription to `{:?}` terminated. Retrying..", config.listen);
						subscription = heads_subscription(client.rpc(), config.listen).await?;
						continue
					}
				}
			},
			maybe_err = rx.recv() => {
				match maybe_err {
					Some(err) => return Err(err),
					None => unreachable!("at least one sender kept in the main loop should always return Some; qed"),
				}
			}
		};

		let storage = helpers::storage_at(config.at, client.chain_api()).await?;
		let phase = storage.fetch_or_default(&runtime::storage().election_provider_multi_block().current_phase())
			.await?;
		let round = storage.fetch_or_default(&runtime::storage().election_provider_multi_block().round())
			.await?;

		let signer2 = signer.clone();
		let client2 = client.clone();
		let config2 = config.clone();
		let submit_lock2 = submit_lock.clone();

		let result = tokio::spawn(async move {
			match phase {
				Phase::Off => {
					log::info!(target: LOG_TARGET, "Phase::Off, do nothing.");
					Artifact::Nothing
				},

				Phase::Snapshot(page) => {
					log::info!(target: LOG_TARGET, "Phase::Snapshot({})", page);
					if let Ok(paged_snapshot) = snapshot_page(at.clone(), client2, config2, storage).await {
						log::info!(target: LOG_TARGET, "fetched snapshot page {:?}", page);

						if page == n_pages {
							Artifact::TargetSnapshot(0)
						} else {
							Artifact::VoterSnapshot(1)
						}

					} else {
						// do something if critical error (panic).
						log::error!(target: LOG_TARGET, "error during snapshot processing");
						Artifact::Nothing
					}
				},
				Phase::Signed => {
					log::info!(target: LOG_TARGET, "Phase::Signed",);
					Artifact::ComputeElectionResult
				},
				_ => {
					log::info!(target: LOG_TARGET, "other phase");
					Artifact::Nothing
				}
			}
		});

		match result.await {
			Ok(Artifact::TargetSnapshot(s)) => {
				log::info!(target: LOG_TARGET, "Artifact::Target: {:?}", s);
				target_snapshot.push(s);
			},
			Ok(Artifact::VoterSnapshot(p)) => {
				log::info!(target: LOG_TARGET, "Artifact::Voter: {:?}", p);
				voter_snapshot_paged.push(p);
			},
			Ok(Artifact::ComputeElectionResult) => {
				log::info!(target: LOG_TARGET, "Artifact::ComputeElectionResult");
				if round == last_round_submitted {
					// skip minig again, everything submitted.
					log::info!(target: LOG_TARGET, "Successfully submitted in round {}", round);
					continue
				}

				// all pages in cache, compute election.
				if target_snapshot.len() == 1 as usize && voter_snapshot_paged.len() == n_pages as usize {
					match mine_and_submit(&signer, &config, &target_snapshot, &voter_snapshot_paged).await {
						Ok(_) => last_round_submitted = round,
						Err(_) => (), // continue trying.
					}
				} else {
					match fetch_mine_and_submit(&signer, &config, &target_snapshot, &voter_snapshot_paged).await {
						Ok(_) => last_round_submitted = round,
						Err(_) => (), // continue trying.
					}
					log::info!(target: LOG_TARGET, "not all snapshots in cache, fetch all and compute.");
				}
			},
			Ok(Artifact::Nothing) => log::info!(target: LOG_TARGET, "skip."),
			Err(e) => log::error!(target: LOG_TARGET, "ERROR: {:?}", e),
		}

	}
	Ok(())
}

enum Artifact {
	Nothing,
	TargetSnapshot(u32),
	VoterSnapshot(u32),
	ComputeElectionResult,
}

async fn snapshot_page(
	at: Header,
	client: Client,
	config: MonitorConfig,
	storage: Storage,
) -> Result<u32, Error> {
	let round = storage.fetch_or_default(&runtime::storage().election_provider_multi_block().round())
		.await?;
	let phase = storage.fetch_or_default(&runtime::storage().election_provider_multi_block().current_phase())
		.await?;

	log::info!(
		target: LOG_TARGET,
		"new event at block #{:?} [round {}, phase: {:?}]",
		at.number,
		round,
		phase
	);

	Ok(1)
}

async fn mine_and_submit(
	signer: &Signer,
	config: &MonitorConfig,
	target_snapshot: &Vec<u32>,
	voter_snapshot_paged: &Vec<u32>,
) -> Result<u64, Error> {

	log::info!(target: LOG_TARGET, "Mine, submit: election 'target snap': {:?}, 'voter snap': {:?}", target_snapshot, voter_snapshot_paged);

	//Err(Error::Other("testing".into()))
	Ok(10)
}

async fn fetch_mine_and_submit(
	signer: &Signer,
	config: &MonitorConfig,
	target_snapshot: &Vec<u32>,
	voter_snapshot_paged: &Vec<u32>,
) -> Result<u64, Error> {

	log::info!(target: LOG_TARGET, "(Fetch), mine, submit: election 'target snap': {:?}, 'voter snap': {:?}", target_snapshot, voter_snapshot_paged);

	Ok(1)
}


async fn heads_subscription(
	rpc: &RpcClient,
	listen: Listen,
) -> Result<RpcSubscription<Header>, Error> {
	match listen {
		Listen::Head => rpc.chain_subscribe_new_heads().await,
		Listen::Finalized => rpc.chain_subscribe_finalized_heads().await,
	}
	.map_err(Into::into)
}

mod helpers {
    use super::*;
	use subxt::storage::Storage;

	/// Helper to get storage at block.
	pub async fn storage_at(
		block: Option<Hash>,
		api: &ChainClient,
	) -> Result<Storage<Config, ChainClient>, Error> {
		if let Some(block_hash) = block {
			Ok(api.storage().at(block_hash))
		} else {
			api.storage().at_latest().await.map_err(Into::into)
		}
	}
}
