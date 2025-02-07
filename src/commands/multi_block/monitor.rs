use crate::{
	client::Client,
	commands::{multi_block::types::SharedSnapshot, Listen},
	epm::{
		self, check_and_update_target_snapshot, check_and_update_voter_snapshot, fetch_missing_snapshots, submit_and_watch, MultiBlockTransaction
	},
	error::Error,
	helpers::{self, kill_main_task_if_critical_err},
	prelude::{
		runtime, AccountId, Header, Storage, LOG_TARGET
	},
	signer::Signer,
	static_types,
};

use polkadot_sdk::{
	pallet_election_provider_multi_block::{types::Phase, unsigned::miner::MinerConfig},
	sp_npos_elections::ElectionScore,
};
use std::sync::Arc;
use subxt::config::Header as _;
use tokio::sync::Mutex;

/// TODO(niklasad1): Add solver algorithm configuration to the monitor command.
#[derive(Debug, Clone, clap::Parser, PartialEq)]
pub struct MonitorConfig {
	#[clap(long, short, env = "SEED")]
	pub seed_or_path: String,

	#[clap(long, value_enum, default_value_t = Listen::Finalized)]
	pub listen: Listen,
}

pub async fn monitor_cmd<T>(client: Client, config: MonitorConfig) -> Result<(), Error>
where
	T: MinerConfig<AccountId = AccountId> + Send + Sync + 'static,
	T::Solution: Send,
	T::Pages: Send + Sync + 'static,
	T::TargetSnapshotPerBlock: Send + Sync + 'static,
	T::VoterSnapshotPerBlock: Send + Sync + 'static,
	T::MaxVotesPerVoter: Send + Sync + 'static,
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

	let mut subscription = helpers::rpc_block_subscription(client.rpc(), config.listen).await?;
	let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Error>();
	let submit_lock = Arc::new(Mutex::new(()));
	let snapshot = SharedSnapshot::<T>::new(static_types::Pages::get());

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
						subscription = helpers::rpc_block_subscription(client.rpc(), config.listen).await?;
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

		let snapshot = snapshot.clone();
		let signer = signer.clone();
		let client = client.clone();
		let submit_lock = submit_lock.clone();
		let tx = tx.clone();

		tokio::spawn(async move {
			if let Err(e) = process_block(client, at, snapshot, signer, config.listen, submit_lock).await {
				kill_main_task_if_critical_err(&tx, e);
			}
		});
	}
}

