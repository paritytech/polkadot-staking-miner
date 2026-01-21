//! Helpers for fetching and shaping election data shared by CLI commands

use polkadot_sdk::{
	frame_election_provider_support::{BoundedSupports, Get},
	frame_support::BoundedVec,
	pallet_election_provider_multi_block::{
		PagedRawSolution,
		unsigned::miner::{BaseMiner, MinerConfig},
	},
	sp_npos_elections::Support,
};

use std::{
	collections::{BTreeMap, HashMap, HashSet},
	time::{SystemTime, UNIX_EPOCH},
};

use crate::{
	commands::{
		multi_block::types::{TargetSnapshotPageOf, Voter, VoterSnapshotPageOf},
		types::{
			ElectionDataSource, NominatorAllocation, NominatorData, NominatorPrediction,
			NominatorsPrediction, PredictionMetadata, ValidatorData, ValidatorInfo,
			ValidatorStakeAllocation, ValidatorsPrediction,
		},
	},
	dynamic::staking::{fetch_targets, fetch_voters},
	error::Error,
	prelude::{AccountId, LOG_TARGET, Storage},
	static_types::multi_block::VoterSnapshotPerBlock,
	utils::{encode_account_id, planck_to_token, planck_to_token_u64},
};

use crate::dynamic::multi_block::try_fetch_snapshot;

/// Context for building predictions, grouping chain metadata and election parameters.
pub struct PredictionContext<'a> {
	pub round: u32,
	pub desired_targets: u32,
	pub block_number: u32,
	pub ss58_prefix: u16,
	pub token_decimals: u8,
	pub token_symbol: &'a str,
	pub data_source: ElectionDataSource,
}

/// Convert election data into the snapshot format expected by the miner.
///
/// Returns a single-page target snapshot and a Vec of voter pages
pub(crate) fn convert_election_data_to_snapshots<T>(
	candidates: Vec<ValidatorData>,
	voters: Vec<NominatorData>,
) -> Result<(TargetSnapshotPageOf<T>, Vec<VoterSnapshotPageOf<T>>), Error>
where
	T: MinerConfig<AccountId = AccountId>,
{
	log::info!(
		target: LOG_TARGET,
		"Converting election data to snapshots (candidates={}, voters={})",
		candidates.len(),
		voters.len()
	);

	// Extract only accounts from candidates
	let target_accounts: Vec<AccountId> =
		candidates.into_iter().map(|(account, _)| account).collect();
	log::info!(
		target: LOG_TARGET,
		"Fetched {} target accounts from candidates",
		target_accounts.len()
	);

	let total_targets = target_accounts.len();
	let target_snapshot: TargetSnapshotPageOf<T> = BoundedVec::truncate_from(target_accounts);
	if target_snapshot.len() < total_targets {
		log::warn!(
			target: LOG_TARGET,
			"Target snapshot truncated: kept {} of {} candidates ({} dropped)",
			target_snapshot.len(),
			total_targets,
			total_targets - target_snapshot.len()
		);
	}

	let per_voter_page = VoterSnapshotPerBlock::get();
	let total_voters = voters.len();
	log::info!(
		target: LOG_TARGET,
		"Preparing {total_voters} voters for conversion"
	);

	let mut voter_pages_vec: Vec<VoterSnapshotPageOf<T>> = Vec::new();
	for (stash, stake, votes) in voters {
		let votes: BoundedVec<AccountId, <T as MinerConfig>::MaxVotesPerVoter> =
			BoundedVec::truncate_from(votes);

		// voters → Voter<T> conversion
		let voter: Voter<T> = (stash, stake, votes);

		// Start a new page if we have no pages yet or the last page is full
		if voter_pages_vec.last().is_none_or(|last| last.len() >= per_voter_page as usize) {
			voter_pages_vec.push(BoundedVec::truncate_from(vec![voter]));
		} else {
			// Try to push to the last page; if it fails (unexpectedly full), start a new page
			match voter_pages_vec.last_mut().unwrap().try_push(voter.clone()) {
				Ok(_) => {},
				Err(_) => {
					let last_idx = voter_pages_vec.len().saturating_sub(1);
					let last_len = voter_pages_vec.last().map(|p| p.len()).unwrap_or(0);
					log::warn!(
						target: LOG_TARGET,
						"Voter page {last_idx} unexpectedly full at size {last_len}; starting new page"
					);
					voter_pages_vec.push(BoundedVec::truncate_from(vec![voter]));
				},
			}
		}
	}

	let n_pages = voter_pages_vec.len();

	log::info!(
		target: LOG_TARGET,
		"Converted election data: {} targets, {} voters across {} pages",
		target_snapshot.len(),
		total_voters,
		n_pages
	);

	Ok((target_snapshot, voter_pages_vec))
}

