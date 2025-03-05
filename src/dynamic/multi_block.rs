//! Utils to interact with multi-block election system.

use crate::{
    client::Client,
    commands::{Listen, SharedSnapshot},
    dynamic::{
        pallet_api,
        utils::{decode_error, storage_addr, to_scale_value, tx},
    },
    error::Error,
    prelude::{
        runtime, AccountId, ChainClient, Config, ExtrinsicParamsBuilder, Hash, Storage,
        TargetSnapshotPage, TargetSnapshotPageOf, VoterSnapshotPage, VoterSnapshotPageOf,
        LOG_TARGET,
    },
    signer::Signer,
    utils,
};
use codec::Decode;
use futures::{stream::FuturesUnordered, StreamExt};
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
        "MineInput: desired_targets={desired_targets},pages={n_pages},target_snapshot_len={},voters_pages_len={},do_reduce=false,round={round},at={block_number}",
        target_snapshot.len(), voter_pages.len()
    );

    let input = MineInput {
        desired_targets,
        all_targets: target_snapshot.clone(),
        voter_pages: voter_pages.clone(),
        pages: n_pages,
        // TODO: get from runtime/configs.
        do_reduce: false,
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
            "multi-block-solution",
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
) -> Result<(), Error> {
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

    // 2. Submit all solution pages.
    let failed_pages = inner_submit_pages::<T>(client, signer, solutions, listen).await?;

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

    let failed_pages = inner_submit_pages::<T>(client, signer, solutions, listen).await?;

    if failed_pages.is_empty() {
        Ok(())
    } else {
        Err(Error::FailedToSubmitPages(failed_pages.len()))
    }
}

pub(crate) async fn inner_submit_pages<T: MinerConfig + Send + Sync + 'static>(
    client: &Client,
    signer: &Signer,
    paged_raw_solution: Vec<(u32, T::Solution)>,
    listen: Listen,
) -> Result<Vec<u32>, Error> {
    let mut txs = FuturesUnordered::new();

    let mut nonce = client
        .rpc()
        .system_account_next_index(signer.account_id())
        .await?;

    let len = paged_raw_solution.len();

    // 1. Submit all solution pages.
    for (page, solution) in paged_raw_solution.into_iter() {
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

    // 2. Wait for all pages to be included in a block.
    let mut failed_pages = Vec::new();
    let mut submitted_pages = HashSet::new();

    while let Some(page) = txs.next().await {
        match page {
            Ok(tx) => {
                let hash = tx.block_hash();

                // NOTE: It's slow to iterate over the events and that's we are
                // submitting all pages "at once" and several pages are submitted in the same block.
                let events = tx.wait_for_success().await?;
                for event in events.iter() {
                    let event = event?;

                    if let Some(solution_stored) =
                        event.as_event::<runtime::multi_block_signed::events::Stored>()?
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
            Err(p) => {
                failed_pages.push(p);
            }
        }

        if submitted_pages.len() == len {
            return Ok(vec![]);
        }
    }

    Ok(failed_pages)
}