/// For each block, monitor essentially does the following:
///
/// 1. Fetch the current storage.
/// 2. Check if the phase is signed/snapshot, otherwise continue with the next block.
/// 3. Check if the solution has already been submitted, if so quit.
/// 4. Fetch the target and voter snapshots.
/// 5. Mine the solution.
/// 6. Lock submissions.
/// 7. Check if the phase is still signed and score is best.
/// 8.. Register the solution score.
/// 9. Submit each page of the solution (one per block)
/// 10. Finally wait for verification.
async fn process_block<T>(
	client: Client,
	at: Header,
	snapshot: SharedSnapshot<T>,
	signer: Signer,
	listen: Listen,
	submit_lock: Arc<Mutex<()>>,
) -> Result<(), Error>
where
	T: MinerConfig<AccountId = AccountId> + Send + Sync + 'static,
	T::Solution: Send,
	T::Pages: Send,
	T::TargetSnapshotPerBlock: Send,
	T::VoterSnapshotPerBlock: Send,
	T::MaxVotesPerVoter: Send,
{
	// 1. Fetch the current storage.
	let storage = helpers::storage_at(Some(at.hash()), client.chain_api()).await?;
	let round = storage.fetch_or_default(&runtime::storage().multi_block().round()).await?;
	let n_pages = static_types::Pages::get();
	let phase = storage
		.fetch_or_default(&runtime::storage().multi_block().current_phase())
		.await?;
	// Target snapshot page (most significant page).
	let target_snapshot_page = n_pages - 1;

	log::trace!(target: LOG_TARGET, "Processing block={} round={}, phase={:?}", at.number, round, phase.0);

	// This will only change after runtime upgrades/when the metadata is changed.
	// but let's be safe and update it every block it's quite cheap.
	snapshot.write().set_page_length(n_pages);

	// 2. Check if the phase is signed/snapshot, otherwise wait for the next block.
	match phase.0 {
		Phase::Snapshot(page) => {
			if page == target_snapshot_page {
				check_and_update_target_snapshot(page, &storage, &snapshot).await?;
			}

			check_and_update_voter_snapshot(page, &storage, &snapshot).await?;

			return Ok(());
		},
		Phase::Signed => {},
		// Ignore other phases.
		_ => {
			snapshot.write().clear();
			return Ok(());
		},
	}

	// 3. If the solution has already been submitted, nothing to do
	if has_submitted::<T>(&storage, round, signer.account_id()).await? {
		return Ok(());
	}

	// 4. Fetch the target and voter snapshots if needed.
	fetch_missing_snapshots::<T>(&snapshot, &storage).await?;

	let desired_targets = storage
		.fetch(&runtime::storage().multi_block().desired_targets())
		.await?
		.unwrap_or(0);
	let target_snapshot = snapshot.read().target.clone().expect("Target snapshot above; qed").0;
	let voter_snapshot = snapshot.read().voter.iter().map(|(_, (v, _))| v.clone()).collect();

	// 5. Lock mining and submission.
	let _guard = submit_lock.lock().await;

	// 6. Mine solution
	let paged_raw_solution =
		epm::mine_solution::<T>(target_snapshot, voter_snapshot, n_pages, round, desired_targets)
			.await?;

	// 7. Because mining is a heavy operation, we need to check if the phase is still signed.
	let storage_head = helpers::storage_at_head(&client, listen).await?;

	if !phase_is_signed(&storage_head).await? {
		return Ok(());
	}

	// 8. Check if our score is better than what's already on the chain
	//    according to the submit strategy.
	if has_submitted::<T>(&storage_head, round, signer.account_id()).await? {
		return Ok(());
	}
		
	if !score_is_best(&storage_head, paged_raw_solution.score, round).await? {
		return Ok(());
	}

	// 9. Register score.
	submit_and_watch::<T>(
		&at,
		&client,
		signer.clone(),
		listen,
		MultiBlockTransaction::register_score(paged_raw_solution.score)?,
	)
	.await?;

	// 10. Submit all solution pages.
	for (page, solution) in paged_raw_solution.solution_pages.into_iter().enumerate() {
		submit_and_watch::<T>(
			&at,
			&client,
			signer.clone(),
			listen,
			MultiBlockTransaction::submit_page::<T>(page as u32, Some(solution))?,
		)
		.await?;
	}

	// 11. Wait for verification.
	// TODO: Implement verification....

	Ok(())
}

/// Whether the current account has already registered a score for the given round.
async fn has_submitted<T: MinerConfig>(
	storage: &Storage,
	round: u32,
	who: &subxt::config::substrate::AccountId32,
) -> Result<bool, Error> {
	let scores = storage.fetch_or_default(&runtime::storage().multi_block_signed().sorted_scores(round))
		.await?;

	if scores.0.into_iter().any(|(account_id, _)| &account_id == who) {
		return Ok(true);
	}

	Ok(false)
}

/// Whether the computed score is better than what is already in the chain.
async fn score_is_best(storage: &Storage, score: ElectionScore, round: u32) -> Result<bool, Error> {
	let scores = storage.fetch_or_default(&runtime::storage().multi_block_signed().sorted_scores(round))
		.await?;

	if scores.0.into_iter().any(|(_, other_score)| other_score.0 > score) {
		return Ok(false);
	}

	Ok(true)
}


async fn phase_is_signed(storage: &Storage) -> Result<bool, Error> {
	let phase = storage.fetch_or_default(&runtime::storage().multi_block().current_phase())
		.await?;

	Ok(phase.0 == Phase::Signed)
}