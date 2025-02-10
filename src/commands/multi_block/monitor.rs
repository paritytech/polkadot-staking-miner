use crate::{
	client::Client,
	commands::{
		multi_block::types::{BlockDetails, SharedSnapshot},
		Listen, SubmissionStrategy,
	},
	epm::{
		self, check_and_update_target_snapshot, check_and_update_voter_snapshot,
		fetch_missing_snapshots, submit_and_watch, MultiBlockTransaction,
	},
	error::Error,
	helpers::{self, kill_main_task_if_critical_err, score_passes_strategy, storage_at_head},
	prelude::{runtime, AccountId, Storage, LOG_TARGET},
	signer::Signer,
	static_types,
};
use futures::future::{abortable, AbortHandle};
use polkadot_sdk::{
	pallet_election_provider_multi_block::{types::Phase, unsigned::miner::MinerConfig},
	sp_npos_elections::ElectionScore,
};
use std::sync::Arc;
use tokio::sync::Mutex;

/// TODO(niklasad1): Add solver algorithm configuration to the monitor command.
#[derive(Debug, Clone, clap::Parser)]
pub struct MonitorConfig {
	#[clap(long, short, env = "SEED")]
	pub seed_or_path: String,

	#[clap(long, value_enum, default_value_t = Listen::Finalized)]
	pub listen: Listen,

	#[clap(long, value_parser, default_value = "if-leading")]
	pub submission_strategy: SubmissionStrategy,
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
	let mut subscription = helpers::rpc_block_subscription(client.rpc(), config.listen).await?;
	let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Error>();
	let submit_lock = Arc::new(Mutex::new(()));
	let snapshot = SharedSnapshot::<T>::new(static_types::Pages::get());
	let mut pending_tasks: Vec<AbortHandle> = Vec::new();
	let mut prev_block_signed_phase = false;

	log::info!(target: LOG_TARGET, "Loaded account {} {{ nonce: {}, free_balance: {}, reserved_balance: {}, frozen_balance: {} }}",
		signer,
		account_info.nonce,
		account_info.data.free,
		account_info.data.reserved,
		account_info.data.frozen,
	);

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

		let state = BlockDetails::new(&client, at).await?;

		if !state.phase_is_signed() && !state.phase_is_snapshot() {
			// Signal to pending mining task the sign phase has ended.
			for stop in pending_tasks.drain(..) {
				stop.abort();
			}

			// Clear snapshot cache
			if prev_block_signed_phase {
				snapshot.write().clear();
				prev_block_signed_phase = false;
			}

			continue;
		}

		prev_block_signed_phase = true;
		let snapshot = snapshot.clone();
		let signer = signer.clone();
		let client = client.clone();
		let submit_lock = submit_lock.clone();
		let tx = tx.clone();
		let (fut, handle) = abortable(async move {
			if let Err(e) = process_block(
				client,
				state,
				snapshot,
				signer,
				config.listen,
				submit_lock,
				config.submission_strategy,
			)
			.await
			{
				kill_main_task_if_critical_err(&tx, e);
			}
		});

		tokio::spawn(fut);
		pending_tasks.push(handle);
	}
}

