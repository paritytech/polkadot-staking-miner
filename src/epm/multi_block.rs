use crate::{
	client::Client,
	commands::Listen,
	epm::{
		pallet_api,
		utils::{storage_addr, MultiBlockTransaction},
	},
	error::Error,
	helpers,
	prelude::{
		AccountId, ChainClient, Hash, Header, Storage, TargetSnapshotPage, TargetSnapshotPageOf,
		VoterSnapshotPage, VoterSnapshotPageOf, LOG_TARGET,
	},
	signer::Signer,
	static_types,
};
use codec::Decode;
use polkadot_sdk::{
	frame_support::BoundedVec,
	pallet_election_provider_multi_block::unsigned::miner::{
		BaseMiner as Miner, MineInput, MinerConfig,
	},
};
use subxt::{
	config::{DefaultExtrinsicParamsBuilder, Header as _},
	dynamic::Value,
};

pub(crate) fn update_metadata_constants(api: &ChainClient) -> Result<(), Error> {
	// multi block constants.
	let pages: u32 = pallet_api::multi_block::constants::PAGES.fetch(api)?;
	static_types::Pages::set(pages);
	static_types::TargetSnapshotPerBlock::set(
		pallet_api::multi_block::constants::TARGET_SNAPSHOT_PER_BLOCK.fetch(api)?,
	);
	static_types::VoterSnapshotPerBlock::set(
		pallet_api::multi_block::constants::VOTER_SNAPSHOT_PER_BLOCK.fetch(api)?,
	);

	// election provider constants.
	static_types::MaxBackersPerWinner::set(
		pallet_api::election_provider_multi_phase::constants::MAX_VOTES_PER_VOTER.fetch(api)?,
	);
	static_types::MaxLength::set(
		pallet_api::election_provider_multi_phase::constants::MAX_LENGTH.fetch(api)?,
	);
	static_types::MaxWinnersPerPage::set(
		pallet_api::election_provider_multi_phase::constants::MAX_WINNERS.fetch(api)?,
	);

	Ok(())
}

/// Fetches the target snapshot.
///
/// Note: the target snapshot is single paged.
pub(crate) async fn target_snapshot<T: MinerConfig>(
	page: u32,
	storage: &Storage,
) -> Result<TargetSnapshotPage<T>, Error> {
	let page_idx = vec![Value::from(page)];
	let addr = storage_addr(pallet_api::multi_block::storage::PAGED_TARGET_SNAPSHOT, page_idx);

	match storage.fetch(&addr).await {
		Ok(Some(val)) => {
			let snapshot: TargetSnapshotPage<T> = Decode::decode(&mut val.encoded())?;
			log::trace!(
				target: LOG_TARGET,
				"Target snapshot with len {:?}",
				snapshot.len()
			);
			Ok(snapshot)
		},
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
	let snapshot_hash: Hash = {
		let bytes = storage
			.fetch(&storage_addr(
				pallet_api::multi_block::storage::PAGED_VOTER_SNAPSHOT_HASH,
				vec![Value::from(page)],
			))
			.await?
			.ok_or(Error::EmptySnapshot)?;

		Decode::decode(&mut bytes.encoded())?
	};

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
					snapshot_hash,
				);
				Ok(snapshot)
			},
			Err(err) => Err(err.into()),
		},
		Ok(None) => Err(Error::EmptySnapshot),
		Err(err) => Err(err.into()),
	}
}

/// Fetches the full voter and target snapshots.
pub(crate) async fn fetch_full_snapshots<T: MinerConfig>(
	n_pages: u32,
	storage: &Storage,
) -> Result<(TargetSnapshotPage<T>, Vec<VoterSnapshotPage<T>>), Error> {
	log::trace!(target: LOG_TARGET, "fetch_full_snapshots");

	let mut voters = Vec::with_capacity(n_pages as usize);
	let targets = target_snapshot::<T>(n_pages - 1, storage).await?;

	for page in (0..n_pages).rev() {
		let paged_voters = paged_voter_snapshot::<T>(page, storage).await?;
		voters.push(paged_voters);
	}

	log::trace!("Fetched full snapshots: targets: {:?}, voters: {:?}", targets.len(), voters.len());

	Ok((targets, voters))
}

