use crate::{client::Client, error::Error, prelude::*, signer::Signer};

use crate::{
	epm, helpers, runtime::runtime_types::pallet_election_provider_multi_block::types::*,
	static_types,
};

use pallet_election_provider_multi_block::unsigned::miner;

use clap::Parser;
use std::sync::Arc;
use subxt::backend::rpc::RpcSubscription;
use tokio::sync::Mutex;

/// Represents the current task to be performed in the election round.
enum Task {
	/// Do nothing.
	Nothing,
	/// A page of the target snapshot has been fetched, cache it locally.
	CacheTargetSnapshot(TargetSnapshotPage),
	/// A page of the voter snapshot has been fetched, cache it locally.
	CacheVoterSnapshot(VoterSnapshotPage),
	/// Next task is to compute the election.
	ComputeElectionResult,
}

#[derive(Debug, Clone, Parser, PartialEq)]
pub struct MonitorConfig {
	#[clap(long)]
	pub at: Option<Hash>,

	#[clap(long, short, env = "SEED")]
	pub seed_or_path: String,

	#[clap(long, value_enum, default_value_t = Listen::Head)]
	pub listen: Listen,
}

#[derive(clap::ValueEnum, Debug, Copy, Clone, PartialEq)]
pub enum Listen {
	Finalized,
	Head,
}

/// Monitors the current on-chain phase and performs the relevant actions.
///
/// Steps:
/// 1. In `Phase::Snapshot(page)`, fetch the pages of the target and voter snapshot.
/// 2. In `Phase::Signed`:
///  2.1. Compute the solution
///  2.2. Register the solution with the *full* election score of the submission
///  2.3. Submit each page separately (1 per block).
///
/// If by the time of computing the solution the snapshot pages have not been fetched and cached,
/// the snapshots will be fetched before computing the solution.
pub async fn monitor_cmd<T>(client: Client, config: MonitorConfig) -> Result<(), Error>
where
	T: miner::Config<
			AccountId = AccountId,
			MaxVotesPerVoter = static_types::MaxVotesPerVoter,
			TargetSnapshotPerBlock = static_types::TargetSnapshotPerBlock,
			VoterSnapshotPerBlock = static_types::VoterSnapshotPerBlock,
			Pages = static_types::Pages,
		> + Send
		+ Sync
		+ 'static,
	T::Solution: Send,
{
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
	let (_tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Error>();
	let _submit_lock = Arc::new(Mutex::new(()));

	let mut target_snapshot: TargetSnapshotPage = Default::default();
	let mut voter_snapshot_paged: Vec<VoterSnapshotPage> = Default::default();

	let n_pages = static_types::Pages::get();
	let mut last_round_submitted = None;

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
		let storage2 = storage.clone();
		let phase = storage
			.fetch_or_default(&runtime::storage().election_provider_multi_block().current_phase())
			.await?;
		let round = storage
			.fetch_or_default(&runtime::storage().election_provider_multi_block().round())
			.await?;

		let result = tokio::spawn(async move {
			match phase {
				Phase::Off => {
					log::trace!(target: LOG_TARGET, "Phase::Off, do nothing.");
					Task::Nothing
				},

				Phase::Snapshot(page) =>
					if page == n_pages {
						epm::target_snapshot(&storage)
							.await
							.map(|t| Task::CacheTargetSnapshot(t))
							.unwrap_or(Task::Nothing)
					} else {
						epm::paged_voter_snapshot(page, &storage)
							.await
							.map(|v| Task::CacheVoterSnapshot(v))
							.unwrap_or(Task::Nothing)
					},
				Phase::Signed => {
					log::trace!(target: LOG_TARGET, "Phase::Signed",);
					Task::ComputeElectionResult
				},
				_ => {
					log::trace!(target: LOG_TARGET, "{:?}, do nothing.", phase);
					Task::Nothing
				},
			}
		});

		match result.await {
			Ok(Task::CacheTargetSnapshot(s)) => {
				target_snapshot = s;
			},
			Ok(Task::CacheVoterSnapshot(p)) => {
				// TODO: page from msp -> lsp, prepend p instead of push().
				voter_snapshot_paged.push(p);
			},
			Ok(Task::ComputeElectionResult) => {
				if last_round_submitted == Some(round) {
					// skip minig again, everything submitted.
					log::trace!(
						target: LOG_TARGET,
						"Solution successfully submitted for round {}, do nothing.",
						round
					);
					continue
				}

				if !target_snapshot.is_empty() && voter_snapshot_paged.len() == n_pages as usize {
					// all pages in cache, compute election.
					match epm::mine_and_submit::<T>(
						&at,
						&client,
						signer.clone(),
						config.listen,
						&target_snapshot,
						&voter_snapshot_paged,
						n_pages,
						round,
					)
					.await
					{
						Ok(_) => last_round_submitted = Some(round),
						Err(err) => {
							log::error!("mine_and_submit: {:?}", err);
						}, // continue trying.
					}
				} else {
					// TODO: check if there are already *some* pageed cached and fetch only missing
					// ones.
					match epm::fetch_mine_and_submit::<T>(
						&at,
						&client,
						signer.clone(),
						config.listen,
						&storage2,
						n_pages,
						round,
					)
					.await
					{
						Ok(_) => last_round_submitted = Some(round),
						Err(err) => {
							log::error!("fetch_mine_and_submit: {:?}", err);
						}, // continue trying.
					}
					log::trace!(target: LOG_TARGET, "not all snapshots in cache, fetch all and compute.");
				}
			},
			Ok(Task::Nothing) => {
				// reset cached snapshot.
				target_snapshot = Default::default();
				voter_snapshot_paged = Default::default();
			},
			Err(e) => log::error!(target: LOG_TARGET, "ERROR: {:?}", e),
		}
	}
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
