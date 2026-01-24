//! Shared utilities for fetching staking data

use crate::{
	commands::types::{NominatorData, ValidatorData},
	dynamic::{pallet_api, utils::storage_addr},
	error::Error,
	prelude::{AccountId, STAKING_LOG_TARGET as LOG_TARGET, Storage},
};
use codec::{Decode, Encode};
use scale_value::At;
use std::{
	collections::{HashMap, HashSet},
	time::Duration,
};
use subxt::dynamic::Value;

/// Fetch all candidate validators (stash AccountId) with their active stake
pub(crate) async fn fetch_candidates(storage: &Storage) -> Result<Vec<ValidatorData>, Error> {
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

		if count % 500 == 0 {
			log::debug!(target: LOG_TARGET, "Fetched {count} candidate accounts...");
		}
	}

	log::info!(
		target: LOG_TARGET,
		"Total candidate accounts fetched: {}",
		candidate_accounts.len()
	);

	// Fetch self-stakes concurrently
	log::info!(target: LOG_TARGET, "Fetching stakes for {} validators...", candidate_accounts.len());

	let mut stake_futures = Vec::with_capacity(candidate_accounts.len());

	for account in &candidate_accounts {
		let storage = storage.clone();
		let account = account.clone();

		stake_futures.push(async move {
			let bytes_vec: Vec<u8> = account.encode();
			let params: Vec<Value> = vec![Value::from_bytes(bytes_vec)];
			let bonded_addr = storage_addr(pallet_api::staking::storage::BONDED, params.clone());

			// Deterministic Controller lookup: first check Bonded, then fallback to account (Stash)
			let ledger_key_addr = if let Some(bonded) = storage.fetch(&bonded_addr).await? {
				let controller_bytes = bonded.encoded();
				if controller_bytes.len() < 32 {
					return Err(Error::Other("Unexpected Bonded key length".into()));
				}
				let tail = &controller_bytes[controller_bytes.len() - 32..];
				let arr: [u8; 32] = tail
					.try_into()
					.map_err(|_| Error::Other("Failed to slice Controller ID".into()))?;
				let controller = AccountId::from(arr);
				storage_addr(
					pallet_api::staking::storage::LEDGER,
					vec![Value::from_bytes(controller.encode())],
				)
			} else {
				storage_addr(pallet_api::staking::storage::LEDGER, params)
			};

			let mut stake = 0u128;
			if let Some(ledger) = storage.fetch(&ledger_key_addr).await? &&
				let Ok(value) = ledger.to_value()
			{
				// Try to get 'active' first (self-stake), fallback to 'total' if 'active' not
				// available
				if let Some(active) = value.at("active").and_then(|v| v.as_u128()) {
					stake = active;
				} else if let Some(total) = value.at("total").and_then(|v| v.as_u128()) {
					stake = total;
				}
			}

			Result::<u128, Error>::Ok(stake)
		});
	}

	let stakes_results = futures::future::join_all(stake_futures).await;
	let mut stakes = Vec::with_capacity(stakes_results.len());

	for result in stakes_results {
		stakes.push(result?);
	}

	log::info!(target: LOG_TARGET, "Fetched stakes for {} validators", stakes.len());

	let candidates: Vec<ValidatorData> =
		candidate_accounts.into_iter().zip(stakes.into_iter()).collect();

	log::info!(target: LOG_TARGET, "Total registered candidates: {}", candidates.len());

	Ok(candidates)
}

/// Helper to fetch just the validator keys (Set) for O(1) existence checks
pub(crate) async fn fetch_validator_keys(storage: &Storage) -> Result<HashSet<AccountId>, Error> {
	log::info!(target: LOG_TARGET, "Fetching validator keys for existence checks");
	let validators_addr = storage_addr(pallet_api::staking::storage::VALIDATORS, vec![]);
	let mut iter = storage.iter(validators_addr).await?;
	let mut keys = HashSet::new();

	while let Some(next) = iter.next().await {
		let kv = next.map_err(|e| Error::Other(format!("storage iteration error: {e}")))?;
		let key_bytes = kv.key_bytes;
		if key_bytes.len() >= 32 {
			let tail = &key_bytes[key_bytes.len() - 32..];
			if let Ok(arr) = <[u8; 32]>::try_from(tail) {
				keys.insert(AccountId::from(arr));
			}
		}
	}
	log::info!(target: LOG_TARGET, "Fetched {} validator keys", keys.len());
	Ok(keys)
}

