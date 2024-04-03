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
	epm,
	error::Error,
	helpers::{kill_main_task_if_critical_err, TimedFuture},
	opt::Solver,
	prelude::*,
	prometheus,
	signer::Signer,
	static_types,
};
use clap::Parser;
use codec::{Decode, Encode};
use frame_election_provider_support::NposSolution;
use futures::future::TryFutureExt;
use jsonrpsee::core::ClientError as JsonRpseeError;
use pallet_election_provider_multi_phase::{RawSolution, SolutionOf};
use sp_runtime::Perbill;
use std::{str::FromStr, sync::Arc};
use subxt::{
	backend::{legacy::rpc_methods::DryRunResult, rpc::RpcSubscription},
	config::{DefaultExtrinsicParamsBuilder, Header as _},
	error::RpcError,
	tx::{TxInBlock, TxProgress},
	Error as SubxtError,
};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Parser)]
#[cfg_attr(test, derive(PartialEq))]
pub struct MonitorConfig {
	/// They type of event to listen to.
	///
	/// Typically, finalized is safer and there is no chance of anything going wrong, but it can be
	/// slower. It is recommended to use finalized, if the duration of the signed phase is longer
	/// than the the finality delay.
	#[clap(long, value_enum, default_value_t = Listen::Finalized)]
	pub listen: Listen,

	/// The solver algorithm to use.
	#[clap(subcommand)]
	pub solver: Solver,

	/// Submission strategy to use.
	///
	/// Possible options:
	///
	/// `--submission-strategy if-leading`: only submit if leading.
	///
	/// `--submission-strategy always`: always submit.
	///
	/// `--submission-strategy "percent-better <percent>"`: submit if the submission is `n` percent better.
	///
	/// `--submission-strategy "no-worse-than  <percent>"`: submit if submission is no more than `n` percent worse.
	#[clap(long, value_parser, default_value = "if-leading")]
	pub submission_strategy: SubmissionStrategy,

	/// The path to a file containing the seed of the account. If the file is not found, the seed is
	/// used as-is.
	///
	/// Can also be provided via the `SEED` environment variable.
	///
	/// WARNING: Don't use an account with a large stash for this. Based on how the bot is
	/// configured, it might re-try and lose funds through transaction fees/deposits.
	#[clap(long, short, env = "SEED")]
	pub seed_or_path: String,

	/// Delay in number seconds to wait until starting mining a solution.
	///
	/// At every block when a solution is attempted
	/// a delay can be enforced to avoid submitting at
	/// "same time" and risk potential races with other miners.
	///
	/// When this is enabled and there are competing solutions, your solution might not be submitted
	/// if the scores are equal.
	#[clap(long, default_value_t = 0)]
	pub delay: usize,

	/// Verify the submission by `dry-run` the extrinsic to check the validity.
	/// If the extrinsic is invalid then the submission is ignored and the next block will attempted again.
	///
	/// This requires a RPC endpoint that exposes unsafe RPC methods, if the RPC endpoint doesn't expose unsafe RPC methods
	/// then the miner will be terminated.
	#[clap(long)]
	pub dry_run: bool,
}

/// The type of event to listen to.
///
///
/// Typically, finalized is safer and there is no chance of anything going wrong, but it can be
/// slower. It is recommended to use finalized, if the duration of the signed phase is longer
/// than the the finality delay.
#[cfg_attr(test, derive(PartialEq))]
#[derive(clap::ValueEnum, Debug, Copy, Clone)]
pub enum Listen {
	/// Latest finalized head of the canonical chain.
	Finalized,
	/// Latest head of the canonical chain.
	Head,
}

/// Submission strategy to use.
#[derive(Debug, Copy, Clone)]
#[cfg_attr(test, derive(PartialEq))]
pub enum SubmissionStrategy {
	/// Always submit.
	Always,
	// Submit if we are leading, or if the solution that's leading is more that the given `Perbill`
	// better than us. This helps detect obviously fake solutions and still combat them.
	/// Only submit if at the time, we are the best (or equal to it).
	IfLeading,
	/// Submit if we are no worse than `Perbill` worse than the best.
	ClaimNoWorseThan(Perbill),
	/// Submit if we are leading, or if the solution that's leading is more that the given `Perbill`
	/// better than us. This helps detect obviously fake solutions and still combat them.
	ClaimBetterThan(Perbill),
}

