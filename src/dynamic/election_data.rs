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
			ElectionDataSource, ElectionOverrides, NominatorAllocation, NominatorData,
			NominatorPrediction, NominatorsPrediction, OverridesConfig, PredictionMetadata,
			ValidatorData, ValidatorInfo, ValidatorStakeAllocation, ValidatorsPrediction,
		},
	},
	dynamic::staking::{fetch_candidates, fetch_voters},
	error::Error,
	prelude::{AccountId, AtBlock, LOG_TARGET},
	utils::{encode_account_id, planck_to_token, planck_to_token_u64, read_data_from_json_file},
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
	mut voters: Vec<NominatorData>,
	data_source: ElectionDataSource,
) -> Result<(TargetSnapshotPageOf<T>, Vec<VoterSnapshotPageOf<T>>), Error>
where
	T: MinerConfig<AccountId = AccountId>,
{
	log::debug!(
		target: LOG_TARGET,
		"Converting election data to snapshots (candidates={}, voters={})",
		candidates.len(),
		voters.len()
	);

	// Extract only accounts from candidates
	let target_accounts: Vec<AccountId> =
		candidates.into_iter().map(|(account, _)| account).collect();
	log::trace!(
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

	let per_voter_page = T::VoterSnapshotPerBlock::get();

	// The miner mines at most `T::Pages` pages and bounds the voter pages to that many, so an extra
	// page is dropped along with every voter in it. Worse, staking data is paginated
	// strongest-first and then reversed below, which makes the dropped page the strongest one.
	// Only overrides can exceed the capacity (fetched data is already truncated to it), so let the
	// weakest voters make room instead: everyone is ranked by stake and the tail is cut.
	let capacity = (T::Pages::get() as usize).saturating_mul(per_voter_page as usize);
	if capacity > 0 && voters.len() > capacity {
		let mut by_stake: Vec<usize> = (0..voters.len()).collect();
		by_stake.sort_unstable_by_key(|&i| std::cmp::Reverse(voters[i].1));
		let dropped: HashSet<usize> = by_stake[capacity..].iter().copied().collect();

		log::warn!(
			target: LOG_TARGET,
			"Voter count {} exceeds the snapshot capacity of {capacity}; dropping the {} weakest voters",
			voters.len(),
			dropped.len()
		);

		let mut index = 0;
		voters.retain(|_| {
			let keep = !dropped.contains(&index);
			index += 1;
			keep
		});
	}

	let total_voters = voters.len();
	log::trace!(
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

	log::debug!(
		target: LOG_TARGET,
		"Converted election data: {} targets, {} voters across {} pages",
		target_snapshot.len(),
		total_voters,
		n_pages
	);

	// When fetching from staking data, voters come from BagsList in descending order (highest
	// stake first). The SDK expects page 0 (lsp) to contain lowest stake voters and page n-1
	// (msp) to contain highest stake voters. Reversing ensures correct page assignment during
	// pagination.
	if matches!(data_source, ElectionDataSource::Staking) {
		voter_pages_vec.reverse();
	}

	Ok((target_snapshot, voter_pages_vec))
}

/// Apply election overrides to candidates and voters.
pub(crate) fn apply_overrides(
	mut candidates: Vec<ValidatorData>,
	mut voters: Vec<NominatorData>,
	overrides: ElectionOverrides,
) -> Result<(Vec<ValidatorData>, Vec<NominatorData>), Error> {
	// (1) Remove specific candidates from the election
	let candidates_exclude: HashSet<AccountId> = overrides
		.candidates_exclude
		.iter()
		.map(|c| {
			c.parse::<AccountId>()
				.map_err(|e| Error::Other(format!("Invalid candidate exclude {c}: {e}")))
		})
		.collect::<Result<_, _>>()?;

	candidates.retain(|(account, _)| !candidates_exclude.contains(account));

	// (2) Add candidates that may not exist on-chain, with an optional self-stake.
	//
	// The target snapshot carries no stake, so a candidate's self-stake only reaches the election
	// through the voter snapshot, as a voter whose sole target is itself (what the chain calls an
	// implicit self-vote). A candidate without one has zero approval stake and can never be
	// elected, hence each self-stake is materialized as a self-vote below.
	//
	// An entry for an account already present OVERRIDES its self-stake, replacing any existing
	// vote of that account with the self-vote. This is what makes "what if validator X bonded N?"
	// expressible. `voters_include` is applied afterwards and therefore wins over it.
	//
	// A bare address (no self-stake, the legacy format) only registers the candidate and leaves
	// the account's votes untouched.
	for candidate_include in &overrides.candidates_include {
		let (address, self_stake) = candidate_include.parts();
		let account = address
			.parse::<AccountId>()
			.map_err(|e| Error::Other(format!("Invalid candidate include {address}: {e}")))?;

		match candidates.iter_mut().find(|(a, _)| a == &account) {
			Some((_, stake)) => *stake = u128::from(self_stake),
			None => candidates.push((account.clone(), u128::from(self_stake))),
		}

		if self_stake == 0 {
			continue;
		}

		let self_vote = (account.clone(), self_stake, vec![account.clone()]);
		match voters.iter_mut().find(|(a, _, _)| a == &account) {
			Some(voter) => {
				if voter.2 != self_vote.2 {
					log::warn!(
						target: LOG_TARGET,
						"Candidate include {address}: replacing its {} existing nomination(s) with a self-vote of {self_stake}",
						voter.2.len()
					);
				}
				*voter = self_vote;
			},
			None => voters.push(self_vote),
		}
	}

	// (3) Remove specific voters from the election
	let voters_exclude: HashSet<AccountId> = overrides
		.voters_exclude
		.iter()
		.map(|v| {
			v.parse::<AccountId>()
				.map_err(|e| Error::Other(format!("Invalid voter exclude {v}: {e}")))
		})
		.collect::<Result<_, _>>()?;

	voters.retain(|(account, _, _)| !voters_exclude.contains(account));

	// (4) Add or override voters with custom stake amounts
	let voter_map: HashMap<AccountId, usize> =
		voters.iter().enumerate().map(|(i, (a, _, _))| (a.clone(), i)).collect();

	for (v_str, stake, t_strs) in overrides.voters_include {
		let account = v_str
			.parse::<AccountId>()
			.map_err(|e| Error::Other(format!("Invalid voter include {v_str}: {e}")))?;
		let targets: Vec<AccountId> = t_strs
			.iter()
			.map(|t| {
				t.parse::<AccountId>()
					.map_err(|e| Error::Other(format!("Invalid voter target {t}: {e}")))
			})
			.collect::<Result<_, _>>()?;

		if let Some(&index) = voter_map.get(&account) {
			voters[index] = (account, stake, targets);
		} else {
			voters.push((account, stake, targets));
		}
	}

	Ok((candidates, voters))
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
	winners_sorted.sort_by_key(|b| std::cmp::Reverse(b.1.total));
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
			// validator has only self-vote if either:
			// 1. They are a validator (in active_set)
			// 2. Their only target is themselves

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
		validator_nominators.sort_by_key(|b| std::cmp::Reverse(b.1));

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

/// Fetch snapshot raw data from chain or synthesize from staking storage when snapshot is
/// unavailable.
pub(crate) async fn get_election_data<T>(
	n_pages: u32,
	round: u32,
	at_block: AtBlock,
) -> Result<(Vec<ValidatorData>, Vec<NominatorData>, ElectionDataSource), Error>
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

	match try_fetch_snapshot::<T>(n_pages, round, &at_block).await {
		Ok((target_snapshot, voter_pages)) => {
			log::info!(target: LOG_TARGET, "Snapshot found");

			let candidates: Vec<ValidatorData> =
				target_snapshot.into_iter().map(|a| (a, 0)).collect();

			let voters: Vec<NominatorData> = voter_pages
				.into_iter()
				.flat_map(|page| {
					page.into_iter().map(|(stash, stake, votes)| {
						(stash, stake, votes.into_iter().collect::<Vec<_>>())
					})
				})
				.collect();

			Ok((candidates, voters, ElectionDataSource::Snapshot))
		},
		Err(err) => {
			log::warn!(target: LOG_TARGET, "Fetching from Snapshot failed: {err}. Falling back to staking pallet");

			let candidates = fetch_candidates(&at_block)
				.await
				.map_err(|e| Error::Other(format!("Failed to fetch candidates: {e}")))?;

			let voter_limit = (T::Pages::get() * T::VoterSnapshotPerBlock::get()) as usize;

			let voters = fetch_voters(voter_limit, &at_block)
				.await
				.map_err(|e| Error::Other(format!("Failed to fetch voters: {e}")))?;

			Ok((candidates, voters, ElectionDataSource::Staking))
		},
	}
}

/// Fetch snapshots from chain or synthesize them from staking storage when snapshot is unavailable.
pub(crate) async fn fetch_snapshots<T>(
	n_pages: u32,
	current_round: u32,
	at_block: &AtBlock,
	overrides: Option<OverridesConfig>,
) -> Result<(TargetSnapshotPageOf<T>, Vec<VoterSnapshotPageOf<T>>, ElectionDataSource), Error>
where
	T: MinerConfig<AccountId = AccountId> + Send + Sync + 'static,
	T::Solution: Send,
	T::Pages: Send,
	T::TargetSnapshotPerBlock: Send,
	T::VoterSnapshotPerBlock: Send,
	T::MaxVotesPerVoter: Send,
{
	// Fetch election data
	let (candidates, nominators, data_source) =
		get_election_data::<T>(n_pages, current_round, at_block.clone()).await?;

	// Apply overrides if provided
	let (candidates, nominators) = if let Some(overrides_config) = overrides {
		let overrides_to_apply = match overrides_config {
			OverridesConfig::Path(path) => {
				log::info!(target: LOG_TARGET, "Applying overrides from {path}");
				read_data_from_json_file(&path).await?
			},
			OverridesConfig::Data(data) => {
				log::info!(target: LOG_TARGET, "Applying overrides");
				data
			},
		};
		apply_overrides(candidates, nominators, overrides_to_apply)?
	} else {
		(candidates, nominators)
	};

	// Convert raw data to snapshots
	let (target_snapshot, voter_snapshot) =
		convert_election_data_to_snapshots::<T>(candidates, nominators, data_source.clone())?;

	Ok((target_snapshot, voter_snapshot, data_source))
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		commands::types::{CandidateInclude, ElectionOverrides},
		dynamic::multi_block::mine_solution,
		prelude::{Accuracy, Hash},
		static_types::multi_block::{BalancingIterations, DynamicSolver},
	};
	use polkadot_sdk::{
		frame_election_provider_support, frame_support,
		sp_core::crypto::Ss58Codec,
		sp_runtime::{PerU16, traits::ConstU32},
	};

	const UNIT: u64 = 1_000_000_000_000;

	const TEST_PAGES: u32 = 1;
	const TEST_VOTERS_PER_PAGE: u32 = 100;

	frame_election_provider_support::generate_solution_type!(
		#[compact]
		pub struct TestNposSolution16::<
			VoterIndex = u32,
			TargetIndex = u16,
			Accuracy = PerU16,
			MaxVoters = ConstU32::<TEST_VOTERS_PER_PAGE>
		>(16)
	);

	pub struct TestMinerConfig;

	impl MinerConfig for TestMinerConfig {
		type AccountId = AccountId;
		type Solution = TestNposSolution16;
		type Solver = DynamicSolver<AccountId, Accuracy, BalancingIterations>;
		type Pages = ConstU32<TEST_PAGES>;
		type MaxVotesPerVoter = ConstU32<16>;
		type MaxWinnersPerPage = ConstU32<10>;
		type MaxBackersPerWinner = ConstU32<16>;
		type MaxBackersPerWinnerFinal = ConstU32<{ u32::MAX }>;
		type VoterSnapshotPerBlock = ConstU32<TEST_VOTERS_PER_PAGE>;
		type TargetSnapshotPerBlock = ConstU32<1000>;
		type MaxLength = ConstU32<{ 1024 * 1024 }>;
		type Hash = Hash;
	}

	fn account(id: u8) -> AccountId {
		AccountId::from([id; 32])
	}

	fn no_overrides() -> ElectionOverrides {
		ElectionOverrides {
			candidates_include: vec![],
			candidates_exclude: vec![],
			voters_include: vec![],
			voters_exclude: vec![],
		}
	}

	#[test]
	fn test_apply_overrides_logic() {
		// Create some test accounts
		let acc1 = AccountId::from([1u8; 32]);
		let acc2 = AccountId::from([2u8; 32]);
		let acc3 = AccountId::from([3u8; 32]);
		let acc4 = AccountId::from([4u8; 32]);

		let s1 = acc1.to_ss58check();
		let s2 = acc2.to_ss58check();
		let s3 = acc3.to_ss58check();
		let s4 = acc4.to_ss58check();

		let candidates = vec![(acc1.clone(), 1000), (acc2.clone(), 2000)];

		let voters = vec![(acc3.clone(), 500, vec![acc1.clone()])];

		// Override:
		// - Remove acc1 candidate
		// - Add acc4 candidate
		// - Remove acc3 voter
		// - Add acc4 voter with targets [acc2, acc4]
		let overrides = ElectionOverrides {
			candidates_include: vec![CandidateInclude::Address(s4.clone())],
			candidates_exclude: vec![s1.clone()],
			voters_include: vec![(s4.clone(), 1500, vec![s2.clone(), s4.clone()])],
			voters_exclude: vec![s3.clone()],
		};

		let (new_candidates, new_voters) = apply_overrides(candidates, voters, overrides).unwrap();

		// Check candidates
		assert_eq!(new_candidates.len(), 2);
		assert!(new_candidates.iter().any(|(a, _)| a == &acc2));
		assert!(new_candidates.iter().any(|(a, s)| a == &acc4 && *s == 0));
		assert!(!new_candidates.iter().any(|(a, _)| a == &acc1));

		// Check voters: a bare address adds no self-vote, so acc4 only votes via voters_include
		assert_eq!(new_voters.len(), 1);
		assert_eq!(new_voters[0].0, acc4);
		assert_eq!(new_voters[0].1, 1500);
		assert_eq!(new_voters[0].2, vec![acc2, acc4]);
	}

	/// A candidate self-stake must show up as a self-vote, otherwise the election never sees it.
	#[test]
	fn candidate_include_self_stake_adds_self_vote() {
		// GIVEN one on-chain candidate backed by one nominator
		let (existing, injected, bare, nominator) =
			(account(1), account(9), account(8), account(3));
		let candidates = vec![(existing.clone(), 1000)];
		let voters = vec![(nominator.clone(), 500, vec![existing.clone()])];

		// WHEN two candidates are injected, only one of them with a self-stake
		let overrides = ElectionOverrides {
			candidates_include: vec![
				CandidateInclude::WithSelfStake(injected.to_ss58check(), 7 * UNIT),
				CandidateInclude::Address(bare.to_ss58check()),
			],
			..no_overrides()
		};
		let (candidates, voters) = apply_overrides(candidates, voters, overrides).unwrap();

		// THEN both are candidates, but only the one with a self-stake votes for itself
		assert!(candidates.iter().any(|(a, s)| a == &injected && *s == u128::from(7 * UNIT)));
		assert!(candidates.iter().any(|(a, s)| a == &bare && *s == 0));
		assert_eq!(
			voters,
			vec![(nominator, 500, vec![existing]), (injected.clone(), 7 * UNIT, vec![injected]),]
		);
	}

	/// Injecting an account that is already in the data overrides its self-stake, so that
	/// "what if this validator raised its self-stake, keeping its nominators?" is expressible:
	/// only the validator's own self-vote is rewritten, the votes cast *into* it are left alone.
	#[test]
	fn candidate_include_self_stake_overrides_existing_stake() {
		// GIVEN a candidate that already self-votes, and a nominator of that candidate
		let (validator, nominator) = (account(1), account(3));
		let candidates = vec![(validator.clone(), u128::from(UNIT))];
		let voters = vec![
			(validator.clone(), UNIT, vec![validator.clone()]),
			(nominator.clone(), 500, vec![validator.clone()]),
		];

		// WHEN the same account is injected with a higher self-stake
		let overrides = ElectionOverrides {
			candidates_include: vec![CandidateInclude::WithSelfStake(
				validator.to_ss58check(),
				9 * UNIT,
			)],
			..no_overrides()
		};
		let (candidates, voters) = apply_overrides(candidates, voters, overrides).unwrap();

		// THEN its self-vote is updated in place, leaving everything else alone
		assert_eq!(candidates, vec![(validator.clone(), u128::from(9 * UNIT))]);
		assert_eq!(
			voters,
			vec![
				(validator.clone(), 9 * UNIT, vec![validator]),
				(nominator, 500, vec![account(1)]),
			]
		);
	}

	/// Turning a nominator into a candidate replaces its nominations with the self-vote.
	#[test]
	fn candidate_include_self_stake_replaces_nominations() {
		let (nominator, target_a, target_b) = (account(3), account(1), account(2));
		let voters = vec![(nominator.clone(), 500, vec![target_a, target_b])];

		let overrides = ElectionOverrides {
			candidates_include: vec![CandidateInclude::WithSelfStake(
				nominator.to_ss58check(),
				4 * UNIT,
			)],
			..no_overrides()
		};
		let (candidates, voters) = apply_overrides(vec![], voters, overrides).unwrap();

		assert_eq!(candidates, vec![(nominator.clone(), u128::from(4 * UNIT))]);
		assert_eq!(voters, vec![(nominator.clone(), 4 * UNIT, vec![nominator])]);
	}

	/// `voters_include` is applied after the candidate self-votes, so it stays the final word.
	#[test]
	fn voters_include_wins_over_candidate_self_stake() {
		let injected = account(9);
		let overrides = ElectionOverrides {
			candidates_include: vec![CandidateInclude::WithSelfStake(
				injected.to_ss58check(),
				7 * UNIT,
			)],
			voters_include: vec![(
				injected.to_ss58check(),
				2 * UNIT,
				vec![injected.to_ss58check()],
			)],
			..no_overrides()
		};

		let (_, voters) = apply_overrides(vec![], vec![], overrides).unwrap();

		assert_eq!(voters, vec![(injected.clone(), 2 * UNIT, vec![injected])]);
	}

	/// A voter added by an override must not push the snapshot past its page capacity: the miner
	/// bounds the voter pages to `T::Pages`, so the extra page is dropped with every voter in it —
	/// and after the strongest-first pagination is reversed, that page is the strongest one.
	#[test]
	fn overrides_beyond_snapshot_capacity_drop_the_weakest_voters() {
		let capacity = (TEST_PAGES * TEST_VOTERS_PER_PAGE) as usize;

		// GIVEN a voter set exactly at capacity, strongest first as staking data arrives
		let voters: Vec<NominatorData> = (0..capacity)
			.map(|i| {
				let who = account((i + 1) as u8);
				(who.clone(), (capacity - i) as u64 * UNIT, vec![who])
			})
			.collect();
		let strongest = voters[0].0.clone();
		let weakest = voters[capacity - 1].0.clone();
		let injected = account(200);

		// WHEN an override injects one more voter, as a candidate self-stake does
		let overrides = ElectionOverrides {
			candidates_include: vec![CandidateInclude::WithSelfStake(
				injected.to_ss58check(),
				50 * UNIT,
			)],
			..no_overrides()
		};
		let (candidates, voters) = apply_overrides(vec![], voters, overrides).unwrap();
		let (_, voter_snapshot) = convert_election_data_to_snapshots::<TestMinerConfig>(
			candidates,
			voters,
			ElectionDataSource::Staking,
		)
		.unwrap();

		// THEN the snapshot still fits in `Pages` pages, and the weakest voter is what made room
		assert_eq!(voter_snapshot.len(), TEST_PAGES as usize);
		let voters: Vec<AccountId> = voter_snapshot
			.iter()
			.flat_map(|page| page.iter().map(|(who, _, _)| who.clone()))
			.collect();
		assert_eq!(voters.len(), capacity);
		assert!(voters.contains(&injected), "injected voter dropped");
		assert!(voters.contains(&strongest), "strongest voter dropped");
		assert!(!voters.contains(&weakest), "weakest voter kept");
	}

	/// An injected self-stake competes for a snapshot slot like any other voter rather than being
	/// forced in: on chain, whether a validator's self-stake reaches the election depends on where
	/// it lands in the bags list, so a self-stake too small to make the cut must not displace a
	/// stronger voter.
	#[test]
	fn an_injected_self_stake_too_small_to_make_the_cut_is_dropped() {
		let capacity = (TEST_PAGES * TEST_VOTERS_PER_PAGE) as usize;

		// GIVEN a voter set exactly at capacity, the weakest of them holding one UNIT
		let voters: Vec<NominatorData> = (0..capacity)
			.map(|i| {
				let who = account((i + 1) as u8);
				(who.clone(), (capacity - i) as u64 * UNIT, vec![who])
			})
			.collect();
		let weakest = voters[capacity - 1].0.clone();
		let injected = account(200);

		// WHEN a candidate is injected with a self-stake below all of them
		let overrides = ElectionOverrides {
			candidates_include: vec![CandidateInclude::WithSelfStake(
				injected.to_ss58check(),
				UNIT / 2,
			)],
			..no_overrides()
		};
		let (candidates, voters) = apply_overrides(vec![], voters, overrides).unwrap();
		let (_, voter_snapshot) = convert_election_data_to_snapshots::<TestMinerConfig>(
			candidates,
			voters,
			ElectionDataSource::Staking,
		)
		.unwrap();

		// THEN it is the injected voter that makes room, and the existing set is untouched
		let voters: Vec<AccountId> = voter_snapshot
			.iter()
			.flat_map(|page| page.iter().map(|(who, _, _)| who.clone()))
			.collect();
		assert_eq!(voters.len(), capacity);
		assert!(!voters.contains(&injected), "injected voter forced in");
		assert!(voters.contains(&weakest), "existing voter displaced by a weaker self-stake");
	}

	/// Run the offline part of the predict pipeline: overrides -> snapshots -> mine -> prediction.
	async fn predict(
		candidates: Vec<ValidatorData>,
		voters: Vec<NominatorData>,
		overrides: ElectionOverrides,
		desired_targets: u32,
	) -> ValidatorsPrediction {
		let (candidates, voters) = apply_overrides(candidates, voters, overrides).unwrap();
		let (target_snapshot, voter_snapshot) =
			convert_election_data_to_snapshots::<TestMinerConfig>(
				candidates,
				voters,
				ElectionDataSource::Staking,
			)
			.unwrap();

		let solution = mine_solution::<TestMinerConfig>(
			target_snapshot.clone(),
			voter_snapshot.clone(),
			1,
			0,
			desired_targets,
			0,
			false,
		)
		.await
		.unwrap();

		let ctx = PredictionContext {
			round: 0,
			desired_targets,
			block_number: 0,
			ss58_prefix: 0,
			token_decimals: 0,
			token_symbol: "PLANCK",
			data_source: ElectionDataSource::Staking,
		};

		build_predictions_from_solution::<TestMinerConfig>(
			&solution,
			&target_snapshot,
			&voter_snapshot,
			&ctx,
		)
		.unwrap()
		.0
	}

	/// An injected candidate is only electable through its overridden self-stake, and the election
	/// credits it with exactly that stake.
	#[tokio::test]
	async fn injected_candidate_is_elected_by_its_self_stake() {
		// GIVEN three self-voting candidates competing for three seats, and an off-chain account
		let (weakest, injected) = (account(3), account(9));
		let candidates: Vec<ValidatorData> =
			vec![(account(1), 0), (account(2), 0), (weakest.clone(), 0)];
		let voters: Vec<NominatorData> = vec![
			(account(1), 3 * UNIT, vec![account(1)]),
			(account(2), 2 * UNIT, vec![account(2)]),
			(weakest.clone(), UNIT, vec![weakest.clone()]),
		];
		let desired_targets = 3;
		let injected_ss58 = encode_account_id(&injected, 0);
		let weakest_ss58 = encode_account_id(&weakest, 0);

		// WHEN it is injected as a candidate with a self-stake beating the weakest validator
		let with_self_stake = ElectionOverrides {
			candidates_include: vec![CandidateInclude::WithSelfStake(
				injected.to_ss58check(),
				5 * UNIT,
			)],
			..no_overrides()
		};
		let elected =
			predict(candidates.clone(), voters.clone(), with_self_stake, desired_targets).await;

		// THEN it takes a seat with exactly the overridden self-stake, evicting the weakest one
		let winners: Vec<&str> = elected.results.iter().map(|v| v.account.as_str()).collect();
		assert_eq!(winners.len(), desired_targets as usize);
		assert!(winners.contains(&injected_ss58.as_str()), "injected candidate not elected");
		assert!(!winners.contains(&weakest_ss58.as_str()), "weakest validator not evicted");

		let injected_info =
			elected.results.iter().find(|v| v.account == injected_ss58).expect("elected");
		assert_eq!(injected_info.self_stake, "5000000000000 PLANCK");
		assert_eq!(injected_info.total_stake, "5000000000000 PLANCK");
		assert_eq!(injected_info.nominator_count, 0);

		// WHEN the same account is injected without a self-stake (legacy bare address)
		let without_self_stake = ElectionOverrides {
			candidates_include: vec![CandidateInclude::Address(injected.to_ss58check())],
			..no_overrides()
		};
		let not_elected = predict(candidates, voters, without_self_stake, desired_targets).await;

		// THEN it has no backing and cannot win: the original three keep their seats
		let winners: Vec<&str> = not_elected.results.iter().map(|v| v.account.as_str()).collect();
		assert!(!winners.contains(&injected_ss58.as_str()), "candidate elected without stake");
		assert!(winners.contains(&weakest_ss58.as_str()), "weakest validator lost its seat");
	}
}
