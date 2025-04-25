use crate::{
    client::Client,
    commands::{
        multi_block::types::{
            BlockDetails, CurrentSubmission, IncompleteSubmission, RoundSubmission,
            RoundTrackingEvent, RoundUntrackingEvent, SharedSnapshot,
        },
        types::{ExperimentalMultiBlockMonitorConfig, Listen, SubmissionStrategy},
    },
    dynamic::multi_block as dynamic,
    error::Error,
    prelude::{AccountId, ExtrinsicParamsBuilder, Storage, LOG_TARGET},
    prometheus,
    runtime::multi_block::{
        self as runtime, runtime_types::pallet_election_provider_multi_block::types::Phase,
    },
    signer::Signer,
    static_types::multi_block as static_types,
    utils::{self, kill_main_task_if_critical_err, score_passes_strategy, TimedFuture},
};
use futures::future::{abortable, AbortHandle};
use polkadot_sdk::{
    pallet_election_provider_multi_block::unsigned::miner::MinerConfig,
    sp_npos_elections::ElectionScore,
};
use std::{collections::HashSet, sync::Arc};
use tokio::sync::{mpsc, Mutex, Notify};

// Round with number of pages to be cleared
type RoundToClear = (u32, u32);

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

    // Always use finalized blocks. We are not supporting best blocks.
    let mut subscription = utils::rpc_block_subscription(client.rpc(), Listen::Finalized).await?;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Error>();
    let submit_lock = Arc::new(Mutex::new(()));
    let snapshot = SharedSnapshot::<T>::new(static_types::Pages::get());
    let mut pending_tasks: Vec<AbortHandle> = Vec::new();
    let mut prev_block_signed_phase = false;

    // State to track submitted rounds
    let submitted_rounds = RoundSubmission::new();

    // Queue size for rounds to clear. Normally we should have just a single round to clear or none.
    // If there are more, it means we have failed to clear for more than an entire round.
    // Dedicated Prometheus metrics for this are in place.
    const ROUND_TO_CLEAR_CHANNEL_SIZE: usize = 10;
    // Communication channel for the clearing task
    let (clear_sender, clear_receiver) = mpsc::channel::<RoundToClear>(ROUND_TO_CLEAR_CHANNEL_SIZE);

    // Notification mechanism to wake the clearing task when new rounds are added
    let clear_notify = Arc::new(Notify::new());

    // Track the current round so we can detect round transitions
    let mut current_round = None;

    // Start the dedicated clearing task
    let client_for_clearing = client.clone();
    let signer_for_clearing = signer.clone();
    let submitted_rounds_for_clearing = submitted_rounds.clone();
    let clear_notify_for_task = clear_notify.clone();

    tokio::spawn(async move {
        if let Err(e) = run_clearing_task(
            client_for_clearing,
            signer_for_clearing,
            clear_receiver,
            submitted_rounds_for_clearing,
            clear_notify_for_task,
        )
        .await
        {
            log::error!(target: LOG_TARGET, "Clearing task failed: {:?}", e);
        }
    });

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
                        log::warn!(target: LOG_TARGET, "subscription to `Finalized` terminated. Retrying..");
                        subscription = utils::rpc_block_subscription(client.rpc(), Listen::Finalized).await?;
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

        let current_block_round = state.round;

        // Detect round transitions - this is where we queue past rounds for clearing
        if let Some(prev_round) = current_round {
            if current_block_round != prev_round {
                // Get all past rounds that need clearing
                let past_rounds = submitted_rounds
                    .get_all_past_rounds(current_block_round)
                    .await;

                // Only send notification if we have rounds to clear
                if !past_rounds.is_empty() {
                    for (past_round, n_pages) in past_rounds {
                        log::info!(
                            target: LOG_TARGET,
                            "Adding round {} to clearing queue (prev round was {})",
                            past_round,
                            prev_round
                        );

                        // Send the round to the clearing task
                        if let Err(e) = clear_sender.send((past_round, n_pages)).await {
                            log::error!(
                                target: LOG_TARGET,
                                "Failed to send round {} to clearing task: {:?}",
                                past_round,
                                e
                            );
                        }
                    }

                    // Only notify once after adding all rounds
                    clear_notify.notify_one();
                }
            }
        }
        current_round = Some(current_block_round);

        // This block handles CURRENT round processing
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

        let snapshot = snapshot.clone();
        let signer = signer.clone();
        let client = client.clone();
        let submit_lock = submit_lock.clone();
        let tx = tx.clone();
        let submitted_rounds = submitted_rounds.clone();

        // Spawn task to potentially mine/submit for the CURRENT round
        let (fut, handle) = abortable(async move {
            if let Err(e) = process_block(
                client,
                state,
                snapshot,
                signer,
                submit_lock,
                config.submission_strategy,
                config.do_reduce,
                config.chunk_size,
                submitted_rounds,
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

/// Dedicated task to clear past submissions
async fn run_clearing_task(
    client: Client,
    signer: Signer,
    mut clear_receiver: mpsc::Receiver<RoundToClear>,
    submitted_rounds: RoundSubmission,
    notify: Arc<Notify>,
) -> Result<(), Error> {
    // Initial queue size update
    prometheus::set_clearing_round_queue_size(clear_receiver.len());

    loop {
        // Process any rounds in the queue
        let mut processed = false;

        while let Ok(round_to_clear) = clear_receiver.try_recv() {
            processed = true;

            process_round_clearing(&client, signer.clone(), round_to_clear, &submitted_rounds)
                .await;

            // Update the queue size metric after each processing
            prometheus::set_clearing_round_queue_size(clear_receiver.len());
        }

        if !processed {
            prometheus::set_clearing_round_queue_size(clear_receiver.len());
        }

        // Wait for notification of a new round to clear
        notify.notified().await;
    }
}

// Helper function to process a single round clearing
async fn process_round_clearing(
    client: &Client,
    signer: Signer,
    round_info: RoundToClear,
    submitted_rounds: &RoundSubmission,
) {
    let (round_to_clear, n_pages) = round_info;

    // Check if the round is still valid before attempting to clear
    match storage_at_finalized(client).await {
        Ok(storage) => {
            match storage
                .fetch(
                    &runtime::storage()
                        .multi_block_signed()
                        .submission_metadata_storage(round_to_clear, signer.account_id()),
                )
                .await
            {
                Ok(maybe_submission_metadata) => {
                    if maybe_submission_metadata.is_none() {
                        log::trace!(
                            target: LOG_TARGET,
                            "Submission metadata for past round {} already gone. Removing from tracking.",
                            round_to_clear
                        );
                        let _ = submitted_rounds
                            .remove(round_to_clear, RoundUntrackingEvent::Cleared)
                            .await;
                        return;
                    }

                    // Submission metadata exists, attempt to clear it
                    match clear_submission(client, signer, round_to_clear, n_pages).await {
                        Ok(_) => {
                            let _ = submitted_rounds
                                .remove(round_to_clear, RoundUntrackingEvent::Cleared)
                                .await;
                        }
                        Err(e) => {
                            log::error!(
                                target: LOG_TARGET,
                                "Failed to clear submission for past round {}: {:?}. Not retrying.",
                                round_to_clear,
                                e
                            );
                        }
                    }
                }
                Err(e) => {
                    log::error!(
                        target: LOG_TARGET,
                        "Error checking submission metadata for round {}: {:?}",
                        round_to_clear,
                        e
                    );

                    prometheus::on_clearing_round_failure();
                }
            }
        }
        Err(e) => {
            log::error!(
                target: LOG_TARGET,
                "Error getting finalized storage: {:?}",
                e
            );

            prometheus::on_clearing_round_failure();
        }
    }
}

/// For each block, the monitor essentially does the following:
///
/// 1. Check if the phase is signed/snapshot, otherwise continue with the next block.
/// 2. Check if the solution has already been submitted, if so quit.
/// 3. Fetch the target and voter snapshots if needed.
/// 4. Mine the solution.
/// 5. Lock submissions.
/// 6. Check if that our score is the best.
/// 7. Register the solution score and submit each page of the solution.
async fn process_block<T>(
    client: Client,
    state: BlockDetails,
    snapshot: SharedSnapshot<T>,
    signer: Signer,
    submit_lock: Arc<Mutex<()>>,
    submission_strategy: SubmissionStrategy,
    do_reduce: bool,
    chunk_size: usize,
    submitted_rounds: RoundSubmission,
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
            dynamic::fetch_missing_snapshots_lossy::<T>(&snapshot, &storage).await?;
            return Ok(());
        }
        Phase::Signed(_) => {}
        // Ignore other phases.
        _ => return Ok(()),
    }

    // 2. Check if the solution has already been submitted (locally or on-chain).
    if is_solution_already_submitted(
        &storage,
        round,
        signer.account_id(),
        n_pages,
        &submitted_rounds,
    )
    .await?
    {
        // Found a complete submission (or recorded an incomplete one found on chain),
        // nothing more to do in this initial check.
        // If it was incomplete, we'll handle potential page submission after mining if needed.
        return Ok(());
    }
    // If is_solution_already_submitted returned false, it means:
    // - No submission found locally or on-chain.
    // - OR an incomplete submission was found on-chain and recorded locally.
    // In either case, we continue processing.

    // 3. Fetch the target and voter snapshots if needed.
    dynamic::fetch_missing_snapshots::<T>(&snapshot, &storage).await?;
    let (target_snapshot, voter_snapshot) = snapshot.read().get();

    // 4. Lock mining and submission.
    let _guard = submit_lock.lock().await;

    // After the submission lock has been acquired, check again
    // that no submissions has been submitted (both locally and on-chain).
    if submitted_rounds.contains(round).await {
        // Round recorded locally, no need to check on-chain
        return Ok(());
    }

    // Always use finalized storage instead of best block
    let storage_after_lock = storage_at_finalized(&client).await?;
    if is_solution_already_submitted(
        &storage_after_lock,
        round,
        signer.account_id(),
        n_pages,
        &submitted_rounds,
    )
    .await?
    {
        return Ok(());
    }

    // 5. Mine solution
    let (paged_raw_solution, mining_duration) = dynamic::mine_solution::<T>(
        target_snapshot,
        voter_snapshot,
        n_pages,
        round,
        desired_targets,
        block_number,
        do_reduce,
    )
    .timed()
    .await;

    let paged_raw_solution = match paged_raw_solution {
        Ok(sol) => {
            log::trace!(target: LOG_TARGET, "Mining solution took {}ms", mining_duration.as_millis());
            prometheus::observe_mined_solution_duration(mining_duration.as_millis() as f64);
            sol
        }
        Err(e) => {
            return Err(e);
        }
    };
    // Capture score before potentially moving paged_raw_solution
    let submitted_score = paged_raw_solution.score;

    // Check again if we have submitted something while mining
    let storage_after_mining = storage_at_finalized(&client).await?;
    match get_submission(&storage_after_mining, round, signer.account_id(), n_pages).await? {
        CurrentSubmission::Done(score) => {
            // We have already submitted the solution with a better score or equal score
            if !score_passes_strategy(submitted_score, score, submission_strategy) {
                log::info!(target: LOG_TARGET, "Mined solution score {:?} not better than already submitted score {:?} for round {}. Skipping.", submitted_score, score, round);
                // Ensure it's recorded locally if somehow missed before
                if !submitted_rounds.contains(round).await {
                    submitted_rounds
                        .insert(round, n_pages, RoundTrackingEvent::Found)
                        .await;
                }
                return Ok(());
            }
            // Revert previous submission and submit the new one.
            log::warn!(target: LOG_TARGET, "Mined solution score {:?} is better than submitted score {:?} for round {}. Bailing previous submission.", submitted_score, score, round);
            bail(&client, signer.clone()).await?;
            // Clear local tracking as we are bailing
            submitted_rounds
                .remove(round, RoundUntrackingEvent::Bailed)
                .await;
        }
        CurrentSubmission::Incomplete(s) => {
            // Submit the missing pages if score matches.
            if s.score() == submitted_score {
                log::info!(target: LOG_TARGET, "Found incomplete submission for round {} with matching score {:?}. Submitting missing pages.", round, submitted_score);
                let mut missing_pages = Vec::new();
                for page in s.get_missing_pages() {
                    // Clone only needed pages
                    if let Some(solution_page) =
                        paged_raw_solution.solution_pages.get(page as usize)
                    {
                        missing_pages.push((page, solution_page.clone()));
                    } else {
                        log::error!(target: LOG_TARGET, "Error: Mined solution missing expected page {} for round {}", page, round);
                        // Handle error appropriately, maybe bail or return error
                        return Err(Error::Other("Mined solution missing expected page".into()));
                    }
                }

                let submission_result = if chunk_size == 0 {
                    dynamic::inner_submit_pages_concurrent::<T>(&client, &signer, missing_pages)
                        .await
                } else {
                    dynamic::inner_submit_pages_chunked::<T>(
                        &client,
                        &signer,
                        missing_pages,
                        chunk_size,
                    )
                    .await
                };

                match submission_result {
                    Ok(failed_pages) if failed_pages.is_empty() => {
                        log::info!(target: LOG_TARGET, "Successfully submitted missing pages for round {}", round);
                        // Ensure complete submission is recorded locally
                        submitted_rounds
                            .insert(round, n_pages, RoundTrackingEvent::SubmittedPartial)
                            .await;
                        return Ok(());
                    }
                    Ok(failed_pages) => {
                        log::error!(target: LOG_TARGET, "Failed to submit some missing pages {:?} for round {}", failed_pages, round);
                        // Don't mark as fully submitted, potentially retry or handle error
                        return Err(Error::Other(format!(
                            "Failed to submit missing pages for round {}",
                            round
                        )));
                    }
                    Err(e) => {
                        log::error!(target: LOG_TARGET, "Error submitting missing pages for round {}: {:?}", round, e);
                        return Err(e);
                    }
                }
            } else {
                // Score mismatch, revert previous submission and submit a new one.
                log::warn!(target: LOG_TARGET, "Mined solution score {:?} differs from incomplete submission score {:?} for round {}. Bailing previous submission.", submitted_score, s.score(), round);
                bail(&client, signer.clone()).await?;
                // Clear local tracking as we are bailing
                submitted_rounds
                    .remove(round, RoundUntrackingEvent::Bailed)
                    .await;
            }
        }
        CurrentSubmission::NotStarted => (), // Continue to submit new solution
    };

    // 6. Check if the score is better than the current best score on chain.
    let storage_before_submit = storage_at_finalized(&client).await?;
    if !score_better(
        &storage_before_submit,
        submitted_score,
        round,
        submission_strategy,
    )
    .await?
    {
        log::info!(target: LOG_TARGET, "Mined solution score {:?} not better than best on chain for round {}. Skipping submission.", submitted_score, round);
        return Ok(());
    }

    prometheus::set_score(submitted_score);

    // 7. Submit the score and solution to the chain.
    match dynamic::submit(
        &client,
        &signer,
        paged_raw_solution,
        Listen::Finalized,
        chunk_size,
    )
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

            // Record successful submission with score
            submitted_rounds
                .insert(round, n_pages, RoundTrackingEvent::Submitted)
                .await;
        }
        (Err(e), _) => {
            submitted_rounds
                .insert(round, n_pages, RoundTrackingEvent::FailedSubmission)
                .await;

            log::error!(target: LOG_TARGET, "Submission failed for round {}: {:?}", round, e);

            // Ensure we DON'T have a potentially incomplete entry if submit fails.
            submitted_rounds
                .remove(round, RoundUntrackingEvent::Failed)
                .await;
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

    // Check if any existing score is better according to the strategy
    if scores
        .0
        .iter()
        .any(|(_, other_score)| !score_passes_strategy(score, other_score.0, strategy))
    {
        log::trace!(target: LOG_TARGET, "Score {:?} is not better than existing scores for round {} based on strategy {:?}", score, round, strategy);
        return Ok(false);
    }

    log::trace!(target: LOG_TARGET, "Score {:?} is better than existing scores for round {} based on strategy {:?}", score, round, strategy);
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
                if submitted {
                    Some(i as u32)
                } else {
                    None
                }
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

/// Check if a solution for this round is already submitted, either locally or on-chain.
/// If found, records it in the local tracking system.
///
/// Returns:
///   - `Ok(true)` if a complete solution is already submitted (should skip further processing)
///   - `Ok(false)` if no solution is found or only an incomplete solution is found (should continue processing)
async fn is_solution_already_submitted(
    storage: &Storage,
    round: u32,
    account_id: &subxt::config::substrate::AccountId32,
    n_pages: u32,
    submitted_rounds: &RoundSubmission,
) -> Result<bool, Error> {
    // Check local tracking first, but we need to distinguish between complete and incomplete
    let submission_state = submitted_rounds.get_state(round).await;

    // If we have a local record AND it's marked as complete, return true
    if let Some(state) = submission_state {
        if state.is_complete() {
            return Ok(true);
        }
        // If it's incomplete, we should continue and check on-chain status
        // in case it's been completed since we last checked
        log::trace!(
            target: LOG_TARGET,
            "Found incomplete local submission for round {}. Checking chain state.",
            round
        );
    }

    // Then check chain state
    match get_submission(storage, round, account_id, n_pages).await? {
        CurrentSubmission::Done(_) => {
            // Found complete submission on chain, record it locally now
            submitted_rounds
                .insert(round, n_pages, RoundTrackingEvent::Found)
                .await;
            return Ok(true);
        }
        CurrentSubmission::Incomplete(_) => {
            // Found incomplete on chain, record locally to prevent full resubmission if score matches
            log::info!(
                target: LOG_TARGET,
                "Found incomplete submission for round {} on-chain, recording locally.",
                round
            );
            submitted_rounds
                .insert(round, n_pages, RoundTrackingEvent::SubmittedPartial)
                .await;
            return Ok(false);
        }
        CurrentSubmission::NotStarted => {
            // No submission found, continue
            return Ok(false);
        }
    }
}

/// Bail out of the current round submission.
async fn bail(client: &Client, signer: Signer) -> Result<(), Error> {
    log::warn!(target: LOG_TARGET, "Bailing out of current submission");
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

    let tx_progress = xt.submit_and_watch().await?;

    // Always use finalized blocks
    match utils::wait_tx_in_block_for_strategy(tx_progress, Listen::Finalized).await {
        Ok(tx_in_block) => {
            log::info!(target: LOG_TARGET, "Successfully bailed in block {:?}", tx_in_block.block_hash());
        }
        Err(e) => {
            log::error!(target: LOG_TARGET, "Failed to bail: {:?}", e);
            return Err(e.into());
        }
    }

    Ok(())
}

/// Clears the miner's submission data for a specific round to reclaim the deposit.
/// This should be called when a previously submitted solution is known to be discarded.
async fn clear_submission(
    client: &Client,
    signer: Signer,
    round_index: u32,
    witness_pages: u32,
) -> Result<(), Error> {
    log::info!(
        target: LOG_TARGET,
        "Attempting to clear submission data for round {} ({} pages)",
        round_index, witness_pages
    );

    // Start timing here for metrics
    let start_time = std::time::Instant::now();

    // Construct the extrinsic call using the static types from the runtime module
    let tx = runtime::tx()
        .multi_block_signed()
        .clear_old_round_data(round_index, witness_pages);

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

    let tx_progress = xt.submit_and_watch().await?;

    match utils::wait_tx_in_block_for_strategy(tx_progress, Listen::Finalized).await {
        Ok(tx_in_block) => {
            // Record the timing metric on success
            let elapsed_ms = start_time.elapsed().as_millis() as f64;
            prometheus::observe_clearing_round_duration(elapsed_ms);

            log::info!(
                target: LOG_TARGET,
                "Successfully cleared submission data for round {} in block {:?} ({:.2}ms)",
                round_index,
                tx_in_block.block_hash(),
                elapsed_ms
            );
            Ok(())
        }
        Err(e) => {
            // Record the failure metric
            prometheus::on_clearing_round_failure();

            log::error!(
                target: LOG_TARGET,
                "Failed to clear submission data for round {}: {:?}",
                round_index,
                e
            );
            return Err(e.into());
        }
    }
}

/// Helper function to get storage at finalized head
async fn storage_at_finalized(client: &Client) -> Result<Storage, Error> {
    let finalized_hash = client.rpc().chain_get_finalized_head().await?;
    utils::storage_at(Some(finalized_hash), client.chain_api()).await
}