/// Build structured predictions from the mined solution and snapshots.
pub(crate) fn build_predictions_from_solution<T>(
	solution: &PagedRawSolution<T>,
	target_snapshot: &TargetSnapshotPageOf<T>,
	voter_snapshot: &[VoterSnapshotPageOf<T>],
	ctx: &PredictionContext<'_>,
) -> Result<(ValidatorsPrediction, NominatorsPrediction), Error>
where
	T: MinerConfig<AccountId = AccountId>,
{
	// Convert slice to BoundedVec for feasibility check (truncates to T::Pages if needed)
	let voter_pages_bounded: BoundedVec<VoterSnapshotPageOf<T>, T::Pages> =
		BoundedVec::truncate_from(voter_snapshot.to_vec());

	// Reuse the on-chain feasibility logic to reconstruct supports from the paged solution.
	let page_supports = BaseMiner::<T>::check_feasibility(
		solution,
		&voter_pages_bounded,
		target_snapshot,
		ctx.desired_targets,
	)
	.map_err(|err| Error::Other(format!("Failed to evaluate solution supports: {err:?}")))?;

	let mut winner_support_map: BTreeMap<AccountId, Support<AccountId>> = BTreeMap::new();

	for page_support in page_supports {
		let BoundedSupports(inner) = page_support;
		for (winner, bounded_support) in inner.into_iter() {
			let support: Support<AccountId> = bounded_support.into();
			let entry = winner_support_map
				.entry(winner)
				.or_insert_with(|| Support { total: 0, voters: Vec::new() });
			entry.total = entry.total.saturating_add(support.total);
			entry.voters.extend(support.voters);
		}
	}

	// Build allocation map per nominator for quick lookup.
	let mut allocation_map: HashMap<AccountId, HashMap<AccountId, u128>> = HashMap::new();
	for (validator, support) in winner_support_map.iter() {
		for (voter, stake) in support.voters.iter() {
			allocation_map
				.entry(voter.clone())
				.or_default()
				.entry(validator.clone())
				.and_modify(|existing| *existing = existing.saturating_add(*stake))
				.or_insert(*stake);
		}
	}

	// Sort winners by backing and enforce desired_targets limit.
	let mut winners_sorted: Vec<(AccountId, Support<AccountId>)> =
		winner_support_map.into_iter().collect();
	winners_sorted.sort_by(|a, b| b.1.total.cmp(&a.1.total));
	if winners_sorted.len() > ctx.desired_targets as usize {
		winners_sorted.truncate(ctx.desired_targets as usize);
	}

	let active_set: HashSet<AccountId> =
		winners_sorted.iter().map(|(validator, _)| validator.clone()).collect();

	// Flatten voters from paged snapshot for nominator perspective.
	let all_voters: Vec<Voter<T>> =
		voter_snapshot.iter().flat_map(|page| page.iter().cloned()).collect();

	// Identify validators who only have self-votes
	let validators_with_only_self_vote: HashSet<AccountId> = all_voters
		.iter()
		.filter(|(nominator, _, targets)| {
			// validator has only self-vote if:
			// 1. They are a validator (in active_set)
			// 2. Their only target is themselves
			// NOTE: Reverted to your original logic as requested, assuming you want strictly this
			// behavior.
			active_set.contains(nominator) || (targets.len() == 1 && targets[0] == *nominator)
		})
		.map(|(nominator, _, _)| nominator.clone())
		.collect();

	let mut validator_infos: Vec<ValidatorInfo> = Vec::new();
	for (validator, support) in winners_sorted.iter() {
		let self_stake = support
			.voters
			.iter()
			.find(|(who, _)| who == validator)
			.map(|(_, stake)| *stake)
			.unwrap_or(0);

		// Collect nominators backing this validator (excluding self-votes)
		let mut validator_nominators: Vec<(AccountId, u128)> = support
			.voters
			.iter()
			.filter(|(who, _)| who != validator)
			.map(|(who, stake)| (who.clone(), *stake))
			.collect();
		// Sort by stake descending for consistent ordering
		validator_nominators.sort_by(|a, b| b.1.cmp(&a.1));

		let nominator_allocations = validator_nominators
			.iter()
			.map(|(nominator, stake)| NominatorAllocation {
				address: encode_account_id(nominator, ctx.ss58_prefix),
				allocated_stake: planck_to_token(*stake, ctx.token_decimals, ctx.token_symbol),
			})
			.collect();

		validator_infos.push(ValidatorInfo {
			account: encode_account_id(validator, ctx.ss58_prefix),
			total_stake: planck_to_token(support.total, ctx.token_decimals, ctx.token_symbol),
			self_stake: planck_to_token(self_stake, ctx.token_decimals, ctx.token_symbol),
			nominator_count: validator_nominators.len(),
			nominators: nominator_allocations,
		});
	}

	let timestamp = SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.map(|d| d.as_secs().to_string())
		.unwrap_or_else(|_| "0".to_string());

	let data_source_str = match &ctx.data_source {
		ElectionDataSource::Snapshot => "snapshot",
		ElectionDataSource::Staking => "staking",
		ElectionDataSource::CustomData => "custom_data",
	}
	.to_string();

	let metadata = PredictionMetadata {
		timestamp,
		desired_validators: ctx.desired_targets,
		round: ctx.round,
		block_number: ctx.block_number,
		solution_score: Some(solution.score),
		data_source: data_source_str,
	};

	let validators_prediction = ValidatorsPrediction { metadata, results: validator_infos };

	// Build nominator predictions, excluding validators who only have self-votes
	let mut nominator_predictions: Vec<NominatorPrediction> = Vec::new();

	for (nominator, stake, nominated_targets) in all_voters {
		// Skip validators who only have self-votes
		if validators_with_only_self_vote.contains(&nominator) {
			continue;
		}

		let nominator_encoded = encode_account_id(&nominator, ctx.ss58_prefix);
		let allocations = allocation_map.get(&nominator);

		let mut active_supported = Vec::new();
		let mut inactive = Vec::new();
		let mut waiting = Vec::new();

		for target in nominated_targets.iter() {
			let encoded = encode_account_id(target, ctx.ss58_prefix);
			let is_winner = active_set.contains(target);
			let allocated = allocations.and_then(|m| m.get(target)).copied().unwrap_or(0);

			if is_winner && allocated > 0 {
				active_supported.push(ValidatorStakeAllocation {
					validator: encoded,
					allocated_stake: planck_to_token(
						allocated,
						ctx.token_decimals,
						ctx.token_symbol,
					),
				});
			} else if is_winner {
				inactive.push(encoded);
			} else {
				waiting.push(encoded);
			}
		}

		nominator_predictions.push(NominatorPrediction {
			address: nominator_encoded,
			stake: planck_to_token_u64(stake, ctx.token_decimals, ctx.token_symbol),
			active_validators: active_supported,
			inactive_validators: inactive,
			waiting_validators: waiting,
		});
	}

	let nominators_prediction = NominatorsPrediction { nominators: nominator_predictions };

	Ok((validators_prediction, nominators_prediction))
}

