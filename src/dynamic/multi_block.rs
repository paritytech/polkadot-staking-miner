//! Utils to interact with multi-block election system.

use crate::{
    client::Client,
    commands::multi_block::types::{
        Snapshot, TargetSnapshotPage, TargetSnapshotPageOf, VoterSnapshotPage, VoterSnapshotPageOf,
    },
    commands::types::Listen,
    dynamic::{
        pallet_api,
        utils::{decode_error, storage_addr, to_scale_value, tx},
    },
    error::Error,
    prelude::{AccountId, ChainClient, Config, ExtrinsicParamsBuilder, Hash, LOG_TARGET, Storage},
    runtime::multi_block as runtime,
    signer::Signer,
    utils,
};
use codec::Decode;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use polkadot_sdk::{
    frame_support::BoundedVec,
    pallet_election_provider_multi_block::{
        types::PagedRawSolution,
        unsigned::miner::{BaseMiner as Miner, MineInput, MinerConfig},
    },
    sp_npos_elections::ElectionScore,
};
use std::collections::HashSet;
use subxt::{
    dynamic::Value,
    tx::{DynamicPayload, TxProgress},
};

/// A multi-block transaction.
pub enum TransactionKind {
    RegisterScore,
    SubmitPage(u32),
}

impl std::fmt::Display for TransactionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RegisterScore => f.write_str("register score"),
            Self::SubmitPage(p) => f.write_fmt(format_args!("submit page {p}")),
        }
    }
}
pub struct MultiBlockTransaction {
    kind: TransactionKind,
    tx: DynamicPayload,
}

impl MultiBlockTransaction {
    /// Create a transaction to register a score.
    pub fn register_score(score: ElectionScore) -> Result<Self, Error> {
        let scale_score = to_scale_value(score).map_err(decode_error::<ElectionScore>)?;

        Ok(Self {
            kind: TransactionKind::RegisterScore,
            tx: tx(
                pallet_api::multi_block_signed::tx::REGISTER,
                vec![scale_score],
            ),
        })
    }

    /// Create a new transaction to submit a page.
    pub fn submit_page<T: MinerConfig + 'static>(
        page: u32,
        maybe_solution: Option<T::Solution>,
    ) -> Result<Self, Error> {
        let scale_page = to_scale_value(page).map_err(decode_error::<T::Pages>)?;
        let scale_solution = to_scale_value(maybe_solution).map_err(decode_error::<T::Solution>)?;

        Ok(Self {
            kind: TransactionKind::SubmitPage(page),
            tx: tx(
                pallet_api::multi_block_signed::tx::SUBMIT_PAGE,
                vec![scale_page, scale_solution],
            ),
        })
    }

    pub fn into_parts(self) -> (TransactionKind, DynamicPayload) {
        (self.kind, self.tx)
    }
}

/// Fetches the target snapshot.
///
/// Note: the target snapshot is single paged.
pub(crate) async fn target_snapshot<T: MinerConfig>(
    page: u32,
    round: u32,
    storage: &Storage,
) -> Result<TargetSnapshotPage<T>, Error> {
    let page_idx = vec![Value::from(round), Value::from(page)];
    let addr = storage_addr(
        pallet_api::multi_block::storage::PAGED_TARGET_SNAPSHOT,
        page_idx,
    );

    match storage.fetch(&addr).await {
        Ok(Some(val)) => {
            let snapshot: TargetSnapshotPage<T> = Decode::decode(&mut val.encoded())?;
            log::trace!(
                target: LOG_TARGET,
                "Target snapshot with len {:?}, hash: {:?}",
                snapshot.len(),
                target_snapshot_hash(page, round, storage).await,
            );
            Ok(snapshot)
        }
        Ok(None) => Err(Error::EmptySnapshot),
        Err(err) => Err(err.into()),
    }
}

