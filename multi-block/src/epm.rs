use crate::{
	client::Client, commands::monitor::Listen, error::Error, helpers, prelude::*, signer::Signer,
	static_types,
};
use pallet_election_provider_multi_block::unsigned::miner;
use sp_npos_elections::ElectionScore;

use codec::{Decode, Encode};
use frame_support::BoundedVec;
use scale_info::PortableRegistry;
use scale_value::scale::decode_as_type;
use subxt::{
	config::{DefaultExtrinsicParamsBuilder, Header as _},
	dynamic::Value,
	tx::DynamicPayload,
};

const EPM_PALLET_NAME: &str = "ElectionProviderMultiBlock";
const EPM_SIGNED_PALLET_NAME: &str = "ElectionSignedPallet";

type TypeId = u32;
type PagedRawSolutionOf<T> = pallet_election_provider_multi_block::types::PagedRawSolution<T>;

#[derive(Copy, Clone, Debug)]
struct EpmConstant {
	epm: &'static str,
	constant: &'static str,
}

impl std::fmt::Display for EpmConstant {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_fmt(format_args!("{}::{}", self.epm, self.constant))
	}
}

impl EpmConstant {
	const fn new(constant: &'static str) -> Self {
		Self { epm: EPM_PALLET_NAME, constant }
	}
	const fn to_parts(self) -> (&'static str, &'static str) {
		(self.epm, self.constant)
	}
}

pub(crate) fn update_metadata_constants(api: &ChainClient) -> Result<(), Error> {
	const PAGES: EpmConstant = EpmConstant::new("Pages");
	const TARGET_SNAPSHOT_PER_BLOCK: EpmConstant = EpmConstant::new("TargetSnapshotPerBlock");
	const VOTER_SNAPSHOT_PER_BLOCK: EpmConstant = EpmConstant::new("VoterSnapshotPerBlock");
	const MAX_VOTES_PER_VOTER: EpmConstant = EpmConstant::new("MinerMaxVotesPerVoter");
	const MAX_BACKERS_PER_WINNER: EpmConstant = EpmConstant::new("MinerMaxBackersPerWinner");
	const MAX_WINNERS_PER_PAGE: EpmConstant = EpmConstant::new("MinerMaxWinnersPerPage");

	let pages: u32 = read_constant(api, PAGES)?;
	let target_snapshot_per_block: u32 = read_constant(api, TARGET_SNAPSHOT_PER_BLOCK)?;
	let voter_snapshot_per_block: u32 = read_constant(api, VOTER_SNAPSHOT_PER_BLOCK)?;
	let max_votes_per_voter: u32 = read_constant(api, MAX_VOTES_PER_VOTER)?;
	let max_backers_per_winner: u32 = read_constant(api, MAX_BACKERS_PER_WINNER)?;
	let max_winners_per_page: u32 = read_constant(api, MAX_WINNERS_PER_PAGE)?;

	fn log_metadata(metadata: EpmConstant, val: impl std::fmt::Display) {
		log::trace!(target: LOG_TARGET, "updating metadata constant `{metadata}`: {val}",);
	}

	log_metadata(PAGES, pages);
	log_metadata(TARGET_SNAPSHOT_PER_BLOCK, target_snapshot_per_block);
	log_metadata(VOTER_SNAPSHOT_PER_BLOCK, voter_snapshot_per_block);
	log_metadata(MAX_VOTES_PER_VOTER, max_votes_per_voter);
	log_metadata(MAX_BACKERS_PER_WINNER, max_backers_per_winner);
	log_metadata(MAX_WINNERS_PER_PAGE, max_winners_per_page);

	static_types::Pages::set(pages);
	static_types::TargetSnapshotPerBlock::set(target_snapshot_per_block);
	static_types::VoterSnapshotPerBlock::set(voter_snapshot_per_block);
	static_types::MaxVotesPerVoter::set(max_votes_per_voter);
	static_types::MaxBackersPerWinner::set(max_backers_per_winner);
	static_types::MaxWinnersPerPage::set(max_winners_per_page);

	Ok(())
}

