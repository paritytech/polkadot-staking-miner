//! Shared utilities for fetching staking data

use crate::{
	client::Client,
	dynamic::{pallet_api, utils::storage_addr},
	error::Error,
	prelude::{AccountId, STAKING_LOG_TARGET as LOG_TARGET},
};
use codec::{Decode, Encode};
use scale_value::At;
use std::time::Duration;
use subxt::dynamic::Value;

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
	const MAX_CONCURRENT_BATCHES: usize = 15;
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
						let error_msg = format!("{e}");
						if (error_msg.contains("limit reached") || error_msg.contains("RPC error")) &&
							attempt < MAX_RETRIES
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
						"Total accounts processed: {processed_accounts}"
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
						"Total accounts processed: {processed_accounts}"
					);
				},
				Ok(Err(e)) => return Err(e),
				Err(e) => return Err(Error::Other(format!("Task join error: {e}"))),
			}
		}
	}

	log::info!(
		target: LOG_TARGET,
		"Completed fetching stakes for {processed_accounts} accounts"
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
			if let Some(ledger) = at.fetch(&ledger_addr).await? &&
				let Ok(value) = ledger.to_value()
			{
				// Try to get 'active' first (self-stake), fallback to 'total' if 'active' not
				// available In Polkadot, 'active' is the active stake which includes
				// self-stake 'total' is the total stake (self + nominators)
				// For election purposes, we use 'active' as it represents the validator's own
				// stake
				if let Some(active) = value.at("active").and_then(|v| v.as_u128()) {
					stake = active;
				} else if let Some(total) = value.at("total").and_then(|v| v.as_u128()) {
					// Fallback to total if active is not available
					stake = total;
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
		pub suppressed: bool,
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

		// only consider active nominators
		if !nominations.suppressed {
			nominators.push((stash.clone(), nominations.targets));
		} else {
			log::info!(target: LOG_TARGET, "Skipping suppressed nominator: {stash}");
		}

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
