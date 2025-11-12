//! Shared utilities for fetching staking data

use crate::commands::multi_block::types::{TargetSnapshotPageOf, VoterSnapshotPageOf};
use crate::commands::types::{
	NominatorPrediction, NominatorsPrediction, PredictionMetadata, ValidatorInfo,
	ValidatorStakeAllocation, ValidatorsPrediction,
};
use crate::{
	client::Client,
	dynamic::{pallet_api, utils::storage_addr},
	error::Error,
	prelude::{AccountId, STAKING_LOG_TARGET as LOG_TARGET},
};
use codec::{Decode, Encode};
use polkadot_sdk::pallet_election_provider_multi_block::unsigned::miner:: MinerConfig;
use polkadot_sdk::sp_npos_elections::{ElectionResult, seq_phragmen};
use polkadot_sdk::sp_runtime::{BoundedVec, Perbill};
use scale_value::At;
use std::time::Duration;
use subxt::dynamic::Value;

/// Fetch the desired validator count from the Staking pallet
pub(crate) async fn fetch_validator_count(client: &Client) -> Result<u32, Error> {
	let storage = client.chain_api().storage().at_latest().await?;
	let validator_count_query = storage_addr(pallet_api::staking::storage::VALIDATOR_COUNT, vec![]);
	log::trace!(target: LOG_TARGET, "Fetching Staking::ValidatorCount");

	let validator_count = storage.fetch(&validator_count_query).await?;

	match validator_count {
		Some(count_data) => {
			let count: u32 = Decode::decode(&mut &count_data.encoded()[..])
				.map_err(|e| Error::Other(format!("Failed to decode validator count: {}", e)))?;
			log::trace!(target: LOG_TARGET, "ValidatorCount decoded: {count}");
			Ok(count)
		},
		None => {
			log::warn!(target: LOG_TARGET, "Staking::ValidatorCount not found; defaulting to 16");
			Ok(16)
		},
	}
}

/// Fetch all candidate validators (stash AccountId) with their active stake
pub(crate) async fn fetch_candidates(client: &Client) -> Result<Vec<(AccountId, u128)>, Error> {
	let storage = client.chain_api().storage().at_latest().await?;
	log::info!(target: LOG_TARGET, "Fetching candidate validators (Staking::Validators keys)");

	let validators_addr = storage_addr(pallet_api::staking::storage::VALIDATORS, vec![]);
	let mut iter = storage.iter(validators_addr).await?;

	let mut candidate_accounts: Vec<AccountId> = Vec::new();
	let mut count: usize = 0;

	while let Some(next) = iter.next().await {
		let kv = match next {
			Ok(kv) => kv,
			Err(e) => return Err(Error::Other(format!("storage iteration error: {e}"))),
		};

		let key_bytes = kv.key_bytes;
		if key_bytes.len() < 32 {
			return Err(Error::Other(format!(
				"unexpected key length {} (< 32); cannot decode AccountId",
				key_bytes.len()
			)));
		}
		let tail = &key_bytes[key_bytes.len() - 32..];
		let arr: [u8; 32] = tail
			.try_into()
			.map_err(|_| Error::Other("failed to slice AccountId32 bytes".into()))?;
		let account = AccountId::from(arr);

		candidate_accounts.push(account);
		count += 1;

		if count % 100 == 0 {
			log::info!(target: LOG_TARGET, "Collected {count} candidate accounts...");
		}
	}

	log::info!(
		target: LOG_TARGET,
		"Total candidate accounts collected: {}",
		candidate_accounts.len()
	);

	let stakes = fetch_stakes_in_batches(client, &candidate_accounts).await?;

	let candidates: Vec<(AccountId, u128)> =
		candidate_accounts.into_iter().zip(stakes.into_iter()).collect();

	log::info!(target: LOG_TARGET, "Total registered candidates: {}", candidates.len());

	Ok(candidates)
}

