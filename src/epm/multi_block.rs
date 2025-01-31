use std::marker::PhantomData;

use crate::{
	client::Client, commands::Listen, error::Error, helpers, prelude::*, signer::Signer,
	static_types,
};
use polkadot_sdk::{
	frame_support::BoundedVec,
	pallet_election_provider_multi_block::unsigned::miner::{
		BaseMiner as Miner, MineInput, MinerConfig,
	},
	sp_npos_elections::ElectionScore,
};

use codec::{Decode, Encode};
use scale_info::PortableRegistry;
use scale_value::scale::decode_as_type;
use serde::de::DeserializeOwned;
use subxt::{
	config::{DefaultExtrinsicParamsBuilder, Header as _},
	dynamic::Value,
	tx::DynamicPayload,
};

const EPM: &str = "ElectionProviderMultiPhase";
const MULTI_BLOCK: &str = "MultiBlock";
const EPM_SIGNED: &str = "ElectionSignedPallet";

const PAGES: &str = "Pages";
const TARGET_SNAPSHOT_PER_BLOCK: &str = "TargetSnapshotPerBlock";
const MAX_VOTES_PER_VOTER: &str = "MinerMaxVotesPerVoter";
const MAX_LENGTH: &str = "MinerMaxLength";
#[allow(unused)]
const VOTER_SNAPSHOT_PER_BLOCK: &str = "VoterSnapshotPerBlock";

const MB_PAGES: PalletConstant<u32> = PalletConstant::new(MULTI_BLOCK, PAGES);
const MB_TARGET_SNAPSHOT_PER_BLOCK: PalletConstant<u32> =
	PalletConstant::new(MULTI_BLOCK, TARGET_SNAPSHOT_PER_BLOCK);
#[allow(unused)]
const MB_VOTER_SNAPSHOT_PER_BLOCK: PalletConstant<u32> =
	PalletConstant::new(EPM, VOTER_SNAPSHOT_PER_BLOCK);

const EPM_MAX_VOTES_PER_VOTER: PalletConstant<u32> = PalletConstant::new(EPM, MAX_VOTES_PER_VOTER);
const EPM_MAX_LENGTH: PalletConstant<u32> = PalletConstant::new(EPM, MAX_LENGTH);

type TypeId = u32;

#[derive(Copy, Clone, Debug)]
struct PalletConstant<T> {
	pallet: &'static str,
	variant: &'static str,
	_marker: std::marker::PhantomData<T>,
}

impl<T> std::fmt::Display for PalletConstant<T> {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_fmt(format_args!("{}::{}", self.pallet, self.variant))
	}
}

impl<T: DeserializeOwned + std::fmt::Display> PalletConstant<T> {
	const fn new(pallet: &'static str, variant: &'static str) -> Self {
		Self { pallet, variant, _marker: PhantomData }
	}
	const fn to_parts(&self) -> (&'static str, &'static str) {
		(self.pallet, self.variant)
	}

	pub fn fetch(&self, api: &ChainClient) -> Result<T, Error> {
		let (pallet, variant) = self.to_parts();

		let val = api
			.constants()
			.at(&subxt::dynamic::constant(pallet, variant))
			.map_err(|e| invalid_metadata_error(variant.to_string(), e))?
			.to_value()
			.map_err(|e| Error::Subxt(e.into()))?;

		let val = scale_value::serde::from_value::<_, T>(val).map_err(|e| {
			Error::InvalidMetadata(format!(
				"Decoding `{}` failed {}",
				std::any::type_name::<T>(),
				e
			))
		})?;

		log::trace!(target: LOG_TARGET, "updating metadata constant `{self}`: {val}",);
		Ok(val)
	}
}

pub(crate) fn update_metadata_constants(api: &ChainClient) -> Result<(), Error> {
	// multi block constants.
	let pages: u32 = MB_PAGES.fetch(api)?;
	static_types::Pages::set(pages);
	static_types::TargetSnapshotPerBlock::set(MB_TARGET_SNAPSHOT_PER_BLOCK.fetch(api)?);
	// TODO: hard coded.
	static_types::VoterSnapshotPerBlock::set(22500 / pages);
	static_types::MaxVotesPerVoter::set(16);
	static_types::MaxWinnersPerPage::set(100);

	// election provider constants.
	static_types::MaxBackersPerWinner::set(EPM_MAX_VOTES_PER_VOTER.fetch(api)?);
	static_types::MaxLength::set(EPM_MAX_LENGTH.fetch(api)?);

	Ok(())
}