/// Fetches `page` of the voter snapshot.
pub(crate) async fn paged_voter_snapshot<T>(
    page: u32,
    round: u32,
    storage: &Storage,
) -> Result<VoterSnapshotPage<T>, Error>
where
    T: MinerConfig,
{
    match storage
        .fetch(&storage_addr(
            pallet_api::multi_block::storage::PAGED_VOTER_SNAPSHOT,
            vec![Value::from(round), Value::from(page)],
        ))
        .await
    {
        Ok(Some(val)) => match Decode::decode(&mut val.encoded()) {
            Ok(s) => {
                let snapshot: VoterSnapshotPage<T> = s;
                log::trace!(
                    target: LOG_TARGET,
                    "Voter snapshot page={page} len={}, hash={:?}",
                    snapshot.len(),
                    paged_voter_snapshot_hash(page, round, storage).await,
                );
                Ok(snapshot)
            }
            Err(err) => Err(err.into()),
        },
        Ok(None) => Err(Error::EmptySnapshot),
        Err(err) => Err(err.into()),
    }
}

/// Submits a transaction and returns the progress.
pub(crate) async fn submit_inner(
    client: &Client,
    signer: Signer,
    tx: MultiBlockTransaction,
    nonce: u64,
) -> Result<TxProgress<Config, ChainClient>, Error> {
    let (kind, tx) = tx.into_parts();

    log::trace!(target: LOG_TARGET, "submit `{kind}` nonce={nonce}");

    // NOTE: subxt sets mortal extrinsic by default.
    let xt_cfg = ExtrinsicParamsBuilder::default().nonce(nonce).build();
    let xt = client
        .chain_api()
        .tx()
        .create_signed(&tx, &*signer, xt_cfg)
        .await?;

    xt.submit_and_watch()
        .await
        .map_err(|e| {
            log::error!(target: LOG_TARGET, "submit tx {kind} failed: {:?}", e);
            e
        })
        .map_err(Into::into)
}

pub(crate) async fn mine_solution<T>(
    target_snapshot: TargetSnapshotPageOf<T>,
    voter_snapshot_paged: Vec<VoterSnapshotPageOf<T>>,
    n_pages: u32,
    round: u32,
    desired_targets: u32,
    block_number: u32,
    do_reduce: bool,
) -> Result<PagedRawSolution<T>, Error>
where
    T: MinerConfig<AccountId = AccountId> + Send + Sync + 'static,
    T::Solution: Send,
    T::Pages: Send,
    T::TargetSnapshotPerBlock: Send,
    T::VoterSnapshotPerBlock: Send,
    T::MaxVotesPerVoter: Send,
{
    log::trace!(
        target: LOG_TARGET,
        "Mine_and_submit: election target snap size: {:?}, voter snap size: {:?}",
        target_snapshot.len(),
        voter_snapshot_paged.len()
    );

    let voter_pages: BoundedVec<VoterSnapshotPageOf<T>, T::Pages> =
        BoundedVec::truncate_from(voter_snapshot_paged);

    log::trace!(
        target: LOG_TARGET,
        "MineInput: desired_targets={desired_targets},pages={n_pages},target_snapshot_len={},voters_pages_len={},do_reduce={do_reduce},round={round},at={block_number}",
        target_snapshot.len(), voter_pages.len()
    );

    let input = MineInput {
        desired_targets,
        all_targets: target_snapshot.clone(),
        voter_pages: voter_pages.clone(),
        pages: n_pages,
        do_reduce,
        round,
    };

    // Mine solution
    tokio::task::spawn_blocking(move || {
        let paged_raw_solution =
            Miner::<T>::mine_solution(input).map_err(|e| Error::Other(format!("{:?}", e)))?;
        Miner::<T>::check_feasibility(
            &paged_raw_solution,
            &voter_pages,
            &target_snapshot,
            desired_targets,
        )
        .map_err(|e| Error::Feasibility(format!("{:?}", e)))?;
        Ok(paged_raw_solution)
    })
    .await
    .map_err(|e| Error::Other(format!("{:?}", e)))?
}

/// Fetches the target snapshot and all voter snapshots which are missing
/// but some snapshots may not exist yet which is just ignored.
pub(crate) async fn fetch_missing_snapshots_lossy<T: MinerConfig>(
    snapshot: &mut Snapshot<T>,
    storage: &Storage,
    round: u32,
) -> Result<(), Error> {
    let n_pages = snapshot.n_pages;

    for page in 0..n_pages {
        match check_and_update_voter_snapshot(page, round, storage, snapshot).await {
            Ok(_) => {}
            Err(Error::EmptySnapshot) => {}
            Err(e) => return Err(e),
        };
    }

    let _ = check_and_update_target_snapshot(n_pages - 1, round, storage, snapshot).await;

    Ok(())
}