/// Fetch stakes in batches for better performance with retry logic for RPC rate limits
pub(crate) async fn fetch_stakes_in_batches(
	client: &Client,
	accounts: &[AccountId],
) -> Result<Vec<u128>, Error> {
	const BATCH_SIZE: usize = 250;
	const MAX_CONCURRENT_BATCHES: usize = 10;
	const MAX_RETRIES: u32 = 5;

	let mut stakes = Vec::with_capacity(accounts.len());
	let mut batch_handles: Vec<tokio::task::JoinHandle<Result<Vec<u128>, Error>>> = Vec::new();

	let mut processed_accounts: usize = 0;

	for chunk in accounts.chunks(BATCH_SIZE) {
		let chunk = chunk.to_vec();
		let client_clone = client.clone();

		let handle = tokio::spawn(async move {
			// Retry logic for individual batch
			let mut last_error: Option<Error> = None;
			for attempt in 0..=MAX_RETRIES {
				match fetch_stakes_batch_static(&client_clone, &chunk).await {
					Ok(batch_stakes) => return Ok(batch_stakes),
					Err(e) => {
						let error_msg = format!("{}", e);
						if (error_msg.contains("limit reached") || error_msg.contains("RPC error"))
							&& attempt < MAX_RETRIES
						{
							last_error = Some(e);
							let delay = Duration::from_secs(1) * (1 << attempt); // Exponential backoff
							tokio::time::sleep(delay.min(Duration::from_secs(30))).await;
							continue;
						} else {
							return Err(e);
						}
					},
				}
			}
			Err(last_error.unwrap_or_else(|| Error::Other("Failed after retries".into())))
		});

		batch_handles.push(handle);

		// If we have reached max concurrent batches, wait for the oldest to finish
		if batch_handles.len() >= MAX_CONCURRENT_BATCHES {
			let handle = batch_handles.remove(0);
			match handle.await {
				Ok(Ok(batch_stakes)) => {
					processed_accounts += batch_stakes.len();
					stakes.extend(batch_stakes);
					log::info!(
						target: LOG_TARGET,
						"Total accounts processed: {}",
						processed_accounts
					);
				},
				Ok(Err(e)) => return Err(e),
				Err(e) => return Err(Error::Other(format!("Task join error: {e}"))),
			}
		}
	}

	// Await any remaining batch tasks
	if !batch_handles.is_empty() {
		let batch_results = futures::future::join_all(batch_handles).await;
		for result in batch_results {
			match result {
				Ok(Ok(batch_stakes)) => {
					processed_accounts += batch_stakes.len();
					stakes.extend(batch_stakes);
					log::info!(
						target: LOG_TARGET,
						"Total accounts processed: {}",
						processed_accounts
					);
				},
				Ok(Err(e)) => return Err(e),
				Err(e) => return Err(Error::Other(format!("Task join error: {e}"))),
			}
		}
	}

	log::info!(
		target: LOG_TARGET,
		"Completed fetching stakes for {} accounts",
		processed_accounts
	);

	Ok(stakes)
}

/// Static version of stake fetching for use in spawned tasks
pub(crate) async fn fetch_stakes_batch_static(
	client: &Client,
	accounts: &[AccountId],
) -> Result<Vec<u128>, Error> {
	let at = client.chain_api().storage().at_latest().await?;

	let mut futs = Vec::with_capacity(accounts.len());
	for account in accounts {
		let at = at.clone();
		let account = account.clone();

		futs.push(async move {
			let bytes_vec: Vec<u8> = account.encode();
			let params: Vec<Value> = vec![Value::from_bytes(bytes_vec)];
			let ledger_addr = storage_addr(pallet_api::staking::storage::LEDGER, params);

			let mut stake = 0u128;
			if let Some(ledger) = at.fetch(&ledger_addr).await? {
				if let Ok(value) = ledger.to_value() {
					// Try to get 'active' first (self-stake), fallback to 'total' if 'active' not available
					// In Polkadot, 'active' is the active stake which includes self-stake
					// 'total' is the total stake (self + nominators)
					// For election purposes, we use 'active' as it represents the validator's own stake
					if let Some(active) = value.at("active").and_then(|v| v.as_u128()) {
						stake = active;
					} else if let Some(total) = value.at("total").and_then(|v| v.as_u128()) {
						// Fallback to total if active is not available
						stake = total;
					}
				}
			}

			Result::<u128, Error>::Ok(stake)
		});
	}

	let results: Vec<Result<u128, Error>> = futures::future::join_all(futs).await;
	let mut stakes = Vec::with_capacity(results.len());

	for result in results {
		stakes.push(result?);
	}

	Ok(stakes)
}