/// For each block, monitor essentially does the following:
///
/// 1. Check if the phase is signed/snapshot, otherwise continue with the next block.
/// 2. Check if the solution has already been submitted, if so quit.
/// 3. Fetch the target and voter snapshots if not already in the cache.
/// 4. Mine the solution.
/// 5. Lock submissions.
/// 6. Check if that our score is the best.
/// 7. Register the solution score.
/// 8. Submit each page of the solution (one per block)
async fn process_block<T>(
	client: Client,
	state: BlockDetails,
	snapshot: SharedSnapshot<T>,
	signer: Signer,
	listen: Listen,
	submit_lock: Arc<Mutex<()>>,
	submission_strategy: SubmissionStrategy,
) -> Result<(), Error>
where
	T: MinerConfig<AccountId = AccountId> + Send + Sync + 'static,
	T::Solution: Send,
	T::Pages: Send,
	T::TargetSnapshotPerBlock: Send,
	T::VoterSnapshotPerBlock: Send,
	T::MaxVotesPerVoter: Send,
{
	let BlockDetails {
		storage, phase, round, n_pages, target_snapshot_page, desired_targets, ..
	} = state;

	// This will only change after runtime upgrades/when the metadata is changed.
	// but let's be safe and update it every block it's quite cheap.
	snapshot.write().set_page_length(n_pages);

	// 1. Check if the phase is signed/snapshot, otherwise wait for the next block.
	match phase {
		Phase::Snapshot(page) => {
			if page == target_snapshot_page {
				check_and_update_target_snapshot(page, &storage, &snapshot).await?;
			}

			check_and_update_voter_snapshot(page, &storage, &snapshot).await?;

			return Ok(());
		},
		Phase::Signed => {},
		// Ignore other phases.
		_ => return Ok(()),
	}

	// 2. If the solution has already been submitted, nothing to do
	if has_submitted::<T>(&storage, round, signer.account_id()).await? {
		return Ok(());
	}

	// 3. Fetch the target and voter snapshots if needed.
	fetch_missing_snapshots::<T>(&snapshot, &storage).await?;

	let target_snapshot = snapshot.read().target.clone().expect("Target snapshot above; qed").0;
	let voter_snapshot = snapshot.read().voter.iter().map(|(_, (v, _))| v.clone()).collect();

	// 4. Lock mining and submission.
	let _guard = submit_lock.lock().await;

	// After the submission lock has been acquired, check again
	// that no submissions has been submitted.
	if has_submitted::<T>(&storage_at_head(&client, listen).await?, round, signer.account_id())
		.await?
	{
		return Ok(());
	}

	// 5. Mine solution
	let paged_raw_solution =
		epm::mine_solution::<T>(target_snapshot, voter_snapshot, n_pages, round, desired_targets)
			.await?;

	// 6. Check that our score is the best (we could also check if our miner hasn't submitted yet)
	let storage_head = helpers::storage_at_head(&client, listen).await?;

	if !score_is_best(&storage_head, paged_raw_solution.score, round, submission_strategy).await? {
		return Ok(());
	}

	// 7. Register score.
	submit_and_watch::<T>(
		&client,
		signer.clone(),
		listen,
		MultiBlockTransaction::register_score(paged_raw_solution.score)?,
	)
	.await?;

	// 8. Submit all solution pages.
	for (page, solution) in paged_raw_solution.solution_pages.into_iter().enumerate() {
		submit_and_watch::<T>(
			&client,
			signer.clone(),
			listen,
			MultiBlockTransaction::submit_page::<T>(page as u32, Some(solution))?,
		)
		.await?;
	}

	// TODO: maybe check the verification events.

	Ok(())
}

/// Whether the current account has already registered a score for the given round.
async fn has_submitted<T: MinerConfig>(
	storage: &Storage,
	round: u32,
	who: &subxt::config::substrate::AccountId32,
) -> Result<bool, Error> {
	let scores = storage
		.fetch_or_default(&runtime::storage().multi_block_signed().sorted_scores(round))
		.await?;

	if scores.0.into_iter().any(|(account_id, _)| &account_id == who) {
		return Ok(true);
	}

	Ok(false)
}

/// Whether the computed score is better than what is already in the chain.
async fn score_is_best(
	storage: &Storage,
	score: ElectionScore,
	round: u32,
	strategy: SubmissionStrategy,
) -> Result<bool, Error> {
	let scores = storage
		.fetch_or_default(&runtime::storage().multi_block_signed().sorted_scores(round))
		.await?;

	if scores
		.0
		.into_iter()
		.any(|(_, other_score)| !score_passes_strategy(score, other_score.0, strategy))
	{
		return Ok(false);
	}

	Ok(true)
}