/// Similar to `fetch_missing_snapshots_lossy` but it returns an error if any snapshot is missing.
pub(crate) async fn fetch_missing_snapshots<T: MinerConfig>(
    snapshot: &mut Snapshot<T>,
    storage: &Storage,
    round: u32,
) -> Result<(), Error> {
    let n_pages = snapshot.n_pages;

    for page in 0..n_pages {
        check_and_update_voter_snapshot(page, round, storage, snapshot).await?;
    }

    check_and_update_target_snapshot(n_pages - 1, round, storage, snapshot).await
}

pub(crate) async fn paged_voter_snapshot_hash(
    page: u32,
    round: u32,
    storage: &Storage,
) -> Result<Hash, Error> {
    let bytes = storage
        .fetch(&storage_addr(
            pallet_api::multi_block::storage::PAGED_VOTER_SNAPSHOT_HASH,
            vec![Value::from(round), Value::from(page)],
        ))
        .await?
        .ok_or(Error::EmptySnapshot)?;

    Decode::decode(&mut bytes.encoded()).map_err(Into::into)
}

pub(crate) async fn target_snapshot_hash(
    page: u32,
    round: u32,
    storage: &Storage,
) -> Result<Hash, Error> {
    let bytes = storage
        .fetch(&storage_addr(
            pallet_api::multi_block::storage::PAGED_TARGET_SNAPSHOT_HASH,
            vec![Value::from(round), Value::from(page)],
        ))
        .await?
        .ok_or(Error::EmptySnapshot)?;

    Decode::decode(&mut bytes.encoded()).map_err(Into::into)
}

pub(crate) async fn check_and_update_voter_snapshot<T: MinerConfig>(
    page: u32,
    round: u32,
    storage: &Storage,
    snapshot: &mut Snapshot<T>,
) -> Result<(), Error> {
    let snapshot_hash = paged_voter_snapshot_hash(page, round, storage).await?;
    if snapshot.needs_voter_page(page, snapshot_hash) {
        let voter_snapshot = paged_voter_snapshot::<T>(page, round, storage).await?;
        snapshot.set_voter_page(page, voter_snapshot, snapshot_hash);
    }
    Ok(())
}

pub(crate) async fn check_and_update_target_snapshot<T: MinerConfig>(
    page: u32,
    round: u32,
    storage: &Storage,
    snapshot: &mut Snapshot<T>,
) -> Result<(), Error> {
    let snapshot_hash = target_snapshot_hash(page, round, storage).await?;
    if snapshot.needs_target_snapshot(snapshot_hash) {
        let target_snapshot = target_snapshot::<T>(page, round, storage).await?;
        snapshot.set_target_snapshot(target_snapshot, snapshot_hash);
    }
    Ok(())
}

