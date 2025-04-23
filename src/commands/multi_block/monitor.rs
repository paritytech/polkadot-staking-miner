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
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tokio::sync::Mutex;

/// Enum representing different tracking events for rounds
#[derive(Debug, Clone, Copy)]
enum RoundTrackingEvent {
    Found,            // Found existing submission on-chain
    Submitted,        // Successfully submitted a new solution
    SubmittedPartial, // Submitted missing pages for partial solution
    Cleared,          // Successfully cleared a discarded submission
    FailedSubmission, // Failed to submit a solution
}

/// Enum representing different untracking events for rounds
#[derive(Debug, Clone, Copy)]
enum RoundUntrackingEvent {
    Cleared, // Removed after clearing
    Bailed,  // Removed after bailing
    Failed,  // Removed after a failed submission
}

/// A dedicated type to manage submitted rounds tracking with clean APIs
/// that hide the mutex implementation details
#[derive(Clone)]
struct RoundSubmission(Arc<Mutex<HashMap<u32, (u32, ElectionScore)>>>);

impl RoundSubmission {
    /// Create a new RoundSubmission tracker
    fn new() -> Self {
        Self(Arc::new(Mutex::new(HashMap::new())))
    }

    /// Check if a round is being tracked
    async fn contains(&self, round: u32) -> bool {
        self.0.lock().await.contains_key(&round)
    }

    /// Insert a new round submission into the tracking map
    async fn insert(
        &self,
        round: u32,
        n_pages: u32,
        score: ElectionScore,
        event: Option<RoundTrackingEvent>,
    ) {
        let mut rounds = self.0.lock().await;
        rounds.insert(round, (n_pages, score));

        if let Some(event) = event {
            let message = match event {
                RoundTrackingEvent::Found => "Found submission for",
                RoundTrackingEvent::Submitted => "Successfully submitted and recorded",
                RoundTrackingEvent::SubmittedPartial => "Completed partial submission for",
                RoundTrackingEvent::Cleared => "Cleared discarded submission for",
                RoundTrackingEvent::FailedSubmission => "Failed submission for",
            };

            log::info!(
                target: LOG_TARGET,
                "{} round {} ({} pages, score {:?})",
                message,
                round,
                n_pages,
                score
            );
        }
    }

    /// Remove a round submission from the tracking map
    async fn remove(&self, round: u32, event: Option<RoundUntrackingEvent>) -> bool {
        let mut rounds = self.0.lock().await;
        let removed = rounds.remove(&round).is_some();

        if removed && event.is_some() {
            let message = match event.unwrap() {
                RoundUntrackingEvent::Cleared => "Removed after clearing submission for",
                RoundUntrackingEvent::Bailed => "Removed after bailing from",
                RoundUntrackingEvent::Failed => "Removed failed submission for",
            };

            log::info!(
                target: LOG_TARGET,
                "{} round {}",
                message,
                round
            );
        }

        removed
    }