/// Fetch snapshots from chain or synthesize them from staking storage when snapshot is unavailable.
pub(crate) async fn get_election_data<T>(
	n_pages: u32,
	round: u32,
	storage: Storage,
) -> Result<(TargetSnapshotPageOf<T>, Vec<VoterSnapshotPageOf<T>>, ElectionDataSource), Error>
where
	T: MinerConfig<AccountId = AccountId> + Send + Sync + 'static,
	T::Solution: Send,
	T::Pages: Send,
	T::TargetSnapshotPerBlock: Send,
	T::VoterSnapshotPerBlock: Send,
	T::MaxVotesPerVoter: Send,
{
	// try to fetch election data from the snapshot
	// if snapshot is not available fetch from staking
	log::info!(target: LOG_TARGET, "Trying to fetch data from snapshot");

	match try_fetch_snapshot::<T>(n_pages, round, &storage).await {
		Ok((target_snapshot, voter_pages)) => {
			log::info!(target: LOG_TARGET, "Snapshot found");
			// Convert BoundedVec to Vec so callers get all pages
			let voter_pages_vec: Vec<VoterSnapshotPageOf<T>> = voter_pages.into_iter().collect();
			Ok((target_snapshot, voter_pages_vec, ElectionDataSource::Snapshot))
		},
		Err(err) => {
			log::warn!(target: LOG_TARGET, "Fetching from Snapshot failed: {err}. Falling back to staking pallet");

			let targets = fetch_targets(&storage)
				.await
				.map_err(|e| Error::Other(format!("Failed to fetch targets: {e}")))?;

			let voter_limit = (T::Pages::get() * T::VoterSnapshotPerBlock::get()) as usize;

			let voters = fetch_voters(voter_limit, &storage)
				.await
				.map_err(|e| Error::Other(format!("Failed to fetch voters: {e}")))?;

			let (target_snapshot, mut voter_snapshot) =
				convert_election_data_to_snapshots::<T>(targets, voters)?;
			// Fix the order
			voter_snapshot.reverse();
			Ok((target_snapshot, voter_snapshot, ElectionDataSource::Staking))
		},
	}
}
