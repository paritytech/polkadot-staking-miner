use crate::{
	client::Client,
	commands::Listen,
	epm,
	error::Error,
	helpers,
	prelude::{
		runtime, AccountId, Header, RpcClient, TargetSnapshotPage, VoterSnapshotPage, LOG_TARGET,
	},
	signer::Signer,
	static_types,
};

use polkadot_sdk::pallet_election_provider_multi_block::{
	types::Phase, unsigned::miner::MinerConfig,
};
use std::{collections::BTreeMap, sync::Arc};
use subxt::{backend::rpc::RpcSubscription, config::Header as _};
use tokio::sync::Mutex;

/// TODO(niklasad1): Add solver algorithm configuration to the monitor command.
#[derive(Debug, Clone, clap::Parser, PartialEq)]
pub struct MonitorConfig {
	#[clap(long, short, env = "SEED")]
	pub seed_or_path: String,

	// TODO: finalized head subscription ends with an error, not signed something.
	#[clap(long, value_enum, default_value_t = Listen::Head)]
	pub listen: Listen,
}

/// Monitors the current on-chain phase and performs the relevant actions.
///
/// Steps:
/// 1. In `Phase::Snapshot(page)`, fetch the pages of the target and voter snapshot.
/// 2. In `Phase::Signed`:
///  2.1. Compute the solution
///  2.2. Register the solution with the *full* election score of the submission
///  2.3. Submit each page separately (1 per block).
pub async fn monitor_cmd<T>(client: Client, config: MonitorConfig) -> Result<(), Error>
where
	T: MinerConfig<AccountId = AccountId> + Send + Sync + 'static,
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
	let mut target_snapshot: TargetSnapshotPage<T> = Default::default();
	let mut voter_snapshot_paged: BTreeMap<u32, VoterSnapshotPage<T>> = Default::default();
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

		let storage = helpers::storage_at(Some(at.hash()), client.chain_api()).await?;
		let storage2 = storage.clone();
		let phase = storage
			.fetch_or_default(&runtime::storage().multi_block().current_phase())
			.await?;
		let round = storage.fetch_or_default(&runtime::storage().multi_block().round()).await?;
		let desired_targets = storage
			.fetch(&runtime::storage().multi_block().desired_targets())
			.await
			.unwrap()
			.unwrap_or(0);
		let n_pages = static_types::Pages::get();

		log::trace!(target: LOG_TARGET, "Processing block={} round={}, phase={:?}", at.number, round, phase);

		match phase.0 {
			Phase::Snapshot(page) => {
				if page == n_pages - 1 {
					match epm::target_snapshot::<T>(page, &storage).await {
						Ok(snapshot) => {
							target_snapshot = snapshot;
						},
						Err(err) => {
							log::error!("fetch snapshot err: {:?}", err);
						},
					};
				}

				match epm::paged_voter_snapshot::<T>(page, &storage).await {
					Ok(snapshot) => {
						voter_snapshot_paged.insert(page, snapshot);
					},
					Err(err) => {
						log::error!("fetch voter snapshot err: {:?}", err);
					},
				};
			},
			Phase::Signed => {
				if last_round_submitted == Some(round) {
					// skip mining again, everything submitted.
					log::trace!(
						target: LOG_TARGET,
						"Solution successfully submitted for round {}, do nothing.",
						round
					);
					continue;
				}

				// All pages are already fetched, compute the solution.
				if !target_snapshot.is_empty() && voter_snapshot_paged.len() == n_pages as usize {
					let v = voter_snapshot_paged.iter().map(|(_, v)| v.clone()).collect::<Vec<_>>();

					match epm::mine_and_submit::<T>(
						&at,
						&client,
						signer.clone(),
						config.listen,
						&target_snapshot,
						&v,
						n_pages,
						round,
						desired_targets,
					)
					.await
					{
						Ok(_) => last_round_submitted = Some(round),
						Err(err) => {
							log::error!("mine_and_submit: {:?}", err);
						}, // continue trying the next block.
					}
				} else {
					// TODO: we can optimize this by fetching only the missing pages
					// instead of all of them here.
					match epm::fetch_mine_and_submit::<T>(
						&at,
						&client,
						signer.clone(),
						config.listen,
						&storage2,
						n_pages,
						round,
						desired_targets,
					)
					.await
					{
						Ok(_) => last_round_submitted = Some(round),
						Err(err) => {
							log::error!("fetch_mine_and_submit: {:?}", err);
						}, // continue trying the next block.
					}
					log::trace!(target: LOG_TARGET, "not all snapshots in cache, fetch all and compute.");
				}
			},
			_ => {
				voter_snapshot_paged.clear();
				target_snapshot.clear();
				continue;
			},
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