/// Submit a multi-block solution.
///
/// It registers the score and submits all solution pages.
pub(crate) async fn submit<T: MinerConfig + Send + Sync + 'static>(
    client: &Client,
    signer: &Signer,
    mut paged_raw_solution: PagedRawSolution<T>,
    listen: Listen,
    chunk_size: usize,
    round: u32,
) -> Result<(), Error> {
    // Record that a submission has started
    crate::prometheus::on_submission_started();

    // 1. Check phase before registering score
    if !validate_signed_phase_or_bail(client, signer, listen, round).await? {
        return Ok(());
    }

    let mut i = 0;
    let tx_status = loop {
        let nonce = client
            .rpc()
            .system_account_next_index(signer.account_id())
            .await?;

        // Register score.
        match submit_inner(
            client,
            signer.clone(),
            MultiBlockTransaction::register_score(paged_raw_solution.score)?,
            nonce,
        )
        .await
        {
            Ok(tx) => break tx,
            Err(Error::Subxt(subxt::Error::Transaction(e))) => {
                i += 1;
                if i >= 10 {
                    return Err(Error::Subxt(subxt::Error::Transaction(e)));
                }
                log::debug!(target: LOG_TARGET, "Failed to register score: {:?}; retrying", e);
                tokio::time::sleep(std::time::Duration::from_secs(6)).await;
            }
            Err(e) => return Err(e),
        }
    };

    // 1. Wait for the `register_score tx` to be included in a block.
    //
    // NOTE: It's slow to iterate over the events to check if the score was registered
    // but it's performed for registering the score only once.
    let tx = utils::wait_tx_in_block_for_strategy(tx_status, listen).await?;
    let events = tx.wait_for_success().await?;
    if !events.has::<runtime::multi_block_election_signed::events::Registered>()? {
        return Err(Error::MissingTxEvent("Register score".to_string()));
    };

    log::info!(target: LOG_TARGET, "Score registered at block {:?}", tx.block_hash());

    // 2. Check phase before submitting pages
    if !validate_signed_phase_or_bail(client, signer, listen, round).await? {
        return Ok(());
    }

    let solutions: Vec<(u32, T::Solution)> = paged_raw_solution
        .solution_pages
        .iter()
        .enumerate()
        .map(|(page, solution)| (page as u32, solution.clone()))
        .collect::<Vec<_>>();

    // 3. Submit all solution pages using the appropriate strategy based on chunk_size
    let failed_pages = if chunk_size == 0 {
        // Use fully concurrent submission
        inner_submit_pages_concurrent::<T>(client, signer, solutions, listen, round).await?
    } else {
        // Use chunked concurrent submission
        inner_submit_pages_chunked::<T>(client, signer, solutions, listen, chunk_size, round)
            .await?
    };

    // 4. All pages were submitted successfully, we are done.
    if failed_pages.is_empty() {
        // Record successful submission
        crate::prometheus::on_submission_success();
        return Ok(());
    }

    log::info!(
        target: LOG_TARGET,
        "Failed to submit pages: {:?}; retrying",
        failed_pages.len()
    );

    // 5. Check phase before retrying failed pages
    if !validate_signed_phase_or_bail(client, signer, listen, round).await? {
        return Ok(());
    }

    // 6. Retry failed pages, one time.
    let mut solutions = Vec::new();
    for page in failed_pages {
        let solution = std::mem::take(&mut paged_raw_solution.solution_pages[page as usize]);
        solutions.push((page, solution));
    }

    // Retry with the same strategy as the initial submission
    let failed_pages = if chunk_size == 0 {
        inner_submit_pages_concurrent::<T>(client, signer, solutions, listen, round).await?
    } else {
        inner_submit_pages_chunked::<T>(client, signer, solutions, listen, chunk_size, round)
            .await?
    };

    if failed_pages.is_empty() {
        // Record successful submission
        crate::prometheus::on_submission_success();
        Ok(())
    } else {
        Err(Error::FailedToSubmitPages(failed_pages.len()))
    }
}

/// Result of a page submission batch
#[derive(Debug)]
pub struct SubmissionResult {
    /// Pages that failed to be included in blocks
    pub failed_pages: Vec<u32>,
    /// Pages that were successfully included in blocks
    pub submitted_pages: HashSet<u32>,
}

impl SubmissionResult {
    /// Check if all pages were submitted successfully
    pub fn all_successful(&self) -> bool {
        self.failed_pages.is_empty()
    }
}