/// Custom `impl` to parse `SubmissionStrategy` from CLI.
///
/// Possible options:
/// * --submission-strategy if-leading: only submit if leading
/// * --submission-strategy always: always submit
/// * --submission-strategy "percent-better <percent>": submit if submission is `n` percent better.
///
impl FromStr for SubmissionStrategy {
	type Err = String;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		let s = s.trim();

		let res = if s == "if-leading" {
			Self::IfLeading
		} else if s == "always" {
			Self::Always
		} else if let Some(percent) = s.strip_prefix("no-worse-than ") {
			let percent: u32 = percent.parse().map_err(|e| format!("{:?}", e))?;
			Self::ClaimNoWorseThan(Perbill::from_percent(percent))
		} else if let Some(percent) = s.strip_prefix("percent-better ") {
			let percent: u32 = percent.parse().map_err(|e| format!("{:?}", e))?;
			Self::ClaimBetterThan(Perbill::from_percent(percent))
		} else {
			return Err(s.into())
		};
		Ok(res)
	}
}

pub async fn monitor_cmd<T>(
	client: Client,
	config: MonitorConfig,
	signed_phase_length: u64,
) -> Result<(), Error>
where
	T: MinerConfig<AccountId = AccountId, MaxVotesPerVoter = static_types::MaxVotesPerVoter>
		+ Send
		+ Sync
		+ 'static,
	T::Solution: Send,
{
	let signer = Signer::new(&config.seed_or_path)?;

	let account_info = {
		let addr = runtime::storage().system().account(signer.account_id());
		client
			.chain_api()
			.storage()
			.at_latest()
			.await?
			.fetch(&addr)
			.await?
			.ok_or(Error::AccountDoesNotExists)?
	};

	log::info!(target: LOG_TARGET, "Loaded account {} {{ nonce: {}, free_balance: {}, reserved_balance: {}, frozen_balance: {} }}",
		signer,
		account_info.nonce,
		account_info.data.free,
		account_info.data.reserved,
		account_info.data.frozen,
	);

	if config.dry_run {
		// if we want to try-run, ensure the node supports it.
		dry_run_works(client.rpc()).await?;
	}

	let mut subscription = heads_subscription(client.rpc(), config.listen).await?;
	let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Error>();
	let submit_lock = Arc::new(Mutex::new(()));

	loop {
		let at = tokio::select! {
			maybe_rp = subscription.next() => {
				match maybe_rp {
					Some(Ok(r)) => r,
					Some(Err(e)) => {
						log::error!(target: LOG_TARGET, "subscription failed to decode Header {:?}, this is bug please file an issue", e);
						return Err(e.into());
					}
					// The subscription was dropped, should only happen if:
					//	- the connection was closed.
					//	- the subscription could not keep up with the server.
					None => {
						log::warn!(target: LOG_TARGET, "subscription to `{:?}` terminated. Retrying..", config.listen);
						subscription = heads_subscription(client.rpc(), config.listen).await?;
						continue
					}
				}
			},
			maybe_err = rx.recv() => {
				match maybe_err {
					Some(err) => return Err(err),
					None => unreachable!("at least one sender kept in the main loop should always return Some; qed"),
				}
			}
		};

		// Spawn task and non-recoverable errors are sent back to the main task
		// such as if the connection has been closed.
		let tx2 = tx.clone();
		let client2 = client.clone();
		let signer2 = signer.clone();
		let config2 = config.clone();
		let submit_lock2 = submit_lock.clone();
		tokio::spawn(async move {
			if let Err(err) = mine_and_submit_solution::<T>(
				at,
				client2,
				signer2,
				config2,
				submit_lock2,
				signed_phase_length,
			)
			.await
			{
				kill_main_task_if_critical_err(&tx2, err)
			}
		});

		let account_info = client
			.chain_api()
			.storage()
			.at_latest()
			.await?
			.fetch(&runtime::storage().system().account(signer.account_id()))
			.await?
			.ok_or(Error::AccountDoesNotExists)?;
		// this is lossy but fine for now.
		prometheus::set_balance(account_info.data.free as f64);
	}
}