/// Fetches the target snapshot.
///
/// Note: the target snapshot is single paged.
pub(crate) async fn target_snapshot(storage: &Storage) -> Result<TargetSnapshotPage, Error> {
	let page_idx = vec![Value::from(0u32)];
	let addr = subxt::dynamic::storage(EPM, "PagedTargetSnapshot", page_idx);

	match storage.fetch(&addr).await {
		Ok(Some(val)) => {
			let snapshot: TargetSnapshotPage = Decode::decode(&mut val.encoded())?;
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
pub(crate) async fn paged_voter_snapshot(
	page: u32,
	storage: &Storage,
) -> Result<VoterSnapshotPage, Error> {
	let page_idx = vec![Value::from(page)];
	let addr = subxt::dynamic::storage(EPM, "PagedVoterSnapshot", page_idx);

	match storage.fetch(&addr).await {
		Ok(Some(val)) => match Decode::decode(&mut val.encoded()) {
			Ok(s) => {
				let snapshot: VoterSnapshotPage = s;
				log::trace!(
					target: LOG_TARGET,
					"Voter snapshot page {:?} with len {:?}",
					page,
					snapshot.len()
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
pub(crate) async fn fetch_full_snapshots(
	n_pages: u32,
	storage: &Storage,
) -> Result<(TargetSnapshotPage, Vec<VoterSnapshotPage>), Error> {
	log::trace!(target: LOG_TARGET, "fetch_full_snapshots");

	let mut voters = vec![];
	let targets = target_snapshot(storage).await?;

	for page in 0..n_pages {
		let paged_voters = paged_voter_snapshot(page, storage).await?;
		voters.push(paged_voters);
	}

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
) -> Result<(), Error>
where
	T: MinerConfig<
			AccountId = AccountId,
			MaxVotesPerVoter = static_types::MaxVotesPerVoter,
			TargetSnapshotPerBlock = static_types::TargetSnapshotPerBlock,
			VoterSnapshotPerBlock = static_types::VoterSnapshotPerBlock,
			Pages = static_types::Pages,
		> + Send
		+ Sync
		+ 'static,
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
		// TODO: get from runtime/configs.
		desired_targets: 400,
		all_targets: targets.clone(),
		voter_pages: voters.clone(),
		pages: n_pages,
		// TODO: get from runtime/configs.
		do_reduce: false,
		round,
	};

	let paged_raw_solution = Miner::<T>::mine_solution(input).unwrap();

	// register solution.
	let register_tx = register_solution(paged_raw_solution.score)?;
	submit_and_watch::<T>(at, client, signer.clone(), listen, register_tx, "register").await?;

	// submit all solution pages.
	for (page, solution) in paged_raw_solution.solution_pages.into_iter().enumerate() {
		let submit_tx = submit_page::<T>(page as u32, Some(solution))?;
		submit_and_watch::<T>(at, client, signer.clone(), listen, submit_tx, "submit page").await?;
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
) -> Result<(), Error>
where
	T: MinerConfig<
			AccountId = AccountId,
			MaxVotesPerVoter = static_types::MaxVotesPerVoter,
			TargetSnapshotPerBlock = static_types::TargetSnapshotPerBlock,
			VoterSnapshotPerBlock = static_types::VoterSnapshotPerBlock,
			Pages = static_types::Pages,
		> + Send
		+ Sync
		+ 'static,
	T::Solution: Send,
{
	let (target_snapshot, voter_snapshot_paged) = fetch_full_snapshots(n_pages, storage).await?;
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
	)
	.await
}

/// Submits and watches a `DynamicPayload`, ie. an extrinsic.
async fn submit_and_watch<T: MinerConfig + Send + Sync + 'static>(
	at: &Header,
	client: &Client,
	signer: Signer,
	listen: Listen,
	tx: DynamicPayload,
	reason: &'static str,
) -> Result<(), Error> {
	let block_hash = at.hash();
	let nonce = client.rpc().system_account_next_index(signer.account_id()).await?;

	log::trace!(target: LOG_TARGET, "submit_and_watch for `{:?}` at {:?}", reason, block_hash);

	// TODO:set mortality.
	let xt_cfg = DefaultExtrinsicParamsBuilder::default().nonce(nonce).build();
	let xt = client.chain_api().tx().create_signed(&tx, &*signer, xt_cfg).await?;

	let tx_progress = xt.submit_and_watch().await.map_err(|e| {
		log::error!(target: LOG_TARGET, "submit solution failed: {:?}", e);
		e
	})?;

	match listen {
		Listen::Head => {
			let in_block = helpers::wait_for_in_block(tx_progress).await?;
			let _events = in_block.fetch_events().await.expect("events should exist");
			// TODO: finish
		},
		Listen::Finalized => {
			let finalized_block = tx_progress.wait_for_finalized().await?;
			let _block_hash = finalized_block.block_hash();
			let _finalized_success = finalized_block.wait_for_success().await?;
			// TODO: finish
		},
	}
	Ok(())
}

/// Helper to construct a register solution transaction.
fn register_solution(election_score: ElectionScore) -> Result<DynamicPayload, Error> {
	let scale_score = to_scale_value(election_score).map_err(|err| {
		Error::DynamicTransaction(format!("failed to encode `ElectionScore: {:?}", err))
	})?;

	Ok(subxt::dynamic::tx(EPM_SIGNED, "register", vec![scale_score]))
}

/// Helper to construct a submit page transaction.
fn submit_page<T: MinerConfig + 'static>(
	page: u32,
	maybe_solution: Option<T::Solution>,
) -> Result<DynamicPayload, Error> {
	let scale_page = to_scale_value(page)
		.map_err(|err| Error::DynamicTransaction(format!("failed to encode Page: {:?}", err)))?;

	let scale_solution = to_scale_value(maybe_solution).map_err(|err| {
		Error::DynamicTransaction(format!("failed to encode Solution: {:?}", err))
	})?;

	Ok(subxt::dynamic::tx(EPM_SIGNED, "submit_page", vec![scale_page, scale_solution]))
}

fn invalid_metadata_error<E: std::error::Error>(item: String, err: E) -> Error {
	Error::InvalidMetadata(format!("{} failed: {}", item, err))
}

fn make_type<T: scale_info::TypeInfo + 'static>() -> (TypeId, PortableRegistry) {
	let m = scale_info::MetaType::new::<T>();
	let mut types = scale_info::Registry::new();
	let id = types.register_type(&m);
	let portable_registry: PortableRegistry = types.into();

	(id.id, portable_registry)
}

fn to_scale_value<T: scale_info::TypeInfo + 'static + Encode>(val: T) -> Result<Value, Error> {
	let (ty_id, types) = make_type::<T>();

	let bytes = val.encode();

	decode_as_type(&mut bytes.as_ref(), ty_id, &types)
		.map(|v| v.remove_context())
		.map_err(|e| {
			Error::DynamicTransaction(format!(
				"Failed to decode {}: {:?}",
				std::any::type_name::<T>(),
				e
			))
		})
}
