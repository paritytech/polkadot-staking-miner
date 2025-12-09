//! Shared utilities for fetching staking data

use crate::{
	client::Client,
	commands::types::{NominatorData, ValidatorData},
	dynamic::{pallet_api, utils::storage_addr},
	error::Error,
	prelude::{AccountId, STAKING_LOG_TARGET as LOG_TARGET},
};
use codec::{Decode, Encode};
use scale_value::At;
use std::{collections::HashMap, time::Duration};
use subxt::dynamic::Value;

/// Fetch all candidate validators (stash AccountId) with their active stake
pub(crate) async fn fetch_candidates(client: &Client) -> Result<Vec<ValidatorData>, Error> {
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

	// Fetch self-stakes concurrently
	log::info!(target: LOG_TARGET, "Fetching stakes for {} validators...", candidate_accounts.len());

	let mut stake_futures = Vec::with_capacity(candidate_accounts.len());

	for account in &candidate_accounts {
		let storage = storage.clone();
		let account = account.clone();

		stake_futures.push(async move {
			let bytes_vec: Vec<u8> = account.encode();
			let params: Vec<Value> = vec![Value::from_bytes(bytes_vec)];
			let ledger_addr = storage_addr(pallet_api::staking::storage::LEDGER, params);

			let mut stake = 0u128;
			if let Some(ledger) = storage.fetch(&ledger_addr).await? &&
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

/// Node data structure for VoterList (decoded from storage)  
/// The actual on-chain structure from pallet-bags-list
/// Note: Field order matters for SCALE codec!
/// Structure based on substrate bags-list pallet Node definition
#[derive(Debug, Clone, Decode)]
struct ListNode {
	_id: AccountId,
	_prev: Option<AccountId>,
	next: Option<AccountId>,
	bag_upper: u64,
	score: u64,
}

/// Bag data structure for VoterList (decoded from storage)
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
	_bag_upper: u64,
}

/// Fetch and sort voters from the VoterList (BagsList)
/// Returns the top voters limited by voter_limit, sorted by score (stake) in descending order
/// This implementation replicates the chain's linked list structure by walking through bags
pub(crate) async fn fetch_voter_list(
	client: &Client,
	voter_limit: usize,
) -> Result<Vec<(AccountId, u64)>, Error> {
	let storage = client.chain_api().storage().at_latest().await?;

	// Fetch all bags (ListBags)
	log::info!(target: LOG_TARGET, "Fetching VoterList bags...");
	let list_bags_addr = storage_addr(pallet_api::voter_list::storage::LIST_BAGS, vec![]);
	let mut bags_iter = storage.iter(list_bags_addr).await?;

	let mut bags: HashMap<u64, AccountId> = HashMap::new();

	while let Some(next) = bags_iter.next().await {
		let kv = match next {
			Ok(kv) => kv,
			Err(e) => return Err(Error::Other(format!("bags iteration error: {e}"))),
		};

		// Extract bag score from key (u64)
		let key_bytes = kv.key_bytes;
		let bag_score = if key_bytes.len() >= 8 {
			let start = key_bytes.len().saturating_sub(8);
			u64::from_le_bytes(key_bytes[start..].try_into().unwrap_or([0u8; 8]))
		} else {
			0u64
		};

		// Decode the bag structure
		let bag: ListBag = Decode::decode(&mut &kv.value.encoded()[..])?;

		// Store bag head if it exists
		if let Some(head_account) = bag.head {
			bags.insert(bag_score, head_account);
		}
	}

	log::info!(target: LOG_TARGET, "Found {} bags", bags.len());

	// Get total node count
	let counter_addr =
		storage_addr(pallet_api::voter_list::storage::COUNTER_FOR_LIST_NODES, vec![]);
	let total_nodes_count = if let Some(counter) = storage.fetch(&counter_addr).await? {
		let count: u32 = Decode::decode(&mut &counter.encoded()[..])?;
		count as usize
	} else {
		0
	};

	log::info!(target: LOG_TARGET, "Total nodes in VoterList: {total_nodes_count}");

	// Fetch nodes in batches (paginated)
	log::info!(target: LOG_TARGET, "Fetching Voters (Nodes) in batches...");

	let mut nodes: HashMap<AccountId, VoterNode> = HashMap::new();
	let list_nodes_addr = storage_addr(pallet_api::voter_list::storage::LIST_NODES, vec![]);
	let mut iter = storage.iter(list_nodes_addr).await?;

	let mut count = 0;

	while let Some(next) = iter.next().await {
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

		// Dedup: skip if already exists
		if nodes.contains_key(&account_id) {
			continue;
		}

		// Try to decode node data using the struct
		let list_node = match ListNode::decode(&mut &kv.value.encoded()[..]) {
			Ok(node) => node,
			Err(e) => {
				// If struct decoding fails, try dynamic decoding as fallback
				log::warn!(
					target: LOG_TARGET,
					"Failed to decode ListNode for {account_id:?}: {e}. Trying dynamic decoding..."
				);

				if let Ok(value) = kv.value.to_value() {
					let score = value.at("score").and_then(|v| v.as_u128()).unwrap_or(0) as u64;
					let bag_upper =
						value.at("bagUpper").and_then(|v| v.as_u128()).unwrap_or(0) as u64;

					nodes.insert(
						account_id.clone(),
						VoterNode { id: account_id, score, next: None, _bag_upper: bag_upper },
					);
				}
				continue;
			},
		};

		nodes.insert(
			account_id.clone(),
			VoterNode {
				id: account_id, // Use the key's account_id to be safe/consistent
				score: list_node.score,
				next: list_node.next,
				_bag_upper: list_node.bag_upper,
			},
		);

		count += 1;

		// Progress logging every 1000 nodes
		if count % 1000 == 0 {
			log::info!(
				target: LOG_TARGET,
				"Fetching Voters... [{} / {}]",
				nodes.len(),
				total_nodes_count
			);
		}
	}

	log::info!(target: LOG_TARGET, "All voters fetched");

	// Stitch and walk the linked list
	log::info!(target: LOG_TARGET, "Stitching the Linked List...");

	// Sort bags from highest to lowest
	let mut sorted_bag_keys: Vec<u64> = bags.keys().copied().collect();
	sorted_bag_keys.sort_by(|a, b| b.cmp(a)); // Descending order

	let mut final_voters: Vec<(AccountId, u64)> = Vec::new();

	for bag_id in sorted_bag_keys {
		if final_voters.len() >= voter_limit {
			break;
		}

		let mut current_head_id = bags.get(&bag_id).cloned();

		// Walk this bag following the linked list
		while let Some(ref head_id) = current_head_id {
			if final_voters.len() >= voter_limit {
				break;
			}

			let node_data = nodes.get(head_id);

			if let Some(node) = node_data {
				final_voters.push((node.id.clone(), node.score));
				current_head_id = node.next.clone(); // Follow the pointer
			} else {
				log::warn!(
					target: LOG_TARGET,
					"Broken Chain: Node {head_id:?} not found in map"
				);
				break;
			}
		}
	}

	log::info!(
		target: LOG_TARGET,
		"Using {} voters for election (limited by voter_limit: {})",
		final_voters.len(),
		voter_limit
	);

	Ok(final_voters)
}

/// Fetch complete voter data including nomination targets
/// This function first fetches voters from VoterList, then queries staking.nominators in batches
/// Uses concurrent batch processing
pub(crate) async fn fetch_nominators(
	client: &Client,
	voter_limit: usize,
) -> Result<Vec<NominatorData>, Error> {
	// Fetch voters from VoterList (BagsList) with their stakes
	log::info!(target: LOG_TARGET, "Fetching voters from VoterList...");
	let voters = fetch_voter_list(client, voter_limit).await?;

	log::info!(target: LOG_TARGET, "Fetched {} voters from VoterList", voters.len());

	// Prepare for batch fetching of nomination targets
	const BATCH_SIZE: usize = 20;
	const MAX_CONCURRENT_BATCHES: usize = 10;
	const MAX_RETRIES: u32 = 8;

	let total = voters.len();
	log::info!(
		target: LOG_TARGET,
		"Starting concurrent batch fetch of targets for {total} voters..."
	);

	let mut complete_voter_data: Vec<NominatorData> = Vec::with_capacity(total);
	let mut batch_handles: Vec<tokio::task::JoinHandle<Result<Vec<NominatorData>, Error>>> =
		Vec::new();
	let mut processed_count: usize = 0;

	// Process in batches with concurrency
	for chunk in voters.chunks(BATCH_SIZE) {
		let chunk = chunk.to_vec();
		let client_clone = client.clone();

		let handle = tokio::spawn(async move {
			// Retry logic for individual batch
			let mut last_error: Option<Error> = None;
			for attempt in 0..=MAX_RETRIES {
				match fetch_nominators_batch(&client_clone, &chunk).await {
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

		// Manage concurrency
		if batch_handles.len() >= MAX_CONCURRENT_BATCHES {
			let handle = batch_handles.remove(0);
			match handle.await {
				Ok(Ok(results)) => {
					processed_count += results.len();
					complete_voter_data.extend(results);
					if processed_count % 2500 == 0 {
						log::info!(target: LOG_TARGET, "Processed {processed_count}/{total} voters...");
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
				Ok(Ok(results)) => {
					processed_count += results.len();
					complete_voter_data.extend(results);
				},
				Ok(Err(e)) => return Err(e),
				Err(e) => return Err(Error::Other(format!("Task join error: {e}"))),
			}
		}
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
async fn fetch_nominators_batch(
	client: &Client,
	voters: &[(AccountId, u64)],
) -> Result<Vec<NominatorData>, Error> {
	let storage = client.chain_api().storage().at_latest().await?;
	let mut batch_results = Vec::with_capacity(voters.len());

	// Prepare futures for fetching individual nominator data
	let mut futs = Vec::with_capacity(voters.len());

	for (account_id, stake) in voters {
		let storage = &storage;
		let account_id = account_id.clone();
		let stake = *stake;

		futs.push(async move {
			let bytes_vec: Vec<u8> = account_id.encode();
			let params: Vec<Value> = vec![Value::from_bytes(bytes_vec)];
			let nominators_addr = storage_addr(pallet_api::staking::storage::NOMINATORS, params);

			// Retry logic for individual request
			let mut last_error = None;
			for attempt in 0..=3 {
				match storage.fetch(&nominators_addr).await {
					Ok(data) => return Result::<_, Error>::Ok((account_id, stake, data)),
					Err(e) => {
						let error_msg = format!("{e}");
						if (error_msg.contains("limit reached") || error_msg.contains("RPC error")) &&
							attempt < 3
						{
							last_error = Some(Error::Subxt(Box::new(e)));
							let delay = Duration::from_secs(1) * (1 << attempt);
							tokio::time::sleep(delay).await;
							continue;
						}
						return Err(Error::Subxt(Box::new(e)));
					},
				}
			}
			Err(last_error.unwrap_or_else(|| Error::Other("Failed after retries".into())))
		});
	}

	let results = futures::future::join_all(futs).await;

	for result in results {
		let (account_id, stake, nominator_data) = result?;

		let targets = if let Some(data) = nominator_data {
			// Decode the nominations
			if let Ok(nominations) = Nominations::decode(&mut &data.encoded()[..]) {
				// Only include active nominators (not suppressed)
				if !nominations.suppressed { nominations.targets } else { Vec::new() }
			} else {
				Vec::new()
			}
		} else {
			Vec::new()
		};
		batch_results.push((account_id, stake, targets));
	}

	Ok(batch_results)
}
