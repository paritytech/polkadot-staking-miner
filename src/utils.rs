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
use polkadot_sdk::sp_core::crypto::{Ss58AddressFormat, Ss58Codec};
use serde::Deserialize;
use subxt_rpcs::rpc_params;
use anyhow::Context;
use pin_project_lite::pin_project;
use polkadot_sdk::{sp_npos_elections, sp_runtime::Perbill};
use serde::{de::DeserializeOwned, Serialize};
use std::{
	fs::{self, File},
	future::Future,
	io::{Read, Write, BufWriter},
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
	let hash = get_latest_finalized_head(api.chain_api()).await?;
	storage_at(Some(hash), api.chain_api()).await
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
pub async fn write_data_to_json_file<T>(data: &T, file_path: &str) -> anyhow::Result<()>
where
	T: Serialize,
{
	let path = Path::new(file_path);
	if let Some(parent) = path.parent() {
		if !parent.as_os_str().is_empty() {
			fs::create_dir_all(parent).with_context(|| format!("create directory {}", parent.display()))?;
		}
	}

	let file = File::create(path).with_context(|| format!("create {}", path.display()))?;
	let mut writer = BufWriter::with_capacity(1024 * 1024, file);
	let json = serde_json::to_string_pretty(data)
		.with_context(|| format!("serialize {}", path.display()))?;
	writer
		.write_all(json.as_bytes())
		.with_context(|| format!("write {}", path.display()))?;
	writer.flush().with_context(|| format!("flush {}", path.display()))?;

	println!("Wrote JSON data to {}", path.display());
	Ok(())
}

/// Read data from a JSON file
pub async fn read_data_from_json_file<T>(file_path: &str) -> anyhow::Result<T>
where
	T: DeserializeOwned,
{
	let path = Path::new(file_path);
	println!("Reading data from file: {:?}", path);

	let mut file = File::open(path)
		.with_context(|| format!("file not found at {}", path.display()))?;
	let mut content = String::new();
	file
		.read_to_string(&mut content)
		.with_context(|| format!("failed to read {}", path.display()))?;

	serde_json::from_str(&content)
		.with_context(|| format!("failed to deserialize {}", path.display()))
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
			log::warn!(target: LOG_TARGET, "Failed to fetch SS58 prefix: {}", e);
			log::warn!(target: LOG_TARGET, "Using default SS58 prefix: 0");
			Ok(0)
		},
	}
}

/// Get chain properties (token decimals and symbol) from system_properties RPC
pub async fn get_chain_properties(client: &Client) -> Result<(u8, String), Error> {
	// Create a new RPC client for the call
	let rpc_client = subxt::backend::rpc::RpcClient::from_url(client.uri())
		.await
		.map_err(|e| Error::Other(format!("Failed to create RPC client: {}", e)))?;

	// Call system_properties RPC method
	let response: ChainProperties = rpc_client
		.request("system_properties", rpc_params![])
		.await
		.map_err(|e| Error::Other(format!("Failed to call system_properties RPC: {}", e)))?;

	// Extract token decimals (first value from array)
	let decimals = response.token_decimals.unwrap_or(10); // Default to 10 for most Substrate chains

	// Extract token symbol (first value from array)
	let symbol = response.token_symbol.unwrap_or("UNIT".to_string()); // Default symbol

	log::info!(
		target: LOG_TARGET,
		"Fetched chain properties: token_symbol={}, token_decimals={}",
		symbol,
		decimals
	);

	Ok((decimals, symbol))
}

/// Encode an AccountId to SS58 string with chain-specific prefix
pub fn encode_account_id(account: &AccountId, ss58_prefix: u16) -> String {
	// AccountId is already AccountId32, just encode it with the custom prefix
	// Clone to avoid move
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
			format!("{}.{}", whole, remainder_trimmed)
		}
	};

	format!("{} {}", amount_str, symbol)
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
}