/// Construct extrinsic at given block and watch it.
async fn mine_and_submit_solution<T>(
	at: Header,
	client: Client,
	signer: Signer,
	config: MonitorConfig,
	submit_lock: Arc<Mutex<()>>,
	signed_phase_len: u64,
) -> Result<(), Error>
where
	T: MinerConfig<AccountId = AccountId, MaxVotesPerVoter = static_types::MaxVotesPerVoter>
		+ Send
		+ Sync
		+ 'static,
	T::Solution: Send,
{
	let block_hash = at.hash();
	log::trace!(target: LOG_TARGET, "new event at #{:?} ({:?})", at.number, block_hash);

	// NOTE: as we try to send at each block then the nonce is used guard against
	// submitting twice. Because once a solution has been accepted on chain
	// the "next transaction" at a later block but with the same nonce will be rejected
	let nonce = client.rpc().system_account_next_index(signer.account_id()).await?;

	ensure_signed_phase(client.chain_api(), block_hash)
		.inspect_err(|e| {
			log::debug!(
				target: LOG_TARGET,
				"ensure_signed_phase failed: {:?}; skipping block: {}",
				e,
				at.number
			)
		})
		.await?;

	let round_fut = async {
		client
			.chain_api()
			.storage()
			.at(block_hash)
			.fetch_or_default(&runtime::storage().election_provider_multi_phase().round())
			.await
	};

	let round = round_fut
		.inspect_err(|e| log::error!(target: LOG_TARGET, "Mining solution failed: {:?}", e))
		.await?;

	ensure_no_previous_solution::<T::Solution>(
		client.chain_api(),
		block_hash,
		&signer.account_id().0.into(),
	)
	.inspect_err(|e| {
		log::debug!(
			target: LOG_TARGET,
			"ensure_no_previous_solution failed: {:?}; skipping block: {}",
			e,
			at.number
		)
	})
	.await?;

	tokio::time::sleep(std::time::Duration::from_secs(config.delay as u64)).await;
	let _lock = submit_lock.lock().await;

	let (solution, score) = match epm::fetch_snapshot_and_mine_solution::<T>(
		&client.chain_api(),
		Some(block_hash),
		config.solver,
		round,
		None,
	)
	.timed()
	.await
	{
		(Ok(mined_solution), elapsed) => {
			// check that the solution looks valid:
			mined_solution.feasibility_check()?;

			// and then get the values we need from it:
			let solution = mined_solution.solution();
			let score = mined_solution.score();
			let size = mined_solution.size();

			let elapsed_ms = elapsed.as_millis();
			let encoded_len = solution.encoded_size();
			let active_voters = solution.voter_count() as u32;
			let desired_targets = solution.unique_targets().len() as u32;

			let final_weight = tokio::task::spawn_blocking(move || {
				T::solution_weight(size.voters, size.targets, active_voters, desired_targets)
			})
			.await?;

			log::info!(
				target: LOG_TARGET,
				"Mined solution with {:?} size: {:?} round: {:?} at: {}, took: {} ms, len: {:?}, weight = {:?}",
				score,
				size,
				round,
				at.number(),
				elapsed_ms,
				encoded_len,
				final_weight,
			);

			prometheus::set_length(encoded_len);
			prometheus::set_weight(final_weight);
			prometheus::observe_mined_solution_duration(elapsed_ms as f64);
			prometheus::set_score(score);

			(solution, score)
		},
		(Err(e), _) => return Err(Error::Other(e.to_string())),
	};

	let best_head = get_latest_head(client.rpc(), config.listen).await?;

	ensure_signed_phase(client.chain_api(), best_head)
		.inspect_err(|e| {
			log::debug!(
				target: LOG_TARGET,
				"ensure_signed_phase failed: {:?}; skipping block: {}",
				e,
				at.number
			)
		})
		.await?;

	ensure_no_previous_solution::<T::Solution>(
		client.chain_api(),
		best_head,
		&signer.account_id().0.into(),
	)
	.inspect_err(|e| {
		log::debug!(
			target: LOG_TARGET,
			"ensure_no_previous_solution failed: {:?}; skipping block: {:?}",
			e,
			best_head,
		)
	})
	.await?;

	match ensure_solution_passes_strategy(
		client.chain_api(),
		best_head,
		score,
		config.submission_strategy,
	)
	.timed()
	.await
	{
		(Ok(_), now) => {
			log::trace!(
				target: LOG_TARGET,
				"Solution validity verification took: {} ms",
				now.as_millis()
			);
		},
		(Err(e), _) => {
			log::debug!(
				target: LOG_TARGET,
				"ensure_no_better_solution failed: {:?}; skipping block: {}",
				e,
				at.number
			);
			return Err(e)
		},
	};

	prometheus::on_submission_attempt();
	match submit_and_watch_solution::<T>(
		&client,
		signer,
		(solution, score, round),
		nonce,
		config.listen,
		config.dry_run,
		&at,
		signed_phase_len,
	)
	.timed()
	.await
	{
		(Ok(_), now) => {
			prometheus::on_submission_success();
			prometheus::observe_submit_and_watch_duration(now.as_millis() as f64);
		},
		(Err(e), _) => {
			log::warn!(
				target: LOG_TARGET,
				"submit_and_watch_solution failed: {e}; skipping block: {}",
				at.number
			);
		},
	};
	Ok(())
}