/// Fetch current round from MultiBlock pallet
pub(crate) async fn fetch_current_round(client: &Client) -> Result<u32, Error> {
	let storage = client.chain_api().storage().at_latest().await?;
	let current_round_query = storage_addr(pallet_api::multi_block::storage::ROUND, vec![]);
	match storage.fetch(&current_round_query).await? {
		Some(round_data) => {
			let round: u32 = Decode::decode(&mut &round_data.encoded()[..])?;
			Ok(round)
		},
		None => {
			log::warn!(target: LOG_TARGET, "No current round found in MultiBlockElection");
			Ok(0)
		},
	}
}

/// Fetch all nominators (stash, stake, targets) from Staking::Nominators
pub(crate) async fn fetch_nominators(
	client: &Client,
) -> Result<Vec<(AccountId, u64, Vec<AccountId>)>, Error> {

	/// Nominations data structure from the blockchain
	#[derive(Debug, Clone, Decode)]
	pub struct Nominations {
		/// List of validator accounts being nominated
		pub targets: Vec<AccountId>,
		/// Era when nominations were submitted
		pub _submitted_in: u32,
		/// Whether the nominator is suppressed
		pub _suppressed: bool,
	}

	// Build an iterator over Staking::Nominators at latest block
	let storage = client.chain_api().storage().at_latest().await?;

	let query = storage_addr(pallet_api::staking::storage::NOMINATORS, vec![]);
	let mut iter = storage.iter(query).await?;

	let mut count: usize = 0;

	let mut nominators: Vec<(AccountId, Vec<AccountId>)> = Vec::new();

	while let Some(Ok(kv)) = iter.next().await {
		// Recover stash AccountId32 from the storage key bytes:
		// For Twox64Concat maps, the original key is at the end (last 32 bytes for AccountId32).
		let key_bytes = kv.key_bytes.as_slice();
		let start = key_bytes.len().saturating_sub(32);
		let stash_arr: [u8; 32] = key_bytes[start..].try_into().expect("32 bytes");
		let stash = AccountId::from(stash_arr);

		// Decode the nominations into a typed struct.
		let nominations: Nominations = Decode::decode(&mut &kv.value.encoded()[..])?;

		nominators.push((stash.clone(), nominations.targets));

		count += 1;
		if count % 1000 == 0 {
			log::info!(target: LOG_TARGET, "Processed {count} nominators...");
		}
	}
	log::info!(target: LOG_TARGET, "Processed {count} nominators...");
	log::info!(target: LOG_TARGET, "Fetching stakes of nominators");

	let nominator_stashes: Vec<AccountId> = nominators.iter().map(|e| e.0.clone()).collect();
	let nominator_stakes_u128 = fetch_stakes_in_batches(client, &nominator_stashes).await?;

	let mut nominators_with_stakes: Vec<(AccountId, u64, Vec<AccountId>)> =
		Vec::with_capacity(nominators.len());
	for ((stash, targets), stake_u128) in nominators.into_iter().zip(nominator_stakes_u128) {
		let stake_u64 = u64::try_from(stake_u128).unwrap_or(u64::MAX);
		nominators_with_stakes.push((stash.clone(), stake_u64, targets.clone()));
	}

	Ok(nominators_with_stakes)
}

