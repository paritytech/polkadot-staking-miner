use crate::{
    client::Client,
    commands::{
        multi_block::types::{
            BlockDetails, CurrentSubmission, IncompleteSubmission, SharedSnapshot,
        },
        types::{ExperimentalMultiBlockMonitorConfig, Listen, SubmissionStrategy},
    },
    dynamic::multi_block as dynamic,
    error::Error,
    prelude::{AccountId, ExtrinsicParamsBuilder, LOG_TARGET, Storage},
    prometheus,
    runtime::multi_block::{
        self as runtime, runtime_types::pallet_election_provider_multi_block::types::Phase,
    },
    signer::Signer,
    static_types::multi_block as static_types,
    utils::{self, TimedFuture, kill_main_task_if_critical_err, score_passes_strategy},
};
use futures::future::{AbortHandle, abortable};
use polkadot_sdk::{
    pallet_election_provider_multi_block::unsigned::miner::MinerConfig,
    sp_npos_elections::ElectionScore,
};
use std::{collections::HashSet, sync::Arc};
use tokio::sync::Mutex;

pub async fn monitor_cmd<T>(
    client: Client,
    config: ExperimentalMultiBlockMonitorConfig,
) -> Result<(), Error>
where
    T: MinerConfig<AccountId = AccountId> + Send + Sync + 'static,
    T::Solution: Send + Sync + 'static,
    T::Pages: Send + Sync + 'static,
    T::TargetSnapshotPerBlock: Send + Sync + 'static,
    T::VoterSnapshotPerBlock: Send + Sync + 'static,
    T::MaxVotesPerVoter: Send + Sync + 'static,
{
    let signer = Signer::new(&config.seed_or_path)?;

    // Emit the account info at the start.
    {
        let account_info = client
            .chain_api()
            .storage()
            .at_latest()
            .await?
            .fetch(&runtime::storage().system().account(signer.account_id()))
            .await?
            .ok_or(Error::AccountDoesNotExists)?;
        prometheus::set_balance(account_info.data.free as f64);

        log::info!(
            target: LOG_TARGET,
            "Loaded account {} {{ nonce: {}, free_balance: {}, reserved_balance: {}, frozen_balance: {} }}",
            signer,
            account_info.nonce,
            account_info.data.free,
            account_info.data.reserved,
            account_info.data.frozen,
        );
    }

    let mut subscription = utils::rpc_block_subscription(client.rpc(), config.listen).await?;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Error>();
    let submit_lock = Arc::new(Mutex::new(()));
    let snapshot = SharedSnapshot::<T>::new(static_types::Pages::get());
    let mut pending_tasks: Vec<AbortHandle> = Vec::new();
    let mut prev_block_signed_phase = false;

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
                        subscription = utils::rpc_block_subscription(client.rpc(), config.listen).await?;
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
        let account_info = state
            .storage
            .fetch(&runtime::storage().system().account(signer.account_id()))
            .await?
            .ok_or(Error::AccountDoesNotExists)?;
        prometheus::set_balance(account_info.data.free as f64);

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
                config.do_reduce,
                config.chunk_size,
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

/// For each block, the monitor essentially does the following:
///
/// 1. Check if the phase is signed/snapshot, otherwise continue with the next block.
/// 2. Check if the solution has already been submitted, if so quit.
/// 3. Fetch the target and voter snapshots if not already in the cache.
/// 4. Mine the solution.
/// 5. Lock submissions.
/// 6. Check if that our score is the best.
/// 7. Register the solution score and submit each page of the solution.
async fn process_block<T>(
    client: Client,
    state: BlockDetails,
    snapshot: SharedSnapshot<T>,
    signer: Signer,
    listen: Listen,
    submit_lock: Arc<Mutex<()>>,
    submission_strategy: SubmissionStrategy,
    do_reduce: bool,
    chunk_size: usize,
) -> Result<(), Error>
where
    T: MinerConfig<AccountId = AccountId> + Send + Sync + 'static,
    T::Solution: Send + Sync + 'static,
    T::Pages: Send + Sync + 'static,
    T::TargetSnapshotPerBlock: Send,
    T::VoterSnapshotPerBlock: Send,
    T::MaxVotesPerVoter: Send + Sync + 'static,
{
    let BlockDetails {
        storage,
        phase,
        round,
        n_pages,
        desired_targets,
        block_number,
        ..
    } = state;

    // This will only change after runtime upgrades/when the metadata is changed.
    // but let's be on the safe-side and update it every block.
    snapshot.write().set_page_length(n_pages);

    // 1. Check if the phase is signed/snapshot, otherwise wait for the next block.
    match phase {
        Phase::Snapshot(_) => {
            dynamic::fetch_missing_snapshots_lossy::<T>(&snapshot, &storage, round).await?;
            return Ok(());
        }
        Phase::Signed(_) => {}
        // Ignore other phases.
        _ => return Ok(()),
    }

    // 2. If the solution has already been submitted, nothing to do
    if has_submitted(&storage, round, signer.account_id(), n_pages).await? {
        return Ok(());
    }

    // 3. Fetch the target and voter snapshots if needed.
    dynamic::fetch_missing_snapshots::<T>(&snapshot, &storage, round).await?;
    let (target_snapshot, voter_snapshot) = snapshot.read().get();

    // 4. Lock mining and submission.
    let _guard = submit_lock.lock().await;

    // After the submission lock has been acquired, check again
    // that no submissions has been submitted.
    if has_submitted(
        &utils::storage_at_head(&client, listen).await?,
        round,
        signer.account_id(),
        n_pages,
    )
    .await?
    {
        return Ok(());
    }

    // 5. Mine solution
    let paged_raw_solution = match dynamic::mine_solution::<T>(
        target_snapshot,
        voter_snapshot,
        n_pages,
        round,
        desired_targets,
        block_number,
        do_reduce,
    )
    .timed()
    .await
    {
        (Ok(sol), dur) => {
            log::trace!(target: LOG_TARGET, "Mining solution took {}ms", dur.as_millis());
            prometheus::observe_mined_solution_duration(dur.as_millis() as f64);
            sol
        }
        (Err(e), _) => {
            return Err(e);
        }
    };

    let storage_head = utils::storage_at_head(&client, listen).await?;

    match get_submission(&storage_head, round, signer.account_id(), n_pages).await? {
        CurrentSubmission::Done(score) => {
            // We have already submitted the solution with a better score or equal score
            if !score_passes_strategy(paged_raw_solution.score, score, submission_strategy) {
                return Ok(());
            }
            // Revert previous submission and submit the new one.
            bail(listen, &client, signer.clone()).await?;
        }
        CurrentSubmission::Incomplete(s) => {
            // Submit the missing pages.
            if s.score() == paged_raw_solution.score {
                let mut missing_pages = Vec::new();

                for page in s.get_missing_pages() {
                    let solution = paged_raw_solution.solution_pages[page as usize].clone();
                    missing_pages.push((page, solution));
                }

                // Use the appropriate submission method based on chunk_size
                if chunk_size == 0 {
                    dynamic::inner_submit_pages_concurrent::<T>(
                        &client,
                        &signer,
                        missing_pages,
                        listen,
                    )
                    .await?;
                } else {
                    dynamic::inner_submit_pages_chunked::<T>(
                        &client,
                        &signer,
                        missing_pages,
                        listen,
                        chunk_size,
                    )
                    .await?;
                }
                return Ok(());
            }

            // Revert previous submission and submit a new one.
            bail(listen, &client, signer.clone()).await?;
        }
        CurrentSubmission::NotStarted => (),
    };

    // 6. Check if the score is better than the current best score.
    //
    // We allow overwriting the score if the "our account" has the best score but hasn't submitted
    // the solution. This is to allow the miner to re-submit the score and solution if the miner crashed
    // or the RPC connection was lost.
    //
    // This to ensure that the miner doesn't miss out on submitting the solution if the miner crashed
    // and to prevent to be slashed.

    if !score_better(
        &storage_head,
        paged_raw_solution.score,
        round,
        submission_strategy,
    )
    .await?
    {
        return Ok(());
    }

    prometheus::set_score(paged_raw_solution.score);

    // 7. Submit the score and solution to the chain.
    match dynamic::submit(&client, &signer, paged_raw_solution, listen, chunk_size)
        .timed()
        .await
    {
        (Ok(_), dur) => {
            log::trace!(
                target: LOG_TARGET,
                "Register score and solution pages took {}ms",
                dur.as_millis()
            );
            prometheus::observe_submit_and_watch_duration(dur.as_millis() as f64);
        }
        (Err(e), _) => {
            return Err(e);
        }
    };

    Ok(())
}

/// Whether the computed score is better than the current best score
async fn score_better(
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

/// Whether the current account has registered the score and submitted all pages for the given round.
async fn get_submission(
    storage: &Storage,
    round: u32,
    who: &subxt::config::substrate::AccountId32,
    n_pages: u32,
) -> Result<CurrentSubmission, Error> {
    let maybe_submission = storage
        .fetch(
            &runtime::storage()
                .multi_block_signed()
                .submission_metadata_storage(round, who),
        )
        .await?;

    let Some(submission) = maybe_submission else {
        return Ok(CurrentSubmission::NotStarted);
    };

    let pages: HashSet<u32> = submission
        .pages
        .0
        .into_iter()
        .enumerate()
        .filter_map(
            |(i, submitted)| {
                if submitted { Some(i as u32) } else { None }
            },
        )
        .collect();

    if pages.len() == n_pages as usize {
        Ok(CurrentSubmission::Done(submission.claimed_score.0))
    } else {
        Ok(CurrentSubmission::Incomplete(IncompleteSubmission::new(
            submission.claimed_score.0,
            pages,
            n_pages,
        )))
    }
}

/// Whether the current account has registered the score and submitted all pages for the given round.
async fn has_submitted(
    storage: &Storage,
    round: u32,
    who: &subxt::config::substrate::AccountId32,
    n_pages: u32,
) -> Result<bool, Error> {
    match get_submission(storage, round, who, n_pages).await? {
        CurrentSubmission::Done(_) => Ok(true),
        _ => Ok(false),
    }
}

async fn bail(listen: Listen, client: &Client, signer: Signer) -> Result<(), Error> {
    let tx = runtime::tx().multi_block_signed().bail();

    let nonce = client
        .rpc()
        .system_account_next_index(signer.account_id())
        .await?;

    let xt_cfg = ExtrinsicParamsBuilder::default().nonce(nonce).build();
    let xt = client
        .chain_api()
        .tx()
        .create_signed(&tx, &*signer, xt_cfg)
        .await?;

    let tx = xt.submit_and_watch().await?;

    utils::wait_tx_in_block_for_strategy(tx, listen).await?;

    Ok(())
}