/// Helper function to submit a batch of pages and wait for their inclusion in blocks
async fn submit_pages_batch<T: MinerConfig + 'static>(
    client: &Client,
    signer: &Signer,
    pages_to_submit: Vec<(u32, T::Solution)>,
    listen: Listen,
    round: u32,
) -> Result<SubmissionResult, Error> {
    // Check phase before submitting this batch
    if !validate_signed_phase_or_bail(client, signer, listen, round).await? {
        // Return all pages as failed if phase changed
        let failed_pages: Vec<u32> = pages_to_submit.iter().map(|(page, _)| *page).collect();
        return Ok(SubmissionResult {
            failed_pages,
            submitted_pages: HashSet::new(),
        });
    }
    let mut txs = FuturesUnordered::new();
    let mut nonce = client
        .rpc()
        .system_account_next_index(signer.account_id())
        .await?;

    // Collect expected pages before consuming the vector
    let expected_pages: HashSet<u32> = pages_to_submit.iter().map(|(page, _)| *page).collect();

    // 1. Submit all pages in the batch
    for (page, solution) in pages_to_submit.into_iter() {
        let tx_status = submit_inner(
            client,
            signer.clone(),
            MultiBlockTransaction::submit_page::<T>(page, Some(solution))?,
            nonce,
        )
        .await?;

        txs.push(async move {
            match utils::wait_tx_in_block_for_strategy(tx_status, listen).await {
                Ok(tx) => Ok(tx),
                Err(_) => Err(page),
            }
        });

        nonce += 1;
    }

    // 2. Wait for all pages in the batch to be included in a block
    let mut failed_pages_set = HashSet::new();
    let mut submitted_pages = HashSet::new();

    // 3. Process all transactions
    while let Some(page) = txs.next().await {
        match page {
            Ok(tx) => {
                let hash = tx.block_hash();
                // NOTE: It's slow to iterate over the events and that's the main reason why
                // submitting all pages "at once" with several pages submitted in the same block
                // is faster than a sequential or chuncked submission.
                let events = tx.wait_for_success().await?;
                for event in events.iter() {
                    let event = event?;

                    if let Some(solution_stored) =
                        event.as_event::<runtime::multi_block_election_signed::events::Stored>()?
                    {
                        let page = solution_stored.2;

                        log::info!(
                            target: LOG_TARGET,
                            "Page {page} included in block {:?}",
                            hash
                        );

                        submitted_pages.insert(solution_stored.2);
                    }
                }
            }
            // Transaction failed to be included in a block.
            // This happens when the transaction itself was rejected or failed
            Err(p) => {
                failed_pages_set.insert(p);
            }
        }
    }

    // 4. Check if all expected pages were included.
    // This handles cases where the transaction was submitted but we didn't get confirmation.
    let missing_pages: HashSet<u32> = expected_pages
        .difference(&submitted_pages)
        .cloned()
        .collect();

    // 5. Add missing pages to failed pages set.
    // This combines both types of failures (transactions not included in a block and transactions not confirmed)
    // into a single set of failed pages.
    failed_pages_set.extend(missing_pages);

    if !failed_pages_set.is_empty() {
        log::warn!(
            target: LOG_TARGET,
            "Some pages were not included in blocks: {:?}",
            failed_pages_set
        );
    }

    let failed_pages: Vec<u32> = failed_pages_set.into_iter().collect();

    Ok(SubmissionResult {
        failed_pages,
        submitted_pages,
    })
}

/// Submit all solution pages concurrently.
pub(crate) async fn inner_submit_pages_concurrent<T: MinerConfig + 'static>(
    client: &Client,
    signer: &Signer,
    paged_raw_solution: Vec<(u32, T::Solution)>,
    listen: Listen,
    round: u32,
) -> Result<Vec<u32>, Error> {
    // Submit all pages in a single batch
    let result = submit_pages_batch::<T>(client, signer, paged_raw_solution, listen, round).await?;

    // If all pages were submitted successfully, we're done
    if result.all_successful() {
        return Ok(vec![]);
    }

    Ok(result.failed_pages)
}