/// Run election using seq_phragmen and extract results
pub(crate) fn predict_election(
	desired_targets: u32,
	candidates: &[(AccountId, u128)], // (validator, self_stake)
	nominators: &[(AccountId, u64, Vec<AccountId>)], // (nominator, stake, votes),
	self_vote: bool,
) -> Result<ElectionResult<AccountId, Perbill>, Error> {
	use crate::prelude::Accuracy;
	// use std::fs::File;
	// use std::io::Write;
	// use std::path::Path;

	// Prepare data for seq_phragmen election
	// Convert nominators to format expected by seq_phragmen: Vec<(voter, vote_weight, Vec<AccountId>)>
	let mut voters: Vec<(AccountId, u64, Vec<AccountId>)> = nominators.to_vec();

	// Extract candidate stashes (validators)
	let candidate_stashes: Vec<AccountId> =
		candidates.iter().map(|(account, _)| account.clone()).collect();

	// in case of snapshot self_vote is already included in the nominators
	// in case of staking self_vote is not included in the nominators
	if self_vote {
		// Validators must vote for themselves with their self-stake
		for (account, self_stake) in candidates.iter() {
			// Convert self_stake from u128 to u64 (clamp to u64::MAX if too large)
			let self_stake_u64 = u64::try_from(*self_stake).unwrap_or(u64::MAX);
			// Validator votes for itself
			voters.push((account.clone(), self_stake_u64, vec![account.clone()]));
		}
	}

	// Run the actual Phragmén election algorithm using seq_phragmen
	let election_result = seq_phragmen::<AccountId, Accuracy>(
		desired_targets as usize,
		candidate_stashes.clone(),
		voters.clone(),
		None, // No balancing config
	)
	.map_err(|e| Error::Other(format!("Failed to run seq_phragmen election: {:?}", e)))?;

	return Ok(election_result);
}

/// Build both validators_prediction.json and nominators_prediction.json
/// from the election_result returned by `seq_phragmen`.
pub(crate) fn build_predictions_from_phragmen(
	election_result: ElectionResult<AccountId, Perbill>,
	desired_targets: u32,
	candidates: &[(AccountId, u128)], // (validator, self_stake)
	nominators: &[(AccountId, u64, Vec<AccountId>)], // (nominator, stake, nominated_targets)
	ss58_prefix: u16,
	token_decimals: u8,
	token_symbol: &str,
) -> (ValidatorsPrediction, NominatorsPrediction) {
	use crate::utils::{encode_account_id, planck_to_token, planck_to_token_u64};
	use std::collections::{HashMap, HashSet};
	use std::time::{SystemTime, UNIX_EPOCH};

	// Build lookup: candidate -> self stake
	let candidate_self_stake: HashMap<AccountId, u128> = candidates.iter().cloned().collect();

	// ========================================================================
	// STEP 1 — Determine active validators (winners)
	// ========================================================================
	let mut active_set: HashSet<AccountId> = HashSet::new();

	election_result.winners.iter().for_each(|(winner, _)| {
		active_set.insert(winner.clone());
	});

	// Sort by total stake backing (descending)
	let mut winners_sorted: Vec<(AccountId, u128)> = election_result.winners.clone();
	winners_sorted.sort_by(|a, b| b.1.cmp(&a.1));
	winners_sorted.truncate(desired_targets as usize);

	// Create validator prediction output
	let validator_infos: Vec<ValidatorInfo> = winners_sorted
		.iter()
		.map(|(v, backing)| ValidatorInfo {
			account: encode_account_id(v, ss58_prefix),
			total_stake: planck_to_token(*backing, token_decimals, token_symbol),
			self_stake: planck_to_token(
				*candidate_self_stake.get(v).unwrap_or(&0),
				token_decimals,
				token_symbol,
			),
		})
		.collect();

	let timestamp = SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.map(|d| d.as_secs().to_string())
		.unwrap_or_else(|_| "0".to_string());

	let metadata = PredictionMetadata {
		timestamp: format!("{}", timestamp),
		desired_validators: desired_targets,
		round: 0,
		block_number: 0,
		solution_score: None,
		data_source: "unknown".into(),
	};

	let validators_prediction =
		ValidatorsPrediction { metadata: metadata.clone(), results: validator_infos };

	// ========================================================================
	// STEP 2 — Build nominator perspective allocation: active / inactive / waiting
	// ========================================================================
	let mut nominator_predictions: Vec<NominatorPrediction> = Vec::new();

	for (nominator, stake, nominated_targets) in nominators.iter() {
		// Map from validator -> allocated stake (computed via Perbill distribution)
		let mut allocations: HashMap<AccountId, u128> = HashMap::new();

		// Look up assignment result for this nominator
		if let Some(assign) = election_result.assignments.iter().find(|a| &a.who == nominator) {
			for (validator, perbill_weight) in assign.distribution.iter() {
				let allocated =
					(*stake as u128 * perbill_weight.deconstruct() as u128) / 1_000_000_000u128; // Perbill denominator

				if allocated > 0 {
					allocations.insert(validator.clone(), allocated);
				}
			}
		}

		// --- categorize nominated targets ---
		let mut active_supported = vec![];
		let mut inactive = vec![];
		let mut waiting = vec![];

		for target in nominated_targets.iter() {
			let encoded = encode_account_id(target, ss58_prefix);

			match (
				active_set.contains(target),      // is validator elected?
				allocations.contains_key(target), // did nominator support it?
			) {
				(true, true) => active_supported.push(ValidatorStakeAllocation {
					validator: encoded,
					allocated_stake: planck_to_token_u64(
						*allocations.get(target).unwrap() as u64,
						token_decimals,
						token_symbol,
					),
				}),
				(true, false) => inactive.push(encoded),
				(false, _) => waiting.push(encoded),
			}
		}

		nominator_predictions.push(NominatorPrediction {
			address: encode_account_id(nominator, ss58_prefix),
			stake: planck_to_token_u64(*stake, token_decimals, token_symbol),
			active_validators: active_supported,
			inactive_validators: inactive,
			waiting_validators: waiting,
		});
	}

	let nominators_prediction =
		NominatorsPrediction { metadata, nominators: nominator_predictions };

	(validators_prediction, nominators_prediction)
}