    /// Collect past rounds that should be cleared
    async fn collect_past_rounds(&self, current_block_round: u32) -> Vec<(u32, u32)> {
        let rounds = self.0.lock().await;

        rounds
            .iter()
            .filter_map(|(&round_to_check, &(n_pages, _))| {
                if round_to_check < current_block_round {
                    Some((round_to_check, n_pages))
                } else {
                    None
                }
            })
            .collect()
    }
}

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

    // State to track submitted rounds, their page count, and score
    let submitted_rounds = RoundSubmission::new();

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

        // This checking logic runs on EVERY block
        let rounds_to_potentially_clear = submitted_rounds
            .collect_past_rounds(current_block_round)
            .await;

        check_and_clear_discarded_submissions(
            &client,
            &signer,
            rounds_to_potentially_clear,
            &submitted_rounds,
            &state.storage,
        )
        .await?;

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

    // 2. If the solution has already been submitted:
    // 2.1 Check local tracking first
    if submitted_rounds.contains(round).await {
        // Already submitted locally, no need to check on-chain
        return Ok(());
    }
    // 2.2 Then check chain state
    match get_submission(&storage, round, signer.account_id(), n_pages).await? {
        CurrentSubmission::Done(score_on_chain) => {
            // Found on chain but not locally, record it locally now.
            submitted_rounds
                .insert(
                    round,
                    n_pages,
                    score_on_chain,
                    Some(RoundTrackingEvent::Found),
                )
                .await;
            return Ok(());
        }
        CurrentSubmission::Incomplete(incomplete_sub) => {
            // Found incomplete on chain, record locally to prevent full resubmission if score matches
            log::info!(target: LOG_TARGET, "Found incomplete submission for round {} on-chain, recording locally.", round);
            submitted_rounds
                .insert(
                    round,
                    n_pages,
                    incomplete_sub.score(),
                    Some(RoundTrackingEvent::SubmittedPartial),
                )
                .await;
            // Continue to potentially submit missing pages below...
        }
        CurrentSubmission::NotStarted => { /* Continue */ }
    }

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
    match get_submission(&storage_after_lock, round, signer.account_id(), n_pages).await? {
        CurrentSubmission::Done(score_on_chain) => {
            log::info!(target: LOG_TARGET, "Found submission for round {} on-chain after acquiring lock, recording locally.", round);
            submitted_rounds
                .insert(
                    round,
                    n_pages,
                    score_on_chain,
                    Some(RoundTrackingEvent::Found),
                )
                .await;
            return Ok(());
        }
        CurrentSubmission::Incomplete(incomplete_sub) => {
            log::info!(target: LOG_TARGET, "Found incomplete submission for round {} on-chain after acquiring lock, recording locally.", round);
            submitted_rounds
                .insert(
                    round,
                    n_pages,
                    incomplete_sub.score(),
                    Some(RoundTrackingEvent::SubmittedPartial),
                )
                .await;
            // Continue...
        }
        CurrentSubmission::NotStarted => { /* Continue */ }
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
                    submitted_rounds.insert(round, n_pages, score, None).await;
                }
                return Ok(());
            }
            // Revert previous submission and submit the new one.
            log::warn!(target: LOG_TARGET, "Mined solution score {:?} is better than submitted score {:?} for round {}. Bailing previous submission.", submitted_score, score, round);
            bail(Listen::Finalized, &client, signer.clone()).await?;
            // Clear local tracking as we are bailing
            submitted_rounds
                .remove(round, Some(RoundUntrackingEvent::Bailed))
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
                    dynamic::inner_submit_pages_concurrent::<T>(
                        &client,
                        &signer,
                        missing_pages,
                        Listen::Finalized,
                    )
                    .await
                } else {
                    dynamic::inner_submit_pages_chunked::<T>(
                        &client,
                        &signer,
                        missing_pages,
                        Listen::Finalized,
                        chunk_size,
                    )
                    .await
                };

                match submission_result {
                    Ok(failed_pages) if failed_pages.is_empty() => {
                        log::info!(target: LOG_TARGET, "Successfully submitted missing pages for round {}", round);
                        // Ensure complete submission is recorded locally
                        submitted_rounds
                            .insert(
                                round,
                                n_pages,
                                submitted_score,
                                Some(RoundTrackingEvent::SubmittedPartial),
                            )
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
                bail(Listen::Finalized, &client, signer.clone()).await?;
                // Clear local tracking as we are bailing
                submitted_rounds
                    .remove(round, Some(RoundUntrackingEvent::Bailed))
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
                .insert(
                    round,
                    n_pages,
                    submitted_score,
                    Some(RoundTrackingEvent::Submitted),
                )
                .await;
        }
        (Err(e), _) => {
            submitted_rounds
                .insert(
                    round,
                    n_pages,
                    submitted_score,
                    Some(RoundTrackingEvent::FailedSubmission),
                )
                .await;

            log::error!(target: LOG_TARGET, "Submission failed for round {}: {:?}", round, e);

            // Ensure we DON'T have a potentially incomplete entry if submit fails.
            submitted_rounds
                .remove(round, Some(RoundUntrackingEvent::Failed))
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

/// Bail out of the current round submission.
async fn bail(listen: Listen, client: &Client, signer: Signer) -> Result<(), Error> {
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

    match utils::wait_tx_in_block_for_strategy(tx_progress, listen).await {
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
    listen: Listen,
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

    match utils::wait_tx_in_block_for_strategy(tx_progress, listen).await {
        Ok(tx_in_block) => {
            log::info!(
                target: LOG_TARGET,
                "Successfully cleared submission data for round {} in block {:?}",
                round_index,
                tx_in_block.block_hash()
            );
            // Optionally check events here if needed, e.g., for deposit reclaimed event
        }
        Err(e) => {
            log::error!(
                target: LOG_TARGET,
                "Failed to clear submission data for round {}: {:?}. Will retry.",
                round_index,
                e
            );
            // Return the error, the caller might want to retry or log differently
            return Err(e.into());
        }
    }

    Ok(())
}

async fn check_and_clear_discarded_submissions(
    client: &Client,
    signer: &Signer,
    rounds_to_potentially_clear: Vec<(u32, u32)>,
    submitted_rounds: &RoundSubmission,
    storage: &Storage,
) -> Result<(), Error> {
    if rounds_to_potentially_clear.is_empty() {
        return Ok(());
    }

    log::debug!(
        target: LOG_TARGET,
        "Checking status of PAST rounds: {:?}",
        rounds_to_potentially_clear.iter().map(|(r,_)| r).collect::<Vec<_>>()
    );

    let mut rounds_to_remove = Vec::new();

    for (round_to_check, n_pages) in rounds_to_potentially_clear {
        let maybe_submission_metadata = storage
            .fetch(
                &runtime::storage()
                    .multi_block_signed()
                    .submission_metadata_storage(round_to_check, signer.account_id()),
            )
            .await?;

        if maybe_submission_metadata.is_none() {
            log::debug!(target: LOG_TARGET, "Submission metadata for past round {} gone. Removing from tracking.", round_to_check);
            rounds_to_remove.push(round_to_check);
            continue;
        }

        // For past rounds, we assume they've already progressed beyond active phases
        // If submission metadata still exists, it needs clearing
        let should_clear = true;
        let clear_reason = "Past round with still existing submission metadata";

        if should_clear {
            log::info!(target: LOG_TARGET, "Submission for past round {} detected as DISCARDED (Reason: {}). Attempting to clear.", round_to_check, clear_reason);
            match clear_submission(
                Listen::Finalized,
                client,
                signer.clone(),
                round_to_check,
                n_pages,
            )
            .await
            {
                Ok(_) => {
                    log::info!(target: LOG_TARGET, "Successfully cleared submission for past round {}", round_to_check);
                    submitted_rounds
                        .insert(
                            round_to_check,
                            n_pages,
                            ElectionScore::default(), // We don't have the score here, use default
                            Some(RoundTrackingEvent::Cleared),
                        )
                        .await;
                    rounds_to_remove.push(round_to_check);
                }
                Err(e) => {
                    log::error!(target: LOG_TARGET, "Failed to clear submission for past round {}: {:?}. Will retry.", round_to_check, e);
                }
            }
        }
    }

    // Remove successfully cleared rounds from tracking
    for round_index in rounds_to_remove {
        submitted_rounds
            .remove(round_index, Some(RoundUntrackingEvent::Cleared))
            .await;
    }

    Ok(())
}

/// Helper function to get storage at finalized head
async fn storage_at_finalized(client: &Client) -> Result<Storage, Error> {
    let finalized_hash = client.rpc().chain_get_finalized_head().await?;
    utils::storage_at(Some(finalized_hash), client.chain_api()).await
}