/// Mines, registers and submits a solution.
pub(crate) async fn mine_and_submit<T>(
	at: &Header,
	client: &Client,
	signer: Signer,
	listen: Listen,
	target_snapshot: &TargetSnapshotPageOf<T>,
	voter_snapshot_paged: &Vec<VoterSnapshotPageOf<T>>,
	n_pages: u32,
	round: u32,
	desired_targets: u32,
) -> Result<(), Error>
where
	T: MinerConfig<AccountId = AccountId> + Send + Sync + 'static,
	T::Solution: Send,
{
	log::trace!(
		target: LOG_TARGET,
		"Mine, submit: election target snap size: {:?}, voter snap size: {:?}",
		target_snapshot.len(),
		voter_snapshot_paged.len()
	);

	let targets: TargetSnapshotPageOf<T> = target_snapshot.clone();
	let voters: BoundedVec<VoterSnapshotPageOf<T>, T::Pages> =
		BoundedVec::truncate_from(voter_snapshot_paged.clone());

	let input = MineInput {
		desired_targets,
		all_targets: targets.clone(),
		voter_pages: voters.clone(),
		pages: n_pages,
		// TODO: get from runtime/configs.
		do_reduce: false,
		round,
	};

	let paged_raw_solution =
		Miner::<T>::mine_solution(input).map_err(|e| Error::Other(format!("{:?}", e)))?;

	// register solution.
	submit_and_watch::<T>(
		at,
		client,
		signer.clone(),
		listen,
		MultiBlockTransaction::register_score(paged_raw_solution.score)?,
	)
	.await?;

	// submit all solution pages.
	for (page, solution) in paged_raw_solution.solution_pages.into_iter().enumerate() {
		submit_and_watch::<T>(
			at,
			client,
			signer.clone(),
			listen,
			MultiBlockTransaction::submit_page::<T>(page as u32, Some(solution))?,
		)
		.await?;
	}

	Ok(())
}

/// Performs the whole set of steps to submit a new solution:
/// 1. Fetches target and voter snapshots;
/// 2. Mines a solution;
/// 3. Registers new solution;
/// 4. Submits the paged solution.
pub(crate) async fn fetch_mine_and_submit<T>(
	at: &Header,
	client: &Client,
	signer: Signer,
	listen: Listen,
	storage: &Storage,
	n_pages: u32,
	round: u32,
	desired_targets: u32,
) -> Result<(), Error>
where
	T: MinerConfig<AccountId = AccountId> + Send + Sync + 'static,
	T::Solution: Send,
{
	let (target_snapshot, voter_snapshot_paged) =
		fetch_full_snapshots::<T>(n_pages, storage).await?;
	log::trace!(
		target: LOG_TARGET,
		"Fetched, full election target snapshot with {} targets, voter snapshot with {:?} pages.",
		target_snapshot.len(),
		voter_snapshot_paged.len()
	);

	mine_and_submit::<T>(
		at,
		client,
		signer,
		listen,
		&target_snapshot,
		&voter_snapshot_paged,
		n_pages,
		round,
		desired_targets,
	)
	.await
}

/// Submits and watches a `DynamicPayload`, ie. an extrinsic.
async fn submit_and_watch<T: MinerConfig + Send + Sync + 'static>(
	at: &Header,
	client: &Client,
	signer: Signer,
	listen: Listen,
	tx: MultiBlockTransaction,
) -> Result<(), Error> {
	let (kind, tx) = tx.to_parts();
	let block_hash = at.hash();
	let nonce = client.rpc().system_account_next_index(signer.account_id()).await?;

	log::trace!(target: LOG_TARGET, "submit_and_watch for `{}` at {:?}", kind.as_str(), block_hash);

	// NOTE: subxt sets mortal extrinsic by default.
	let xt_cfg = DefaultExtrinsicParamsBuilder::default().nonce(nonce).build();
	let xt = client.chain_api().tx().create_signed(&tx, &*signer, xt_cfg).await?;

	let tx_progress = xt.submit_and_watch().await.map_err(|e| {
		log::error!(target: LOG_TARGET, "submit tx {} failed: {:?}", kind.as_str(), e);
		e
	})?;

	match listen {
		Listen::Head => {
			let in_block = helpers::wait_for_in_block(tx_progress).await?;
			let evs = in_block.fetch_events().await?;
			// TODO: if we were submitting a page, then we should check for verification event as well
			if !kind.in_events(&evs)? {
				return Err(Error::MissingTxEvent(kind.as_str().to_string()));
			}
		},
		Listen::Finalized => {
			let finalized_block = tx_progress.wait_for_finalized().await?;
			let _block_hash = finalized_block.block_hash();
			let evs = finalized_block.wait_for_success().await?;
			// TODO: if we were submitting a page, then we should check for verification event as well
			if !kind.in_events(&evs)? {
				return Err(Error::MissingTxEvent(kind.as_str().to_string()));
			}
		},
	}
	Ok(())
}
