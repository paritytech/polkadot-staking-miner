// Copyright 2021-2022 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

use crate::{
	client::Client,
	commands::types::SubmissionStrategy,
	error::Error,
	prelude::{AccountId, ChainClient, Config, Hash, LOG_TARGET, Storage},
};
use pin_project_lite::pin_project;
use polkadot_sdk::{
	sp_core::crypto::{Ss58AddressFormat, Ss58Codec},
	sp_npos_elections,
	sp_runtime::Perbill,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use ss58_registry::Ss58AddressFormat as RegistryFormat;
use std::{
	fs::{self, File},
	future::Future,
	io::{BufWriter, Read, Write},
	path::Path,
	pin::Pin,
	task::{Context as TaskContext, Poll},
	time::{Duration, Instant},
};
use subxt::tx::{TxInBlock, TxProgress};

pin_project! {
	pub struct Timed<Fut>
		where
		Fut: Future,
	{
		#[pin]
		inner: Fut,
		start: Option<Instant>,
	}
}

impl<Fut> Future for Timed<Fut>
where
	Fut: Future,
{
	type Output = (Fut::Output, Duration);

	fn poll(self: Pin<&mut Self>, cx: &mut TaskContext) -> Poll<Self::Output> {
		let this = self.project();
		let start = this.start.get_or_insert_with(Instant::now);

		match this.inner.poll(cx) {
			Poll::Pending => Poll::Pending,
			Poll::Ready(v) => {
				let elapsed = start.elapsed();
				Poll::Ready((v, elapsed))
			},
		}
	}
}

pub trait TimedFuture: Sized + Future {
	fn timed(self) -> Timed<Self> {
		Timed { inner: self, start: None }
	}
}

impl<F: Future> TimedFuture for F {}

/// Helper to get storage at block.
pub async fn storage_at(block: Option<Hash>, api: &ChainClient) -> Result<Storage, Error> {
	if let Some(block_hash) = block {
		Ok(api.storage().at(block_hash))
	} else {
		Ok(api.storage().at_latest().await?)
	}
}

pub async fn storage_at_head(api: &Client) -> Result<Storage, Error> {
	let chain_api = api.chain_api().await;
	let hash = get_latest_finalized_head(&chain_api).await?;
	storage_at(Some(hash), &chain_api).await
}

pub async fn get_latest_finalized_head(api: &ChainClient) -> Result<Hash, Error> {
	let finalized_block_ref = api.backend().latest_finalized_block_ref().await?;
	Ok(finalized_block_ref.hash())
}

/// Returns `true` if `our_score` better the onchain `best_score` according the given strategy.
pub fn score_passes_strategy(
	our_score: sp_npos_elections::ElectionScore,
	best_score: sp_npos_elections::ElectionScore,
	strategy: SubmissionStrategy,
) -> bool {
	match strategy {
		SubmissionStrategy::Always => true,
		SubmissionStrategy::IfLeading =>
			our_score.strict_threshold_better(best_score, Perbill::zero()),
		SubmissionStrategy::ClaimBetterThan(epsilon) =>
			our_score.strict_threshold_better(best_score, epsilon),
		SubmissionStrategy::ClaimNoWorseThan(epsilon) =>
			!best_score.strict_threshold_better(our_score, epsilon),
	}
}

/// Wait for the transaction to be included in a finalized block.
pub async fn wait_tx_in_finalized_block(
	tx: TxProgress<Config, ChainClient>,
) -> Result<TxInBlock<Config, ChainClient>, Error> {
	tx.wait_for_finalized().await.map_err(Into::into)
}

/// Write data to a JSON file
pub async fn write_data_to_json_file<T>(data: &T, file_path: &str) -> Result<(), Error>
where
	T: Serialize,
{
	let path = Path::new(file_path);
	if let Some(parent) = path.parent() &&
		!parent.as_os_str().is_empty()
	{
		fs::create_dir_all(parent)?;
	}

	let file = File::create(path)?;
	let mut writer = BufWriter::with_capacity(1024 * 1024, file);

	let json = serde_json::to_string_pretty(data)?;
	writer.write_all(json.as_bytes())?;
	writer.flush()?;

	Ok(())
}

/// Read data from a JSON file
pub async fn read_data_from_json_file<T>(file_path: &str) -> Result<T, Error>
where
	T: DeserializeOwned,
{
	let path = Path::new(file_path);

	let mut file = File::open(path)?;
	let mut content = String::new();
	file.read_to_string(&mut content)?;

	Ok(serde_json::from_str(&content)?)
}

/// Chain properties from system_properties RPC call
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChainProperties {
	token_symbol: Option<String>,
	token_decimals: Option<u8>,
}

/// Get the SS58 prefix from the chain
pub async fn get_ss58_prefix(client: &Client) -> Result<u16, Error> {
	match crate::dynamic::pallet_api::system::constants::SS58_PREFIX.fetch(client.chain_api()) {
		Ok(ss58_prefix) => Ok(ss58_prefix),
		Err(e) => {
			log::warn!(target: LOG_TARGET, "Failed to fetch SS58 prefix: {e}");
			log::warn!(target: LOG_TARGET, "Using default SS58 prefix: 0");
			Ok(0)
		},
	}
}

/// Get block hash from block number using RPC
pub async fn get_block_hash(client: &Client, block_number: u32) -> Result<Hash, Error> {
	use serde_json::{json, value::to_raw_value};

	// Convert block number to JSON raw value
	let params = to_raw_value(&json!([block_number]))
		.map_err(|e| Error::Other(format!("Failed to serialize block number: {e}")))?;

	// Make RPC request - params is already Box<RawValue>
	let response = client
		.rpc()
		.request("chain_getBlockHash".to_string(), Some(params))
		.await
		.map_err(|e| {
			Error::Other(format!("Failed to get block hash for block {block_number}: {e}"))
		})?;

	// Parse response - it can be null if block doesn't exist
	let response_str = response.get();
	if response_str == "null" {
		return Err(Error::Other(format!(
			"Block {block_number} not found (may be pruned or invalid)"
		)));
	}

	// Deserialize the hash
	let block_hash: Option<Hash> = serde_json::from_str(response_str)
		.map_err(|e| Error::Other(format!("Failed to parse block hash response: {e}")))?;

	block_hash.ok_or_else(|| {
		Error::Other(format!("Block {block_number} not found (may be pruned or invalid)"))
	})
}

/// Get chain properties (ss58 prefix, token decimals and symbol) from system_properties RPC
pub async fn get_chain_properties(client: Client) -> Result<(u16, u8, String), Error> {
	use serde_json::{json, value::to_raw_value};

	// Call system_properties RPC method using the client's RPC
	let params = to_raw_value(&json!([]))
		.map_err(|e| Error::Other(format!("Failed to serialize RPC params: {e}")))?;

	let response_raw = client
		.rpc()
		.request("system_properties".to_string(), Some(params))
		.await
		.map_err(|e| Error::Other(format!("Failed to call system_properties RPC: {e}")))?;

	// Deserialize the response
	let response: ChainProperties = serde_json::from_str(response_raw.get())
		.map_err(|e| Error::Other(format!("Failed to parse system_properties response: {e}")))?;

	// Extract token decimals
	let decimals = response.token_decimals.unwrap_or(10); // Default to 10 for most Substrate chains

	// Extract token symbol
	let symbol = response.token_symbol.unwrap_or("UNIT".to_string()); // Default symbol

	// fetch the ss58 prefix of the chain
	let ss58_prefix = get_ss58_prefix(&client).await?;

	log::info!(
		target: LOG_TARGET,
		"Fetched chain properties: ss_58 prefix={ss58_prefix} token_symbol={symbol}, token_decimals={decimals}"
	);

	Ok((ss58_prefix, decimals, symbol))
}

/// Encode an AccountId to SS58 string with chain-specific prefix
/// Uses ss58-registry to validate the prefix against known networks
pub fn encode_account_id(account: &AccountId, ss58_prefix: u16) -> String {
	// Use ss58-registry to validate and get network information
	let is_known = RegistryFormat::all().iter().any(|entry| {
		let entry_format: RegistryFormat = (*entry).into();
		entry_format.prefix() == ss58_prefix
	});

	if is_known {
		log::trace!(
			target: LOG_TARGET,
			"Encoding with SS58 prefix {ss58_prefix} (validated in registry)"
		);
	} else {
		log::trace!(
			target: LOG_TARGET,
			"Encoding with SS58 prefix {ss58_prefix} (custom format, not in registry)"
		);
	}

	// Encode using the standard SS58 encoding with the provided prefix
	// The registry validation above ensures we're aware if it's a known network
	account
		.clone()
		.to_ss58check_with_version(Ss58AddressFormat::custom(ss58_prefix))
}

/// Convert Plancks to tokens (divide by 10^decimals) and format with token symbol
pub fn planck_to_token(planck: u128, decimals: u8, symbol: &str) -> String {
	let divisor = 10_u128.pow(decimals as u32);
	let whole = planck / divisor;
	let remainder = planck % divisor;

	let amount_str = if remainder == 0 {
		whole.to_string()
	} else {
		// Format with proper decimal places
		let remainder_str = format!("{:0>width$}", remainder, width = decimals as usize);
		// Remove trailing zeros
		let remainder_trimmed = remainder_str.trim_end_matches('0');
		if remainder_trimmed.is_empty() {
			whole.to_string()
		} else {
			format!("{whole}.{remainder_trimmed}")
		}
	};

	format!("{amount_str} {symbol}")
}

/// Convert Plancks (u64) to tokens with symbol
pub fn planck_to_token_u64(planck: u64, decimals: u8, symbol: &str) -> String {
	planck_to_token(planck as u128, decimals, symbol)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::prelude::Accuracy;
	use std::str::FromStr;

	#[test]
	fn score_passes_strategy_works() {
		let s = |x| sp_npos_elections::ElectionScore { minimal_stake: x, ..Default::default() };
		let two = Perbill::from_percent(2);

		// anything passes Always
		assert!(score_passes_strategy(s(0), s(0), SubmissionStrategy::Always));
		assert!(score_passes_strategy(s(5), s(0), SubmissionStrategy::Always));
		assert!(score_passes_strategy(s(5), s(10), SubmissionStrategy::Always));

		// if leading
		assert!(!score_passes_strategy(s(0), s(0), SubmissionStrategy::IfLeading));
		assert!(score_passes_strategy(s(1), s(0), SubmissionStrategy::IfLeading));
		assert!(score_passes_strategy(s(2), s(0), SubmissionStrategy::IfLeading));
		assert!(!score_passes_strategy(s(5), s(10), SubmissionStrategy::IfLeading));
		assert!(!score_passes_strategy(s(9), s(10), SubmissionStrategy::IfLeading));
		assert!(!score_passes_strategy(s(10), s(10), SubmissionStrategy::IfLeading));

		// if better by 2%
		assert!(!score_passes_strategy(s(50), s(100), SubmissionStrategy::ClaimBetterThan(two)));
		assert!(!score_passes_strategy(s(100), s(100), SubmissionStrategy::ClaimBetterThan(two)));
		assert!(!score_passes_strategy(s(101), s(100), SubmissionStrategy::ClaimBetterThan(two)));
		assert!(!score_passes_strategy(s(102), s(100), SubmissionStrategy::ClaimBetterThan(two)));
		assert!(score_passes_strategy(s(103), s(100), SubmissionStrategy::ClaimBetterThan(two)));
		assert!(score_passes_strategy(s(150), s(100), SubmissionStrategy::ClaimBetterThan(two)));

		// if no less than 2% worse
		assert!(!score_passes_strategy(s(50), s(100), SubmissionStrategy::ClaimNoWorseThan(two)));
		assert!(!score_passes_strategy(s(97), s(100), SubmissionStrategy::ClaimNoWorseThan(two)));
		assert!(score_passes_strategy(s(98), s(100), SubmissionStrategy::ClaimNoWorseThan(two)));
		assert!(score_passes_strategy(s(99), s(100), SubmissionStrategy::ClaimNoWorseThan(two)));
		assert!(score_passes_strategy(s(100), s(100), SubmissionStrategy::ClaimNoWorseThan(two)));
		assert!(score_passes_strategy(s(101), s(100), SubmissionStrategy::ClaimNoWorseThan(two)));
		assert!(score_passes_strategy(s(102), s(100), SubmissionStrategy::ClaimNoWorseThan(two)));
		assert!(score_passes_strategy(s(103), s(100), SubmissionStrategy::ClaimNoWorseThan(two)));
		assert!(score_passes_strategy(s(150), s(100), SubmissionStrategy::ClaimNoWorseThan(two)));
	}

	#[test]
	fn submission_strategy_from_str_works() {
		assert_eq!(SubmissionStrategy::from_str("if-leading"), Ok(SubmissionStrategy::IfLeading));
		assert_eq!(SubmissionStrategy::from_str("always"), Ok(SubmissionStrategy::Always));
		assert_eq!(
			SubmissionStrategy::from_str("  percent-better 99   "),
			Ok(SubmissionStrategy::ClaimBetterThan(Accuracy::from_percent(99)))
		);
	}

	#[tokio::test]
	async fn test_read_write_json_file() {
		let dir = tempfile::tempdir().unwrap();
		let file_path = dir.path().join("test.json");
		let file_path_str = file_path.to_str().unwrap();

		let data = vec![1, 2, 3];
		write_data_to_json_file(&data, file_path_str).await.unwrap();

		let read_data: Vec<i32> = read_data_from_json_file(file_path_str).await.unwrap();
		assert_eq!(data, read_data);
	}

	#[test]
	fn test_planck_to_token() {
		assert_eq!(planck_to_token(100, 2, "DOT"), "1 DOT");
		assert_eq!(planck_to_token(100, 0, "DOT"), "100 DOT");
		assert_eq!(planck_to_token(1234, 3, "DOT"), "1.234 DOT");
		assert_eq!(planck_to_token(1230, 3, "DOT"), "1.23 DOT");
		assert_eq!(planck_to_token(5, 2, "DOT"), "0.05 DOT");
		assert_eq!(planck_to_token(0, 5, "DOT"), "0 DOT");
	}

	#[test]
	fn test_planck_to_token_u64() {
		assert_eq!(planck_to_token_u64(100, 2, "KSM"), "1 KSM");
		assert_eq!(planck_to_token_u64(123456789, 6, "KSM"), "123.456789 KSM");
	}

	#[test]
	fn test_encode_account_id() {
		// Alice's public key
		let alice_pub_key = [
			212, 53, 147, 199, 21, 253, 211, 28, 97, 20, 26, 189, 4, 169, 159, 214, 130, 44, 133,
			88, 133, 76, 205, 227, 154, 86, 132, 231, 165, 109, 162, 125,
		];
		let account = AccountId::new(alice_pub_key);

		// Polkadot prefix (0)
		assert_eq!(
			encode_account_id(&account, 0),
			"15oF4uVJwmo4TdGW7VfQxNLavjCXviqxT9S1MgbjMNHr6Sp5"
		);
		// Generic Substrate (42)
		assert_eq!(
			encode_account_id(&account, 42),
			"5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY"
		);
	}
}