/// Ensure that now is the signed phase.
async fn ensure_signed_phase(api: &ChainClient, block_hash: Hash) -> Result<(), Error> {
	use pallet_election_provider_multi_phase::Phase;

	let addr = runtime::storage().election_provider_multi_phase().current_phase();
	let phase = api.storage().at(block_hash).fetch(&addr).await?;

	if let Some(Phase::Signed) = phase.map(|p| p.0) {
		Ok(())
	} else {
		Err(Error::IncorrectPhase)
	}
}

/// Ensure that our current `us` have not submitted anything previously.
async fn ensure_no_previous_solution<T>(
	api: &ChainClient,
	block_hash: Hash,
	us: &AccountId,
) -> Result<(), Error>
where
	T: NposSolution + scale_info::TypeInfo + Decode + 'static,
{
	let addr = runtime::storage().election_provider_multi_phase().signed_submission_indices();
	let indices = api.storage().at(block_hash).fetch_or_default(&addr).await?;

	for (_score, _, idx) in indices.0 {
		let submission = epm::signed_submission_at::<T>(idx, Some(block_hash), api).await?;

		if let Some(submission) = submission {
			if &submission.who == us {
				return Err(Error::AlreadySubmitted)
			}
		}
	}

	Ok(())
}

async fn ensure_solution_passes_strategy(
	api: &ChainClient,
	block_hash: Hash,
	score: sp_npos_elections::ElectionScore,
	strategy: SubmissionStrategy,
) -> Result<(), Error> {
	// don't care about current scores.
	if matches!(strategy, SubmissionStrategy::Always) {
		return Ok(())
	}

	let addr = runtime::storage().election_provider_multi_phase().signed_submission_indices();
	let indices = api.storage().at(block_hash).fetch_or_default(&addr).await?;

	log::debug!(target: LOG_TARGET, "submitted solutions: {:?}", indices.0);

	if indices
		.0
		.last()
		.map_or(true, |(best_score, _, _)| score_passes_strategy(score, best_score.0, strategy))
	{
		Ok(())
	} else {
		Err(Error::BetterScoreExist)
	}
}

async fn submit_and_watch_solution<T: MinerConfig + Send + Sync + 'static>(
	client: &Client,
	signer: Signer,
	(solution, score, round): (SolutionOf<T>, sp_npos_elections::ElectionScore, u32),
	nonce: u64,
	listen: Listen,
	dry_run: bool,
	at: &Header,
	signed_phase_length: u64,
) -> Result<(), Error> {
	let tx = epm::signed_solution(RawSolution { solution, score, round })?;

	// TODO: https://github.com/paritytech/polkadot-staking-miner/issues/730
	//
	// The extrinsic mortality length is static and it doesn't know when the signed phase ends.
	let xt_cfg = DefaultExtrinsicParamsBuilder::default()
		.nonce(nonce)
		.mortal(at, signed_phase_length)
		.build();

	let xt = client.chain_api().tx().create_signed(&tx, &*signer, xt_cfg).await?;

	if dry_run {
		let dry_run_bytes = client.rpc().dry_run(xt.encoded(), None).await?;

		match dry_run_bytes.into_dry_run_result(&client.chain_api().metadata())? {
			DryRunResult::Success => (),
			DryRunResult::DispatchError(e) => return Err(Error::TransactionRejected(e.to_string())),
			DryRunResult::TransactionValidityError =>
				return Err(Error::TransactionRejected("TransactionValidityError".into())),
		}
	}

	let tx_progress = xt.submit_and_watch().await.map_err(|e| {
		log::warn!(target: LOG_TARGET, "submit solution failed: {:?}", e);
		e
	})?;

	match listen {
		Listen::Head => {
			let in_block = wait_for_in_block(tx_progress).await?;
			let events = in_block.fetch_events().await.expect("events should exist");

			let solution_stored = events
				.find_first::<runtime::election_provider_multi_phase::events::SolutionStored>(
			);

			if let Ok(Some(_)) = solution_stored {
				log::info!(target: LOG_TARGET, "Included at {:?}", in_block.block_hash());
			} else {
				return Err(Error::Other(format!(
					"No SolutionStored event found at {:?}",
					in_block.block_hash()
				)))
			}
		},
		Listen::Finalized => {
			let finalized = tx_progress.wait_for_finalized_success().await?;

			let solution_stored = finalized
				.find_first::<runtime::election_provider_multi_phase::events::SolutionStored>(
			);

			if let Ok(Some(_)) = solution_stored {
				log::info!(target: LOG_TARGET, "Finalized at {:?}", finalized.block_hash());
			} else {
				return Err(Error::Other(format!(
					"No SolutionStored event found at {:?}",
					finalized.block_hash()
				)))
			}
		},
	};

	Ok(())
}