/// Submit solution pages in chunks, waiting for each chunk to be included in a block
/// before submitting the next chunk.
pub(crate) async fn inner_submit_pages_chunked<T: MinerConfig + 'static>(
    client: &Client,
    signer: &Signer,
    paged_raw_solution: Vec<(u32, T::Solution)>,
    listen: Listen,
    chunk_size: usize,
    round: u32,
) -> Result<Vec<u32>, Error> {
    assert!(chunk_size > 0, "Chunk size must be greater than 0");

    let mut failed_pages = Vec::new();
    let mut submitted_pages = HashSet::new();
    let total_pages = paged_raw_solution.len();

    // Process pages in chunks
    for chunk in paged_raw_solution.chunks(chunk_size) {
        // Check phase before each chunk
        if !validate_signed_phase_or_bail(client, signer, listen, round).await? {
            // Add remaining pages to failed list
            let remaining_pages: Vec<u32> = chunk.iter().map(|(page, _)| *page).collect();
            failed_pages.extend(remaining_pages);
            break;
        }

        // Convert the chunk slice to a Vec to pass to submit_pages_batch
        let chunk_vec = chunk.to_vec();

        // Get the actual page numbers in this chunk for logging
        let chunk_page_numbers: Vec<u32> = chunk_vec.iter().map(|(page, _)| *page).collect();

        log::info!(
            target: LOG_TARGET,
            "Submitting pages {:?} (out of {})",
            chunk_page_numbers,
            paged_raw_solution.len()
        );

        // Submit the current chunk
        let result = submit_pages_batch::<T>(client, signer, chunk_vec, listen, round).await?;

        // Check if we have failed pages before extending the overall lists
        if !result.all_successful() {
            log::warn!(
                target: LOG_TARGET,
                "Pages {:?} failed to be included in blocks",
                result.failed_pages
            );

            // Add the failed pages from this chunk to the overall list
            failed_pages.extend(result.failed_pages);

            return Ok(failed_pages);
        }

        // Add submitted pages to the overall set
        submitted_pages.extend(result.submitted_pages);

        log::info!(
            target: LOG_TARGET,
            "All pages {:?} were successfully included in blocks",
            chunk_page_numbers
        );

        // If all pages have been submitted, we're done
        if submitted_pages.len() == total_pages {
            return Ok(vec![]);
        }
    }

    Ok(failed_pages)
}

/// Submit a bail transaction to revert incomplete submissions
pub(crate) async fn bail(client: &Client, signer: &Signer, listen: Listen) -> Result<(), Error> {
    let bail_tx = runtime::tx().multi_block_election_signed().bail();
    let nonce = client
        .rpc()
        .system_account_next_index(signer.account_id())
        .await?;
    let xt_cfg = ExtrinsicParamsBuilder::default().nonce(nonce).build();
    let xt = client
        .chain_api()
        .tx()
        .create_signed(&bail_tx, &**signer, xt_cfg)
        .await?;
    let tx = xt.submit_and_watch().await?;
    utils::wait_tx_in_block_for_strategy(tx, listen).await?;
    Ok(())
}

/// Helper function to validate that we're still in Signed phase
/// If not in Signed phase and we have an incomplete submission, bail it
/// Returns true if we should continue, false if we should abort
async fn validate_signed_phase_or_bail(
    client: &Client,
    signer: &Signer,
    listen: Listen,
    round: u32,
) -> Result<bool, Error> {
    let storage = utils::storage_at_head(client, listen).await?;
    let current_phase = storage
        .fetch_or_default(&runtime::storage().multi_block_election().current_phase())
        .await?;

    // Import Phase enum from runtime types
    use crate::runtime::multi_block::runtime_types::pallet_election_provider_multi_block::types::Phase;

    if !matches!(current_phase, Phase::Signed(_)) {
        log::warn!(
            target: LOG_TARGET,
            "Phase changed from Signed to {:?} during submission for round {} - checking for incomplete submission",
            current_phase,
            round
        );

        // Check if we have a partial submission and bail it
        let storage = utils::storage_at_head(client, listen).await?;
        let maybe_submission = storage
            .fetch(
                &runtime::storage()
                    .multi_block_election_signed()
                    .submission_metadata_storage(round, signer.account_id()),
            )
            .await?;

        if let Some(submission) = maybe_submission {
            // We have a submission - check if it's incomplete
            let n_pages = crate::static_types::multi_block::Pages::get();
            let submitted_pages: usize = submission
                .pages
                .0
                .iter()
                .map(|&b| if b { 1 } else { 0 })
                .sum();

            if submitted_pages < n_pages as usize {
                log::info!(
                    target: LOG_TARGET,
                    "Bailing incomplete submission for round {} ({}/{} pages submitted)",
                    round,
                    submitted_pages,
                    n_pages
                );

                bail(client, signer, listen).await?;

                log::info!(target: LOG_TARGET, "Successfully bailed incomplete submission for round {}", round);
            }
        }

        return Ok(false);
    }

    Ok(true)
}
