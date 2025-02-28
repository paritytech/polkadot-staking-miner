use crate::{
    client::Client,
    commands::{
        multi_block::types::{BlockDetails, SharedSnapshot},
        types::{Listen, SubmissionStrategy},
    },
    dynamic,
    error::Error,
    prelude::{runtime, AccountId, ExtrinsicParamsBuilder, Storage, LOG_TARGET},
    prometheus,
    signer::Signer,
    static_types,
    utils::{
        self, kill_main_task_if_critical_err, score_passes_strategy, storage_at_head, TimedFuture,
    },
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
#[cfg_attr(test, derive(PartialEq))]
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

    // Emit the account info at the start.
    {
        let storage = client.chain_api().storage().at_latest().await?;
        let account_info = utils::account_info(&storage, signer.account_id()).await?;

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
        let account_info = utils::account_info(&state.storage, signer.account_id()).await?;
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
/// 7. Register the solution score and submit each page of the solution (one per block)
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
        storage,
        phase,
        round,
        n_pages,
        target_snapshot_page,
        desired_targets,
        ..
    } = state;

    // This will only change after runtime upgrades/when the metadata is changed.
    // but let's be on the safe-side and update it every block.
    snapshot.write().set_page_length(n_pages);

    // 1. Check if the phase is signed/snapshot, otherwise wait for the next block.
    match phase {
        Phase::Snapshot(page) => {
            if page == target_snapshot_page {
                dynamic::check_and_update_target_snapshot(page, &storage, &snapshot).await?;
            }

            dynamic::check_and_update_voter_snapshot(page, &storage, &snapshot).await?;

            return Ok(());
        }
        Phase::Signed => {}
        // Ignore other phases.
        _ => return Ok(()),
    }

    // 2. If the solution has already been submitted, nothing to do
    if has_submitted(&storage, round, signer.account_id(), n_pages).await? {
        return Ok(());
    }

    // 3. Fetch the target and voter snapshots if needed.
    dynamic::fetch_missing_snapshots::<T>(&snapshot, &storage).await?;
    let (target_snapshot, voter_snapshot) = snapshot.read().get();

    // 4. Lock mining and submission.
    let _guard = submit_lock.lock().await;

    // After the submission lock has been acquired, check again
    // that no submissions has been submitted.
    if has_submitted(
        &storage_at_head(&client, listen).await?,
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

    // 6. Check if the score is better than the current best score.
    //
    // We allow overwriting the score if the "our account" has the best score but hasn't submitted
    // the solution. This is to allow the miner to re-submit the score and solution if the miner crashed
    // or the RPC connection was lost.
    //
    // This to ensure that the miner doesn't miss out on submitting the solution if the miner crashed
    // and to prevent to be slashed.
    if !own_score_or_better(
        &storage_head,
        paged_raw_solution.score,
        round,
        submission_strategy,
        signer.account_id(),
    )
    .await?
    {
        return Ok(());
    }

    prometheus::set_score(paged_raw_solution.score);

    // De-register the score and solution if the miner has already submitted the score.
    if has_submitted_score(&storage_head, round, signer.account_id()).await? {
        bail(listen, &client, signer.clone()).await?;
    }

    // 7. Submit the score and solution to the chain.
    match dynamic::submit(&client, &signer, paged_raw_solution, listen)
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

    // TODO(niklasad1): check events that the submissions were successful and
    // the verification was successful.

    Ok(())
}

/// Whether the current account has already registered a score for the given round.
async fn has_submitted_score(
    storage: &Storage,
    round: u32,
    who: &subxt::config::substrate::AccountId32,
) -> Result<bool, Error> {
    let scores = storage
        .fetch_or_default(&runtime::storage().multi_block_signed().sorted_scores(round))
        .await?;

    if scores
        .0
        .into_iter()
        .any(|(account_id, _)| &account_id == who)
    {
        return Ok(true);
    }

    Ok(false)
}

/// Whether the computed score is better than the current best score
/// or that the current account has the best score.
async fn own_score_or_better(
    storage: &Storage,
    score: ElectionScore,
    round: u32,
    strategy: SubmissionStrategy,
    who: &subxt::config::substrate::AccountId32,
) -> Result<bool, Error> {
    let scores = storage
        .fetch_or_default(&runtime::storage().multi_block_signed().sorted_scores(round))
        .await?;

    if scores
        .0
        .into_iter()
        .filter(|(account_id, _)| account_id != who)
        .any(|(_, other_score)| !score_passes_strategy(score, other_score.0, strategy))
    {
        return Ok(false);
    }

    Ok(true)
}

/// Whether the current account has submitted all pages for the given round.
async fn all_solution_pages_submitted(
    storage: &Storage,
    round: u32,
    who: &subxt::config::substrate::AccountId32,
    n_pages: u32,
) -> Result<bool, Error> {
    for page in 0..n_pages {
        let page = storage
            .fetch(
                &runtime::storage()
                    .multi_block_signed()
                    .submission_storage(round, who, page),
            )
            .await?;

        if page.is_none() {
            return Ok(false);
        }
    }

    Ok(true)
}

/// Whether the current account has registered the score and submitted all pages for the given round.
async fn has_submitted(
    storage: &Storage,
    round: u32,
    who: &subxt::config::substrate::AccountId32,
    n_pages: u32,
) -> Result<bool, Error> {
    if !has_submitted_score(storage, round, who).await? {
        return Ok(false);
    }

    if !all_solution_pages_submitted(storage, round, who, n_pages).await? {
        return Ok(false);
    }

    Ok(true)
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