async fn heads_subscription(
	rpc: &RpcClient,
	listen: Listen,
) -> Result<RpcSubscription<Header>, Error> {
	match listen {
		Listen::Head => rpc.chain_subscribe_new_heads().await,
		Listen::Finalized => rpc.chain_subscribe_finalized_heads().await,
	}
	.map_err(Into::into)
}

async fn get_latest_head(rpc: &RpcClient, listen: Listen) -> Result<Hash, Error> {
	match listen {
		Listen::Head => match rpc.chain_get_block_hash(None).await {
			Ok(Some(hash)) => Ok(hash),
			Ok(None) => Err(Error::Other("Latest block not found".into())),
			Err(e) => Err(e.into()),
		},
		Listen::Finalized => rpc.chain_get_finalized_head().await.map_err(Into::into),
	}
}

/// Returns `true` if `our_score` better the onchain `best_score` according the given strategy.
pub(crate) fn score_passes_strategy(
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

async fn dry_run_works(rpc: &RpcClient) -> Result<(), Error> {
	if let Err(SubxtError::Rpc(RpcError::ClientError(e))) = rpc.dry_run(&[], None).await {
		let rpc_err = match e.downcast::<JsonRpseeError>() {
			Ok(e) => *e,
			Err(_) =>
				return Err(Error::Other(
					"Failed to downcast RPC error; this is a bug please file an issue".to_string(),
				)),
		};

		if let JsonRpseeError::Call(e) = rpc_err {
			if e.message() == "RPC call is unsafe to be called externally" {
				return Err(Error::Other(
					"dry-run requires a RPC endpoint with `--rpc-methods unsafe`; \
						either connect to another RPC endpoint or disable dry-run"
						.to_string(),
				))
			}
		}
	}
	Ok(())
}

/// Wait for the transaction to be in a block.
///
/// **Note:** transaction statuses like `Invalid`/`Usurped`/`Dropped` indicate with some
/// probability that the transaction will not make it into a block but there is no guarantee
/// that this is true. In those cases the stream is closed however, so you currently have no way to find
/// out if they finally made it into a block or not.
async fn wait_for_in_block<T, C>(mut tx: TxProgress<T, C>) -> Result<TxInBlock<T, C>, subxt::Error>
where
	T: subxt::Config,
	C: subxt::client::OnlineClientT<T>,
{
	use subxt::{error::TransactionError, tx::TxStatus};

	while let Some(status) = tx.next().await {
		match status? {
			// Finalized or otherwise in a block! Return.
			TxStatus::InBestBlock(s) | TxStatus::InFinalizedBlock(s) => return Ok(s),
			// Error scenarios; return the error.
			TxStatus::Error { message } => return Err(TransactionError::Error(message).into()),
			TxStatus::Invalid { message } => return Err(TransactionError::Invalid(message).into()),
			TxStatus::Dropped { message } => return Err(TransactionError::Dropped(message).into()),
			// Ignore anything else and wait for next status event:
			_ => continue,
		}
	}
	Err(RpcError::SubscriptionDropped.into())
}

#[cfg(test)]
mod tests {
	use super::*;

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