/// Node data structure for VoterList (decoded from storage)  
/// The actual on-chain structure from pallet-bags-list
/// Structure based on substrate bags-list pallet Node definition
#[derive(Debug, Clone, Decode)]
struct ListNode {
	id: AccountId,
	#[allow(dead_code)]
	prev: Option<AccountId>,
	next: Option<AccountId>,
	bag_upper: u64,
	score: u64,
}

/// Bag data structure for VoterList
#[derive(Debug, Clone, Decode)]
struct ListBag {
	head: Option<AccountId>,
	#[allow(dead_code)]
	tail: Option<AccountId>,
}

/// Node data structure for VoterList processing
#[derive(Debug, Clone)]
struct VoterNode {
	id: AccountId,
	score: u64,
	next: Option<AccountId>,
	#[allow(dead_code)]
	bag_upper: u64,
}

/// Fetch and sort voters from the VoterList (BagsList)
/// Returns the top voters limited by voter_limit, sorted by score (stake) in descending order
pub(crate) async fn fetch_voter_list(
	voter_limit: usize,
	storage: &Storage,
) -> Result<Vec<(AccountId, u64)>, Error> {
	// Increase voter limit to have a buffer for filtering ineligible voters later
	let extended_voter_limit = voter_limit.saturating_add(100);

	log::info!(target: LOG_TARGET, "Fetching From Voter List");

	// Fetch all bags (ListBags) - store as HashMap with bag_upper as key
	log::info!(target: LOG_TARGET, "Fetching ListBags...");
	let list_bags_addr = storage_addr(pallet_api::voter_list::storage::LIST_BAGS, vec![]);
	let mut bags_iter = storage.iter(list_bags_addr).await?;

	let mut bags: HashMap<u64, ListBag> = HashMap::new();

	while let Some(next) = bags_iter.next().await {
		let kv = match next {
			Ok(kv) => kv,
			Err(e) => return Err(Error::Other(format!("bags iteration error: {e}"))),
		};

		// Extract bag score (bag_upper) from key (u64)
		let key_bytes = kv.key_bytes;
		let bag_upper = if key_bytes.len() >= 8 {
			let start = key_bytes.len().saturating_sub(8);
			u64::from_le_bytes(key_bytes[start..].try_into().unwrap_or([0u8; 8]))
		} else {
			0u64
		};

		// Decode the bag structure
		let bag: ListBag = Decode::decode(&mut &kv.value.encoded()[..])?;

		// Store bag with its upper bound as key
		bags.insert(bag_upper, bag);
	}

	log::info!(target: LOG_TARGET, "Found {} bags", bags.len());

	// Fetch all nodes (ListNodes) - store as HashMap with AccountId as key
	log::info!(target: LOG_TARGET, "Fetching ListNodes...");

	let mut nodes: HashMap<AccountId, VoterNode> = HashMap::new();
	let list_nodes_addr = storage_addr(pallet_api::voter_list::storage::LIST_NODES, vec![]);
	let mut nodes_iter = storage.iter(list_nodes_addr).await?;

	let mut nodes_count = 0;

	while let Some(next) = nodes_iter.next().await {
		let kv = match next {
			Ok(kv) => kv,
			Err(e) => return Err(Error::Other(format!("node iteration error: {e}"))),
		};

		// Extract AccountId from key
		let key_bytes = &kv.key_bytes;
		if key_bytes.len() < 32 {
			continue;
		}
		let tail = &key_bytes[key_bytes.len() - 32..];
		let arr: [u8; 32] = tail
			.try_into()
			.map_err(|_| Error::Other("failed to slice AccountId32 bytes".into()))?;
		let account_id = AccountId::from(arr);

		// Decode node data using the struct
		let list_node = match ListNode::decode(&mut &kv.value.encoded()[..]) {
			Ok(node) => node,
			Err(e) => {
				log::warn!(
					target: LOG_TARGET,
					"Failed to decode ListNode for {account_id:?}: {e}"
				);
				continue;
			},
		};

		// Store node in HashMap for O(1) lookup
		nodes.insert(
			list_node.id.clone(),
			VoterNode {
				id: list_node.id,
				score: list_node.score,
				next: list_node.next,
				bag_upper: list_node.bag_upper,
			},
		);

		nodes_count += 1;

		if nodes_count % 5000 == 0 {
			log::info!(target: LOG_TARGET, "Fetched {nodes_count} nodes...");
		}
	}

	log::info!(target: LOG_TARGET, "Found {} nodes total", nodes.len());

	// Sort bags from highest to lowest score (descending order)
	log::info!(target: LOG_TARGET, "Sorting bags by score (descending)...");
	let mut sorted_bag_keys: Vec<u64> = bags.keys().copied().collect();
	sorted_bag_keys.sort_by(|a, b| b.cmp(a)); // Descending order - highest stake first

	log::info!(
		target: LOG_TARGET,
		"Bags sorted. Highest bag: {:?}, Lowest bag: {:?}",
		sorted_bag_keys.first(),
		sorted_bag_keys.last()
	);

	// Iterate through bags and follow linked lists to build voter snapshot
	log::info!(
		target: LOG_TARGET,
		"Building voter snapshot (limit: {voter_limit})..."
	);

	let mut voters: Vec<(AccountId, u64)> = Vec::new();
	let mut processed: HashMap<AccountId, bool> = HashMap::new();
	let mut total_nodes_processed = 0;

	// Iterate through each bag from highest to lowest
	for bag_upper in sorted_bag_keys {
		// Check if we've reached the extended voter limit
		if voters.len() >= extended_voter_limit {
			log::info!(target: LOG_TARGET, "Reached extended voter limit of {extended_voter_limit}");
			break;
		}

		// Get the bag
		let bag = match bags.get(&bag_upper) {
			Some(b) => b,
			None => continue,
		};

		// Skip empty bags (no head)
		if bag.head.is_none() {
			continue;
		}

		// Start from the head of this bag's linked list
		let mut current_node_id = bag.head.clone();
		let mut nodes_in_bag = 0;

		// Walk through the linked list for this bag
		while let Some(node_id) = current_node_id {
			// Check extended voter limit
			if voters.len() >= extended_voter_limit {
				break;
			}

			// Skip if already processed (detect cycles)
			if processed.contains_key(&node_id) {
				log::warn!(
					target: LOG_TARGET,
					"Cycle detected: node {node_id:?} already processed in bag {bag_upper}"
				);
				break;
			}

			// Mark as processed
			processed.insert(node_id.clone(), true);

			// Get the node from our HashMap
			let node = match nodes.get(&node_id) {
				Some(n) => n,
				None => {
					log::warn!(
						target: LOG_TARGET,
						"Broken chain: Node {node_id:?} not found in bag {bag_upper}"
					);
					break;
				},
			};

			// Add this voter to our snapshot
			voters.push((node.id.clone(), node.score));
			nodes_in_bag += 1;
			total_nodes_processed += 1;

			// Move to the next node in the linked list
			current_node_id = node.next.clone();
		}

		if nodes_in_bag > 0 {
			log::debug!(
				target: LOG_TARGET,
				"Bag {bag_upper} (upper: {bag_upper}): processed {nodes_in_bag} nodes"
			);
		}
	}

	log::info!(
		target: LOG_TARGET,
		"Voter List Fetch Completed"
	);
	log::info!(
		target: LOG_TARGET,
		"Total voters fetched for selection pool: {} (voter_limit: {}, extended_voter_limit: {})",
		voters.len(),
		voter_limit,
		extended_voter_limit
	);
	log::info!(
		target: LOG_TARGET,
		"Total nodes processed: {total_nodes_processed}"
	);

	Ok(voters)
}