pub(crate) async fn target_snapshot(storage: &Storage) -> Result<TargetSnapshotPage, Error> {
	// target snapshot has *always* one page.
	let page_idx = vec![Value::from(0u32)];
	let addr = subxt::dynamic::storage(EPM_PALLET_NAME, "PagedTargetSnapshot", page_idx);

	match storage.fetch(&addr).await {
		Ok(Some(val)) => {
			let snapshot: TargetSnapshotPage = Decode::decode(&mut val.encoded())?;
			log::info!(
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

pub(crate) async fn paged_voter_snapshot(
	page: u32,
	storage: &Storage,
) -> Result<VoterSnapshotPage, Error> {
	let page_idx = vec![Value::from(page)];
	let addr = subxt::dynamic::storage(EPM_PALLET_NAME, "PagedVoterSnapshot", page_idx);

	match storage.fetch(&addr).await {
		Ok(Some(val)) => match Decode::decode(&mut val.encoded()) {
			Ok(s) => {
				let snapshot: VoterSnapshotPage = s;
				log::info!(
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

	log::info!(target: LOG_TARGET, "fetch_full_snapshots: voters with {} pages; targets with {} len", voters.len(), targets.len());

	Ok((targets, voters))
}

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
	T: miner::Config<
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
	// TODO: get from runtime/configs.
	let do_reduce = false;
	let desired_targets = 400;

	log::trace!(
		target: LOG_TARGET,
		"Mine, submit: election target snap size: {:?}, voter snap size: {:?}",
		target_snapshot.len(),
		voter_snapshot_paged.len()
	);

	let targets: TargetSnapshotPageOf<T> = target_snapshot.clone(); // one page.
	let voters: BoundedVec<VoterSnapshotPageOf<T>, T::Pages> =
		BoundedVec::truncate_from(voter_snapshot_paged.clone());

	let (paged_raw_solution, _trimming_status): (PagedRawSolutionOf<T>, _) =
		miner::Miner::<T>::mine_paged_solution_with_snapshot(
			&voters,
			&targets,
			n_pages,
			round,
			desired_targets,
			do_reduce,
		)
		.unwrap(); // TODO: handle err

	let solution_pages: Vec<(u32, ElectionScore, <T as miner::Config>::Solution)> =
		paged_raw_solution
			.solution_pages
			.iter()
			.enumerate()
			.map(|(page, solution)| {
				let partial_score = miner::Miner::<T>::compute_partial_score(
					&voters,
					&targets,
					solution,
					desired_targets,
					page as u32,
				)
				.map_err(|_| Error::Other("MinerError - computing solution".to_string()))?;

				Ok((page as u32, partial_score, solution.to_owned()))
			})
			.collect::<Result<Vec<_>, Error>>()?;

	// register solution.
	let register_tx = register_solution(paged_raw_solution.score)?;
	submit_and_watch::<T>(at, client, signer.clone(), listen, register_tx, "register").await?;

	// submit all solution pages.
	for (page, _score, solution) in solution_pages {
		let submit_tx = submit_page::<T>(page, Some(solution))?;
		submit_and_watch::<T>(at, client, signer.clone(), listen, submit_tx, "submit page").await?;
	}

	Ok(())
}

// TODO: move this down, doc.
async fn submit_and_watch<T: miner::Config + Send + Sync + 'static>(
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
			// TODO
		},
		Listen::Finalized => {
			let finalized_block = tx_progress.wait_for_finalized().await?;
			let _block_hash = finalized_block.block_hash();
			let _finalized_success = finalized_block.wait_for_success().await?;
			// TODO
		},
	}
	Ok(())
}

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
	T: miner::Config<
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
	log::info!(target: LOG_TARGET, "fetch_mine_and_compute");

	let (target_snapshot, voter_snapshot_paged) = fetch_full_snapshots(n_pages, storage).await?;
	log::info!(
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

/// Helper to constrct a register solution transaction.
fn register_solution(election_score: ElectionScore) -> Result<DynamicPayload, Error> {
	let scale_score = to_scale_value(election_score).map_err(|err| {
		Error::DynamicTransaction(format!("failed to encode `ElectionScore: {:?}", err))
	})?;

	Ok(subxt::dynamic::tx(EPM_SIGNED_PALLET_NAME, "register", vec![scale_score]))
}

/// Helper to constrct a submit page transaction.
fn submit_page<T: miner::Config + 'static>(
	page: u32,
	maybe_solution: Option<T::Solution>,
) -> Result<DynamicPayload, Error> {
	let scale_page = to_scale_value(page)
		.map_err(|err| Error::DynamicTransaction(format!("failed to encode Page: {:?}", err)))?;

	let scale_solution = to_scale_value(maybe_solution).map_err(|err| {
		Error::DynamicTransaction(format!("failed to encode Solution: {:?}", err))
	})?;

	Ok(subxt::dynamic::tx(EPM_SIGNED_PALLET_NAME, "submit_page", vec![scale_page, scale_solution]))
}

fn read_constant<'a, T: serde::Deserialize<'a>>(
	api: &ChainClient,
	constant: EpmConstant,
) -> Result<T, Error> {
	let (epm_name, constant) = constant.to_parts();

	let val = api
		.constants()
		.at(&subxt::dynamic::constant(epm_name, constant))
		.map_err(|e| invalid_metadata_error(constant.to_string(), e))?
		.to_value()
		.map_err(|e| Error::Subxt(e.into()))?;

	scale_value::serde::from_value::<_, T>(val).map_err(|e| {
		Error::InvalidMetadata(format!("Decoding `{}` failed {}", std::any::type_name::<T>(), e))
	})
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
