use crate::{
	client::Client,
	commands::{Listen, Snapshot},
	epm::{
		pallet_api,
		utils::{dynamic_decode_error, storage_addr, to_scale_value, tx},
	},
	error::Error,
	helpers::{self, TimedFuture},
	prelude::{
		runtime, AccountId, ChainClient, Config, Hash, Header, Storage, TargetSnapshotPage,
		TargetSnapshotPageOf, VoterSnapshotPage, VoterSnapshotPageOf, LOG_TARGET,
	},
	signer::Signer,
	static_types,
};
use codec::Decode;
use polkadot_sdk::{
	frame_support::BoundedVec,
	pallet_election_provider_multi_block::{
		types::PagedRawSolution,
		unsigned::miner::{BaseMiner as Miner, MineInput, MinerConfig},
	},
	sp_npos_elections::ElectionScore,
};
use subxt::{
	blocks::ExtrinsicEvents,
	config::{DefaultExtrinsicParamsBuilder, Header as _},
	dynamic::Value,
	tx::DynamicPayload,
};

/// A multi-block transaction.
pub enum TransactionKind {
	RegisterScore,
	SubmitPage,
}

impl TransactionKind {
	pub fn as_str(&self) -> &'static str {
		match self {
			Self::RegisterScore => "register score",
			Self::SubmitPage => "submit page",
		}
	}

	/// Check if the transaction is in the given events.
	pub fn in_events(&self, evs: &ExtrinsicEvents<Config>) -> Result<bool, Error> {
		match self {
			Self::RegisterScore =>
				evs.has::<runtime::multi_block_signed::events::Registered>().map_err(Into::into),
			Self::SubmitPage =>
				evs.has::<runtime::multi_block_signed::events::Stored>().map_err(Into::into),
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
		let scale_score =
			to_scale_value(score).map_err(|err| dynamic_decode_error::<ElectionScore>(err))?;

		Ok(Self {
			kind: TransactionKind::RegisterScore,
			tx: tx(pallet_api::multi_block_signed::tx::REGISTER, vec![scale_score]),
		})
	}

	/// Create a new transaction to submit a page.
	pub fn submit_page<T: MinerConfig + 'static>(
		page: u32,
		maybe_solution: Option<T::Solution>,
	) -> Result<Self, Error> {
		let scale_page =
			to_scale_value(page).map_err(|err| dynamic_decode_error::<T::Pages>(err))?;
		let scale_solution = to_scale_value(maybe_solution)
			.map_err(|err| dynamic_decode_error::<T::Solution>(err))?;

		Ok(Self {
			kind: TransactionKind::SubmitPage,
			tx: tx(
				pallet_api::multi_block_signed::tx::SUBMIT_PAGE,
				vec![scale_page, scale_solution],
			),
		})
	}

	pub fn to_parts(self) -> (TransactionKind, DynamicPayload) {
		(self.kind, self.tx)
	}
}

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
				"Target snapshot with len {:?}, hash: {:?}",
				snapshot.len(),
				target_snapshot_hash(page, storage).await,
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
			},
			Err(err) => Err(err.into()),
		},
		Ok(None) => Err(Error::EmptySnapshot),
		Err(err) => Err(err.into()),
	}
}

/// Submits and watches a `DynamicPayload`, ie. an extrinsic.
pub(crate) async fn submit_and_watch<T: MinerConfig + Send + Sync + 'static>(
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
			// TODO: This it slow for the sign phase in the substrate-node with the staking-playground feature.
			//
			/*let evs = finalized_block.wait_for_success().await?;
			// TODO: if we were submitting a page, then we should check for verification event as well
			if !kind.in_events(&evs)? {
				return Err(Error::MissingTxEvent(kind.as_str().to_string()));
			}*/
		},
	}
	Ok(())
}

pub(crate) async fn mine_solution<T>(
	target_snapshot: TargetSnapshotPageOf<T>,
	voter_snapshot_paged: Vec<VoterSnapshotPageOf<T>>,
	n_pages: u32,
	round: u32,
	desired_targets: u32,
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
		"MineInput: desired_targets={desired_targets},pages={n_pages},target_snapshot_len={},voters_pages_len={},do_reduce=false,round={round}",
		target_snapshot.len(), voter_pages.len()
	);

	let input = MineInput {
		desired_targets,
		all_targets: target_snapshot,
		voter_pages,
		pages: n_pages,
		// TODO: get from runtime/configs.
		do_reduce: false,
		round,
	};

	// Mine solution
	match tokio::task::spawn_blocking(move || {
		Miner::<T>::mine_solution(input).map_err(|e| Error::Other(format!("{:?}", e)))
	})
	.timed()
	.await
	{
		(Ok(Ok(s)), dur) => {
			log::trace!(target: LOG_TARGET, "Mined solution in {}ms", dur.as_millis());
			Ok(s)
		},
		(Ok(Err(e)), _) => Err(e),
		(Err(e), _dur) => Err(Error::Other(format!("{:?}", e))),
	}
}

pub(crate) async fn fetch_missing_snapshots<T: MinerConfig>(
	snapshot: &mut Snapshot<T>,
	storage: &Storage,
) -> Result<(), Error> {
	for page in 0..snapshot.n_pages {
		check_and_update_voter_snapshot(page, storage, snapshot).await?;
	}

	check_and_update_target_snapshot(snapshot.n_pages - 1, storage, snapshot).await?;

	Ok(())
}

pub(crate) async fn paged_voter_snapshot_hash(page: u32, storage: &Storage) -> Result<Hash, Error> {
	let bytes = storage.fetch(&storage_addr(
				pallet_api::multi_block::storage::PAGED_VOTER_SNAPSHOT_HASH,
				vec![Value::from(page)],
		))
		.await?
		.ok_or(Error::EmptySnapshot)?;

	Decode::decode(&mut bytes.encoded()).map_err(Into::into)
}

pub(crate) async fn target_snapshot_hash(page: u32, storage: &Storage) -> Result<Hash, Error> {
	let bytes = storage
		.fetch(&storage_addr(pallet_api::multi_block::storage::PAGED_TARGET_SNAPSHOT_HASH, vec![Value::from(page)]))
		.await?
		.ok_or(Error::EmptySnapshot)?;

	Decode::decode(&mut bytes.encoded()).map_err(Into::into)
}

pub(crate) async fn check_and_update_voter_snapshot<T: MinerConfig>(
	page: u32,
	storage: &Storage,
	snapshot: &mut Snapshot<T>,
) -> Result<(), Error> {
	let snapshot_hash = paged_voter_snapshot_hash(page, storage).await?;
	if snapshot.needs_voter_page(page, snapshot_hash) {
		let voter_snapshot = paged_voter_snapshot::<T>(page, storage).await?;
		snapshot.set_voter_page(page, voter_snapshot, snapshot_hash);
	}
	Ok(())
}

pub(crate) async fn check_and_update_target_snapshot<T: MinerConfig>(
	page: u32,
	storage: &Storage,
	snapshot: &mut Snapshot<T>,
) -> Result<(), Error> {
	let snapshot_hash = target_snapshot_hash(page, storage).await?;
	if snapshot.needs_target_snapshot(snapshot_hash) {
		let target_snapshot = target_snapshot::<T>(page, storage).await?;
		snapshot.set_target_snapshot(target_snapshot, snapshot_hash);
	}
	Ok(())
}