/// Fetch complete voter data including nomination targets
/// This function first fetches voters from VoterList, then queries staking.nominators in batches
/// Uses concurrent batch processing
pub(crate) async fn fetch_voters(
	voter_limit: usize,
	storage: &Storage,
) -> Result<Vec<NominatorData>, Error> {
	// Fetch voters from VoterList (BagsList) with their stakes (already sorted)
	log::info!(target: LOG_TARGET, "Fetching voters from VoterList...");
	let voters = fetch_voter_list(voter_limit, storage).await?;

	log::info!(target: LOG_TARGET, "Fetched {} voters from VoterList", voters.len());

	// Fetch Validators (keys only) to perform `Validators::<T>::contains_key(&voter)` check
	// This is critical for implicit self-votes where no Nominator entry exists.
	let validator_keys = fetch_validator_keys(storage).await?;

	// Prepare for batch fetching of nomination targets
	const BATCH_SIZE: usize = 20;
	const MAX_CONCURRENT_BATCHES: usize = 10;
	const MAX_RETRIES: u32 = 8;

	let total = voters.len();
	log::info!(
		target: LOG_TARGET,
		"Fetching targets for {total} voters"
	);

	let mut complete_voter_data: Vec<NominatorData> = Vec::with_capacity(total);

	// Note: Using the return type of the batch task to Option<Vec<...>> to handle missing entries
	type BatchResult = Vec<(AccountId, u64, Option<Vec<AccountId>>)>;
	type BatchHandle = tokio::task::JoinHandle<Result<BatchResult, Error>>;
	let mut batch_handles: Vec<BatchHandle> = Vec::new();
	let mut processed_count: usize = 0;

	// Process in batches with concurrency
	for chunk in voters.chunks(BATCH_SIZE) {
		let chunk = chunk.to_vec();
		let storage_clone = storage.clone();

		let handle = tokio::spawn(async move {
			// Retry logic for individual batch
			let mut last_error: Option<Error> = None;
			for attempt in 0..=MAX_RETRIES {
				match fetch_voter_batch(&chunk, &storage_clone).await {
					Ok(results) => return Ok(results),
					Err(e) => {
						let error_msg = format!("{e}");
						if (error_msg.contains("limit reached") ||
							error_msg.contains("RPC error") ||
							error_msg.contains("timeout")) &&
							attempt < MAX_RETRIES
						{
							last_error = Some(e);
							let delay = Duration::from_secs(2) * (1 << attempt); // Exponential backoff starting at 2s
							tokio::time::sleep(delay.min(Duration::from_secs(60))).await;
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

		// Manage concurrency and process results
		if batch_handles.len() >= MAX_CONCURRENT_BATCHES {
			let handle = batch_handles.remove(0);
			match handle.await {
				Ok(Ok(results)) => {
					// Logic: get_npos_voters implementation
					for (account, stake, maybe_targets) in results {
						match maybe_targets {
							// Case 1: Nominator entry exists (include only if targets exist)
							Some(targets) =>
								if !targets.is_empty() {
									complete_voter_data.push((account, stake, targets));
								} else {
									log::debug!(target: LOG_TARGET, "Skipping nominator {account:?} - no targets");
								},
							// Case 2: No Nominator entry -> Check if Validator
							None => {
								if validator_keys.contains(&account) {
									// Implicit self-vote
									complete_voter_data.push((
										account.clone(),
										stake,
										vec![account],
									));
								}
								// Else: Defensive error / skip
							},
						}
					}
					processed_count += BATCH_SIZE.min(total - processed_count); // Approximate
					if processed_count % 5000 == 0 {
						log::info!(target: LOG_TARGET, "Processed {processed_count} voters...");
					}
				},
				Ok(Err(e)) => return Err(e),
				Err(e) => return Err(Error::Other(format!("Task join error: {e}"))),
			}
		}
	}

	// Await remaining tasks
	if !batch_handles.is_empty() {
		let batch_results = futures::future::join_all(batch_handles).await;
		for result in batch_results {
			match result {
				Ok(Ok(results)) =>
					for (account, stake, maybe_targets) in results {
						match maybe_targets {
							// Include only nominators with non-empty targets
							Some(targets) =>
								if !targets.is_empty() {
									complete_voter_data.push((account, stake, targets));
								},
							None =>
								if validator_keys.contains(&account) {
									complete_voter_data.push((
										account.clone(),
										stake,
										vec![account],
									));
								},
						}
					},
				Ok(Err(e)) => return Err(e),
				Err(e) => return Err(Error::Other(format!("Task join error: {e}"))),
			}
		}
	}

	// Truncate to exact voter_limit to match on-chain behavior
	if complete_voter_data.len() > voter_limit {
		log::info!(
			target: LOG_TARGET,
			"Truncating voters from {} to {}",
			complete_voter_data.len(),
			voter_limit
		);
		complete_voter_data.truncate(voter_limit);
	}

	log::info!(
		target: LOG_TARGET,
		"Completed fetching voter data with targets for {} voters",
		complete_voter_data.len()
	);

	Ok(complete_voter_data)
}

/// Nominations data structure from the blockchain
#[derive(Debug, Clone, Decode)]
struct Nominations {
	/// List of validator accounts being nominated
	pub targets: Vec<AccountId>,
	/// Era when nominations were submitted
	#[allow(dead_code)]
	pub submitted_in: u32,
	/// Whether the nominator is suppressed
	pub suppressed: bool,
}

/// Helper to fetch a single batch of nominators
async fn fetch_voter_batch(
	voters: &[(AccountId, u64)],
	storage: &Storage,
) -> Result<Vec<(AccountId, u64, Option<Vec<AccountId>>)>, Error> {
	let mut batch_results = Vec::with_capacity(voters.len());

	// Prepare futures for fetching individual nominator data
	let mut futs = Vec::with_capacity(voters.len());

	for (account_id, stake) in voters {
		let storage = &storage;
		let account_id = account_id.clone();
		let score_stake = *stake;

		futs.push(async move {
			let bytes_vec: Vec<u8> = account_id.encode();
			let params: Vec<Value> = vec![Value::from_bytes(bytes_vec)];
			let nominators_addr =
				storage_addr(pallet_api::staking::storage::NOMINATORS, params.clone());
			let bonded_addr = storage_addr(pallet_api::staking::storage::BONDED, params.clone());

			// Fetch targets
			let nominator_data = storage.fetch(&nominators_addr).await?;

			// Deterministic Controller lookup for stake accuracy
			let ledger_key_addr = if let Some(bonded) = storage.fetch(&bonded_addr).await? {
				let controller_bytes = bonded.encoded();
				if controller_bytes.len() < 32 {
					return Err(Error::Other("Unexpected Bonded key length".into()));
				}
				let tail = &controller_bytes[controller_bytes.len() - 32..];
				let arr: [u8; 32] = tail
					.try_into()
					.map_err(|_| Error::Other("Failed to slice Controller ID".into()))?;
				let controller = AccountId::from(arr);
				storage_addr(
					pallet_api::staking::storage::LEDGER,
					vec![Value::from_bytes(controller.encode())],
				)
			} else {
				storage_addr(pallet_api::staking::storage::LEDGER, params)
			};

			// Default to VoterList score, but override with actual Ledger active stake
			let mut actual_stake = score_stake;

			if let Some(ledger) = storage.fetch(&ledger_key_addr).await? &&
				let Ok(value) = ledger.to_value()
			{
				if let Some(active) = value.at("active").and_then(|v| v.as_u128()) {
					actual_stake = active as u64;
				}
			}

			Result::<_, Error>::Ok((account_id, actual_stake, nominator_data))
		});
	}

	let results = futures::future::join_all(futs).await;

	for result in results {
		let (account_id, stake, nominator_data) = result?;

		let targets = if let Some(data) = nominator_data {
			// Decode the nominations
			if let Ok(nominations) = Nominations::decode(&mut &data.encoded()[..]) {
				// Only include active nominators (not suppressed)
				if !nominations.suppressed { Some(nominations.targets) } else { Some(Vec::new()) }
			} else {
				// Failed to decode, treat as empty targets
				Some(Vec::new())
			}
		} else {
			// Nominator data NOT FOUND -> This is where we will check if it is a validator later
			None
		};
		batch_results.push((account_id, stake, targets));
	}

	Ok(batch_results)
}