/// Convert staking data (candidates and nominators) into MineInput format for election mining
pub(crate) fn convert_staking_to_mine_solution_input<T>(
	candidates: Vec<(AccountId, u128)>,
	nominators: Vec<(AccountId, u64, Vec<AccountId>)>
) -> Result<(TargetSnapshotPageOf<T>, Vec<VoterSnapshotPageOf<T>>), Error>
where
	T: MinerConfig<AccountId = AccountId>,
{
	// Extract only accounts from candidates
	let target_accounts: Vec<AccountId> =
		candidates.into_iter().map(|(account, _)| account).collect();

	// Enforce MAX TARGETS PER PAGE (TargetSnapshotPerBlock)
	use crate::static_types::multi_block::TargetSnapshotPerBlock;
	let per_target_page = TargetSnapshotPerBlock::get();

	let mut target_pages: Vec<TargetSnapshotPageOf<T>> = Vec::new();
	for chunk in target_accounts.chunks(per_target_page as usize) {
		target_pages.push(BoundedVec::truncate_from(chunk.to_vec()));
	}

	// Flatten into one BoundedVec for MineInput (only single-page snapshots supported manually)
	let all_targets: TargetSnapshotPageOf<T> = BoundedVec::truncate_from(
		// convert the first BoundedVec page into a plain Vec<AccountId>; if no pages exist use an empty vec
		target_pages.first().map(|b| b.clone().into_inner()).unwrap_or_default(),
	);

	// voters → Voter<T> conversion
	use crate::static_types::multi_block::VoterSnapshotPerBlock;
	let per_voter_page = VoterSnapshotPerBlock::get();

	let total_nominators = nominators.len();

	let mut voter_pages_vec: Vec<VoterSnapshotPageOf<T>> = Vec::new();
	for (stash, stake, votes) in nominators {
		let votes: BoundedVec<AccountId, <T as MinerConfig>::MaxVotesPerVoter> =
			BoundedVec::truncate_from(votes);

		let voter = (stash, stake, votes);

		if voter_pages_vec
			.last()
			.map_or(true, |last| last.len() >= per_voter_page as usize)
		{
			voter_pages_vec.push(BoundedVec::truncate_from(vec![voter]));
		} else {
			voter_pages_vec.last_mut().unwrap().try_push(voter).ok();
		}
	}

	// Calculate the actual number of pages from the voter pages before moving
	let n_pages = voter_pages_vec.len() as u32;

	let voter_pages: BoundedVec<VoterSnapshotPageOf<T>, <T as MinerConfig>::Pages> =
		BoundedVec::truncate_from(voter_pages_vec);

	log::info!(
		target: LOG_TARGET,
		"Converted staking data: {} targets, {} voters across {} pages",
		all_targets.len(),
		total_nominators,
		n_pages
	);

	Ok((all_targets, voter_pages.to_vec()))
}