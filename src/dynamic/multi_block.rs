//! Utils to interact with multi-block election system.

use crate::{
    client::Client,
    commands::multi_block::types::{
        SharedSnapshot, TargetSnapshotPage, TargetSnapshotPageOf, VoterSnapshotPage,
        VoterSnapshotPageOf,
    },
    commands::types::Listen,
    dynamic::{
        pallet_api,
        utils::{decode_error, storage_addr, to_scale_value, tx},
    },
    error::Error,
    prelude::{AccountId, ChainClient, Config, ExtrinsicParamsBuilder, Hash, Storage, LOG_TARGET},
    runtime::multi_block as runtime,
    signer::Signer,
    utils,
};
use codec::Decode;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
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
    storage: &Storage,
) -> Result<TargetSnapshotPage<T>, Error> {
    let page_idx = vec![Value::from(page)];
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
                target_snapshot_hash(page, storage).await,
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
    storage: &Storage,
) -> Result<VoterSnapshotPage<T>, Error>
where
    T: MinerConfig,
{
    match storage
        .fetch(&storage_addr(
            pallet_api::multi_block::storage::PAGED_VOTER_SNAPSHOT,
            vec![Value::from(page)],
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
                    paged_voter_snapshot_hash(page, storage).await,
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
    snapshot: &SharedSnapshot<T>,
    storage: &Storage,
) -> Result<(), Error> {
    let n_pages = snapshot.read().n_pages;

    for page in 0..n_pages {
        match check_and_update_voter_snapshot(page, storage, snapshot).await {
            Ok(_) => {}
            Err(Error::EmptySnapshot) => {}
            Err(e) => return Err(e),
        };
    }

    let _ = check_and_update_target_snapshot(n_pages - 1, storage, snapshot).await;

    Ok(())
}

/// Similar to `fetch_missing_snapshots_lossy` but it returns an error if any snapshot is missing.
pub(crate) async fn fetch_missing_snapshots<T: MinerConfig>(
    snapshot: &SharedSnapshot<T>,
    storage: &Storage,
) -> Result<(), Error> {
    let n_pages = snapshot.read().n_pages;

    for page in 0..n_pages {
        check_and_update_voter_snapshot(page, storage, snapshot).await?;
    }

    check_and_update_target_snapshot(n_pages - 1, storage, snapshot).await
}

pub(crate) async fn paged_voter_snapshot_hash(page: u32, storage: &Storage) -> Result<Hash, Error> {
    let bytes = storage
        .fetch(&storage_addr(
            pallet_api::multi_block::storage::PAGED_VOTER_SNAPSHOT_HASH,
            vec![Value::from(page)],
        ))
        .await?
        .ok_or(Error::EmptySnapshot)?;

    Decode::decode(&mut bytes.encoded()).map_err(Into::into)
}

pub(crate) async fn target_snapshot_hash(page: u32, storage: &Storage) -> Result<Hash, Error> {
    let bytes = storage
        .fetch(&storage_addr(
            pallet_api::multi_block::storage::PAGED_TARGET_SNAPSHOT_HASH,
            vec![Value::from(page)],
        ))
        .await?
        .ok_or(Error::EmptySnapshot)?;

    Decode::decode(&mut bytes.encoded()).map_err(Into::into)
}

pub(crate) async fn check_and_update_voter_snapshot<T: MinerConfig>(
    page: u32,
    storage: &Storage,
    snapshot: &SharedSnapshot<T>,
) -> Result<(), Error> {
    let snapshot_hash = paged_voter_snapshot_hash(page, storage).await?;
    if snapshot.read().needs_voter_page(page, snapshot_hash) {
        let voter_snapshot = paged_voter_snapshot::<T>(page, storage).await?;
        snapshot
            .write()
            .set_voter_page(page, voter_snapshot, snapshot_hash);
    }
    Ok(())
}

pub(crate) async fn check_and_update_target_snapshot<T: MinerConfig>(
    page: u32,
    storage: &Storage,
    snapshot: &SharedSnapshot<T>,
) -> Result<(), Error> {
    let snapshot_hash = target_snapshot_hash(page, storage).await?;
    if snapshot.read().needs_target_snapshot(snapshot_hash) {
        let target_snapshot = target_snapshot::<T>(page, storage).await?;
        snapshot
            .write()
            .set_target_snapshot(target_snapshot, snapshot_hash);
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
) -> Result<(), Error>
where
    T::Solution: Send + Sync + 'static,
    T::Pages: Send + Sync + 'static,
{
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
    if !events.has::<runtime::multi_block_signed::events::Registered>()? {
        return Err(Error::MissingTxEvent("Register score".to_string()));
    };

    log::info!(target: LOG_TARGET, "Score registered at block {:?}", tx.block_hash());

    let solutions: Vec<(u32, T::Solution)> = paged_raw_solution
        .solution_pages
        .iter()
        .enumerate()
        .map(|(page, solution)| (page as u32, solution.clone()))
        .collect::<Vec<_>>();

    // 2. Submit all solution pages using the appropriate strategy based on chunk_size
    let failed_pages = if chunk_size == 0 {
        // Use fully concurrent submission
        inner_submit_pages_concurrent::<T>(client, signer, solutions, listen).await?
    } else {
        // Use chunked concurrent submission
        inner_submit_pages_chunked::<T>(client, signer, solutions, listen, chunk_size).await?
    };

    // 3. All pages were submitted successfully, we are done.
    if failed_pages.is_empty() {
        return Ok(());
    }

    log::info!(
        target: LOG_TARGET,
        "Failed to submit pages: {:?}; retrying",
        failed_pages.len()
    );

    // 4. Retry failed pages, one time.
    let mut solutions = Vec::new();
    for page in failed_pages {
        let solution = std::mem::take(&mut paged_raw_solution.solution_pages[page as usize]);
        solutions.push((page, solution));
    }

    // Retry with the same strategy as the initial submission
    let failed_pages = if chunk_size == 0 {
        inner_submit_pages_concurrent::<T>(client, signer, solutions, listen).await?
    } else {
        inner_submit_pages_chunked::<T>(client, signer, solutions, listen, chunk_size).await?
    };

    if failed_pages.is_empty() {
        Ok(())
    } else {
        Err(Error::FailedToSubmitPages(failed_pages.len()))
    }
}

/// Helper function to submit a batch of pages and wait for their inclusion in blocks
async fn submit_pages_batch<T: MinerConfig + Send + Sync + 'static>(
    client: &Client,
    signer: &Signer,
    pages_to_submit: Vec<(u32, T::Solution)>,
    listen: Listen,
) -> Result<(Vec<u32>, HashSet<u32>), Error>
where
    T::Solution: Send + Sync + 'static,
    T::Pages: Send + Sync + 'static,
{
    let mut txs = FuturesUnordered::new();
    let mut nonce = client
        .rpc()
        .system_account_next_index(signer.account_id())
        .await?;

    // Create a map to track which page corresponds to which transaction
    let mut page_to_tx = Vec::new();

    // Submit all pages in the batch
    for (page, solution) in pages_to_submit.into_iter() {
        let tx_status = submit_inner(
            client,
            signer.clone(),
            MultiBlockTransaction::submit_page::<T>(page, Some(solution))?,
            nonce,
        )
        .await?;

        // Store the page number with the transaction
        page_to_tx.push(page);

        txs.push(async move {
            match utils::wait_tx_in_block_for_strategy(tx_status, listen).await {
                Ok(tx) => Ok(tx),
                Err(_) => Err(page),
            }
        });

        nonce += 1;
    }

    // Wait for all pages in the batch to be included in a block
    let mut failed_pages = Vec::new();
    let mut submitted_pages = HashSet::new();
    let mut processed_pages = HashSet::new();

    let mut tx_index = 0;
    while let Some(page) = txs.next().await {
        match page {
            Ok(tx) => {
                let hash = tx.block_hash();

                // Instead of waiting for events, use a polling mechanism with exponential backoff
                let mut retries = 0;
                let max_retries = 10;
                let mut backoff_ms = 100;

                let mut page_confirmed = false;
                while retries < max_retries && !page_confirmed {
                    // Check if the page was successfully submitted by querying the storage
                    let storage = utils::storage_at(Some(hash), client.chain_api()).await?;
                    let round = storage
                        .fetch_or_default(&runtime::storage().multi_block().round())
                        .await?;

                    // Get the submission metadata for the current account
                    let submission = storage
                        .fetch(
                            &runtime::storage()
                                .multi_block_signed()
                                .submission_metadata_storage(round, signer.account_id()),
                        )
                        .await?;

                    // If we have submission metadata and the page is marked as submitted, add it to our set
                    if let Some(submission) = submission {
                        // Get the page number for this transaction
                        let page_num = page_to_tx[tx_index];

                        // Check if this specific page is marked as submitted
                        if let Some(submitted) = submission.pages.0.get(page_num as usize) {
                            if *submitted && !processed_pages.contains(&page_num) {
                                log::info!(
                                    target: LOG_TARGET,
                                    "Page {page_num} confirmed in block {:?}",
                                    hash
                                );
                                submitted_pages.insert(page_num);
                                processed_pages.insert(page_num);
                                page_confirmed = true;
                            }
                        }
                    }

                    if !page_confirmed {
                        // Exponential backoff with a maximum of 1 second
                        tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                        backoff_ms = std::cmp::min(backoff_ms * 2, 1000);
                        retries += 1;
                    }
                }

                tx_index += 1;
            }
            Err(p) => {
                failed_pages.push(p);
                tx_index += 1;
            }
        }
    }

    Ok((failed_pages, submitted_pages))
}

/// Submit all solution pages concurrently.
pub(crate) async fn inner_submit_pages_concurrent<T: MinerConfig + Send + Sync + 'static>(
    client: &Client,
    signer: &Signer,
    paged_raw_solution: Vec<(u32, T::Solution)>,
    listen: Listen,
) -> Result<Vec<u32>, Error>
where
    T::Solution: Send + Sync + 'static,
    T::Pages: Send + Sync + 'static,
{
    let len = paged_raw_solution.len();

    // First, check which pages are already submitted to avoid resubmitting
    let storage = utils::storage_at_head(client, listen).await?;
    let round = storage
        .fetch_or_default(&runtime::storage().multi_block().round())
        .await?;

    // Get the submission metadata for the current account
    let submission = storage
        .fetch(
            &runtime::storage()
                .multi_block_signed()
                .submission_metadata_storage(round, signer.account_id()),
        )
        .await?;

    // Filter out pages that are already submitted
    let mut pages_to_submit = Vec::new();
    let mut already_submitted = HashSet::new();

    if let Some(submission) = submission {
        for (page, solution) in paged_raw_solution {
            if let Some(submitted) = submission.pages.0.get(page as usize) {
                if *submitted {
                    already_submitted.insert(page);
                } else {
                    pages_to_submit.push((page, solution));
                }
            } else {
                pages_to_submit.push((page, solution));
            }
        }
    } else {
        pages_to_submit = paged_raw_solution;
    }

    // If all pages are already submitted, we're done
    if already_submitted.len() == len {
        return Ok(vec![]);
    }

    // Submit only the pages that haven't been submitted yet
    let (failed_pages, submitted_pages) =
        submit_pages_batch::<T>(client, signer, pages_to_submit, listen).await?;

    // Combine already submitted pages with newly submitted pages
    let mut all_submitted = already_submitted;
    all_submitted.extend(submitted_pages);

    // If all pages were submitted successfully, we're done
    if all_submitted.len() == len {
        return Ok(vec![]);
    }

    Ok(failed_pages)
}

/// Submit solution pages in chunks, waiting for each chunk to be included in a block
/// before submitting the next chunk.
pub(crate) async fn inner_submit_pages_chunked<T: MinerConfig + Send + Sync + 'static>(
    client: &Client,
    signer: &Signer,
    paged_raw_solution: Vec<(u32, T::Solution)>,
    listen: Listen,
    chunk_size: usize,
) -> Result<Vec<u32>, Error>
where
    T::Solution: Send + Sync + 'static,
    T::Pages: Send + Sync + 'static,
{
    assert!(chunk_size > 0, "Chunk size must be greater than 0");

    let mut failed_pages = Vec::new();
    let mut submitted_pages = HashSet::new();
    let total_pages = paged_raw_solution.len();

    // First, check which pages are already submitted to avoid resubmitting
    let storage = utils::storage_at_head(client, listen).await?;
    let round = storage
        .fetch_or_default(&runtime::storage().multi_block().round())
        .await?;

    // Get the submission metadata for the current account
    let submission = storage
        .fetch(
            &runtime::storage()
                .multi_block_signed()
                .submission_metadata_storage(round, signer.account_id()),
        )
        .await?;

    // Filter out pages that are already submitted
    let mut pages_to_submit = Vec::new();

    if let Some(submission) = submission {
        for (page, solution) in paged_raw_solution {
            if let Some(submitted) = submission.pages.0.get(page as usize) {
                if *submitted {
                    submitted_pages.insert(page);
                } else {
                    pages_to_submit.push((page, solution));
                }
            } else {
                pages_to_submit.push((page, solution));
            }
        }
    } else {
        pages_to_submit = paged_raw_solution;
    }

    // If all pages are already submitted, we're done
    if submitted_pages.len() == total_pages {
        return Ok(vec![]);
    }

    // Process pages in chunks
    for chunk_start in (0..pages_to_submit.len()).step_by(chunk_size) {
        let chunk_end = std::cmp::min(chunk_start + chunk_size, pages_to_submit.len());

        let chunk: Vec<_> = pages_to_submit[chunk_start..chunk_end]
            .iter()
            .map(|(page, solution)| (*page, solution.clone()))
            .collect();

        log::info!(
            target: LOG_TARGET,
            "Submitting chunk of pages {}-{} of {}",
            chunk_start,
            chunk_end - 1,
            pages_to_submit.len()
        );

        // Submit the current chunk
        let (chunk_failed_pages, chunk_submitted_pages) =
            submit_pages_batch::<T>(client, signer, chunk, listen).await?;

        // Add failed pages to the overall list
        failed_pages.extend(chunk_failed_pages);

        // Add submitted pages to the overall set
        submitted_pages.extend(chunk_submitted_pages);

        // If all pages have been submitted, we're done
        if submitted_pages.len() == total_pages {
            return Ok(vec![]);
        }
    }

    Ok(failed_pages)
}
