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
	epm,
	error::Error,
	helpers::TimedFuture,
	opt::{Listen, MonitorConfig, SubmissionStrategy},
	prelude::*,
	prometheus,
	signer::Signer,
	static_types,
};
use codec::{Decode, Encode};
use frame_election_provider_support::NposSolution;
use pallet_election_provider_multi_phase::{RawSolution, SolutionOf};
use sp_runtime::Perbill;
use std::sync::Arc;
use subxt::{error::RpcError, rpc::Subscription, tx::TxStatus};
use tokio::sync::Mutex;

pub async fn monitor_cmd<T>(api: SubxtClient, config: MonitorConfig) -> Result<(), Error>
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
		api.storage().fetch(&addr, None).await?.ok_or(Error::AccountDoesNotExists)?
	};

	log::info!(target: LOG_TARGET, "Loaded account {}, {:?}", signer, account_info);

	let mut subscription = heads_subscription(&api, config.listen).await?;
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
						subscription = heads_subscription(&api, config.listen).await?;
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
		tokio::spawn(mine_and_submit_solution::<T>(
			tx.clone(),
			at,
			api.clone(),
			signer.clone(),
			config.clone(),
			submit_lock.clone(),
		));

		let account_info = {
			let addr = runtime::storage().system().account(signer.account_id());
			api.storage().fetch(&addr, None).await?.ok_or(Error::AccountDoesNotExists)?
		};
		// this is lossy but fine for now.
		prometheus::set_balance(account_info.data.free as f64);
	}
}

fn kill_main_task_if_critical_err(tx: &tokio::sync::mpsc::UnboundedSender<Error>, err: Error) {
	use jsonrpsee::{core::Error as JsonRpseeError, types::error::CallError};
	use subxt::Error as SubxtError;

	match err {
		Error::AlreadySubmitted |
		Error::BetterScoreExist |
		Error::IncorrectPhase |
		Error::TransactionRejected(_) |
		Error::SubscriptionClosed => {},
		Error::Subxt(SubxtError::Rpc(e)) => {
			log::debug!(target: LOG_TARGET, "rpc error: {:?}", e);

			match e {
				RpcError::ClientError(e) => {
					let e = match e.downcast::<JsonRpseeError>() {
						Ok(e) => *e,
						Err(_) => {
							let _ = tx.send(Error::Other(
								"Failed to downcast RPC error; this is a bug please file an issue"
									.to_string(),
							));
							return
						},
					};

					match e {
						JsonRpseeError::Call(CallError::Custom(e)) => {
							const BAD_EXTRINSIC_FORMAT: i32 = 1001;
							const VERIFICATION_ERROR: i32 = 1002;

							// Check if the transaction gets fatal errors from `author` RPC.
							// It's possible to get other errors such as outdated nonce and similar
							// but then it should be possible to try again in the next block or round.
							if e.code() == BAD_EXTRINSIC_FORMAT || e.code() == VERIFICATION_ERROR {
								let _ = tx.send(Error::Subxt(SubxtError::Rpc(
									RpcError::ClientError(Box::new(CallError::Custom(e))),
								)));
							}
						},
						JsonRpseeError::Call(CallError::Failed(_)) => {},
						JsonRpseeError::RequestTimeout => {},
						err => {
							let _ = tx.send(Error::Subxt(SubxtError::Rpc(RpcError::ClientError(
								Box::new(err),
							))));
						},
					}
				},
				RpcError::SubscriptionDropped => (),
			}
		},
		err => {
			let _ = tx.send(err);
		},
	}
}

/// Construct extrinsic at given block and watch it.
async fn mine_and_submit_solution<T>(
	tx: tokio::sync::mpsc::UnboundedSender<Error>,
	at: Header,
	api: SubxtClient,
	signer: Signer,
	config: MonitorConfig,
	submit_lock: Arc<Mutex<()>>,
) where
	T: MinerConfig<AccountId = AccountId, MaxVotesPerVoter = static_types::MaxVotesPerVoter>
		+ Send
		+ Sync
		+ 'static,
	T::Solution: Send,
{
	let hash = at.hash();
	log::trace!(target: LOG_TARGET, "new event at #{:?} ({:?})", at.number, hash);

	// NOTE: as we try to send at each block then the nonce is used guard against
	// submitting twice. Because once a solution has been accepted on chain
	// the "next transaction" at a later block but with the same nonce will be rejected
	let nonce = match api.rpc().system_account_next_index(signer.account_id()).await {
		Ok(none) => none,
		Err(e) => {
			kill_main_task_if_critical_err(&tx, e.into());
			return
		},
	};

	if let Err(e) = ensure_signed_phase(&api, hash).await {
		log::debug!(
			target: LOG_TARGET,
			"ensure_signed_phase failed: {:?}; skipping block: {}",
			e,
			at.number
		);

		kill_main_task_if_critical_err(&tx, e);
		return
	}

	let round = match api
		.storage()
		.fetch(&runtime::storage().election_provider_multi_phase().round(), Some(hash))
		.await
	{
		Ok(Some(round)) => round,
		// Default round is 1
		// https://github.com/paritytech/substrate/blob/49b06901eb65f2c61ff0934d66987fd955d5b8f5/frame/election-provider-multi-phase/src/lib.rs#L1188
		Ok(None) => 1,
		Err(e) => {
			log::error!(target: LOG_TARGET, "Mining solution failed: {:?}", e);
			kill_main_task_if_critical_err(&tx, e.into());
			return
		},
	};

	let _lock = submit_lock.lock().await;

	if let Err(e) =
		ensure_no_previous_solution::<T::Solution>(&api, hash, signer.account_id()).await
	{
		log::debug!(
			target: LOG_TARGET,
			"ensure_no_previous_solution failed: {:?}; skipping block: {}",
			e,
			at.number
		);

		kill_main_task_if_critical_err(&tx, e);
		return
	}

	let (solution, score) =
		match epm::fetch_snapshot_and_mine_solution::<T>(&api, Some(hash), config.solver)
			.timed()
			.await
		{
			(Ok((solution, score, size)), elapsed) => {
				let elapsed_ms = elapsed.as_millis();
				let encoded_len = solution.encoded_size();
				let active_voters = solution.voter_count() as u32;
				let desired_targets = solution.unique_targets().len() as u32;

				let final_weight = tokio::task::spawn_blocking(move || {
					T::solution_weight(size.voters, size.targets, active_voters, desired_targets)
				})
				.await
				.unwrap();

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
			(Err(e), _) => {
				kill_main_task_if_critical_err(&tx, e);
				return
			},
		};

	let best_head = match get_latest_head(&api, config.listen).await {
		Ok(head) => head,
		Err(e) => {
			kill_main_task_if_critical_err(&tx, e);
			return
		},
	};

	if let Err(e) = ensure_signed_phase(&api, best_head).await {
		log::debug!(
			target: LOG_TARGET,
			"ensure_signed_phase failed: {:?}; skipping block: {}",
			e,
			at.number
		);
		kill_main_task_if_critical_err(&tx, e);
		return
	}

	match ensure_solution_passes_strategy(&api, best_head, score, config.submission_strategy)
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
			kill_main_task_if_critical_err(&tx, e);
			return
		},
	};

	prometheus::on_submission_attempt();
	match submit_and_watch_solution::<T>(&api, signer, (solution, score, round), hash, nonce)
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
				"submit_and_watch_solution failed: {:?}; skipping block: {}",
				e,
				at.number
			);
			kill_main_task_if_critical_err(&tx, e);
		},
	};
}

/// Ensure that now is the signed phase.
async fn ensure_signed_phase(api: &SubxtClient, hash: Hash) -> Result<(), Error> {
	use pallet_election_provider_multi_phase::Phase;

	let addr = runtime::storage().election_provider_multi_phase().current_phase();
	let res = api.storage().fetch(&addr, Some(hash)).await;

	match res {
		Ok(Some(Phase::Signed)) => Ok(()),
		Ok(Some(_)) => Err(Error::IncorrectPhase),
		// Default phase is None
		// https://github.com/paritytech/substrate/blob/49b06901eb65f2c61ff0934d66987fd955d5b8f5/frame/election-provider-multi-phase/src/lib.rs#L1193
		Ok(None) => Err(Error::IncorrectPhase),
		Err(e) => Err(e.into()),
	}
}

/// Ensure that our current `us` have not submitted anything previously.
async fn ensure_no_previous_solution<T>(
	api: &SubxtClient,
	at: Hash,
	us: &AccountId,
) -> Result<(), Error>
where
	T: NposSolution + scale_info::TypeInfo + Decode + 'static,
{
	let addr = runtime::storage().election_provider_multi_phase().signed_submission_indices();
	let indices = api.storage().fetch_or_default(&addr, Some(at)).await?;

	for (_score, _, idx) in indices.0 {
		let submission = epm::signed_submission_at::<T>(idx, at, api).await?;

		if let Some(submission) = submission {
			if &submission.who == us {
				return Err(Error::AlreadySubmitted)
			}
		}
	}

	Ok(())
}

async fn ensure_solution_passes_strategy(
	api: &SubxtClient,
	at: Hash,
	score: sp_npos_elections::ElectionScore,
	strategy: SubmissionStrategy,
) -> Result<(), Error> {
	// don't care about current scores.
	if matches!(strategy, SubmissionStrategy::Always) {
		return Ok(())
	}

	let addr = runtime::storage().election_provider_multi_phase().signed_submission_indices();
	let indices = api.storage().fetch_or_default(&addr, Some(at)).await?;

	log::debug!(target: LOG_TARGET, "submitted solutions: {:?}", indices.0);

	if indices
		.0
		.last()
		.map_or(true, |(best_score, _, _)| score_passes_strategy(score, *best_score, strategy))
	{
		Ok(())
	} else {
		Err(Error::BetterScoreExist)
	}
}

async fn submit_and_watch_solution<T: MinerConfig + Send + Sync + 'static>(
	api: &SubxtClient,
	signer: Signer,
	(solution, score, round): (SolutionOf<T>, sp_npos_elections::ElectionScore, u32),
	hash: Hash,
	nonce: u32,
) -> Result<(), Error> {
	let tx = epm::signed_solution(RawSolution { solution, score, round })?;

	let xt = api
		.tx()
		.create_signed_with_nonce(&tx, &*signer, nonce, ExtrinsicParams::default())?;

	let mut status_sub = xt.submit_and_watch().await.map_err(|e| {
		log::warn!(target: LOG_TARGET, "submit solution failed: {:?}", e);
		e
	})?;

	loop {
		let status = match status_sub.next_item().await {
			Some(Ok(status)) => status,
			Some(Err(err)) => {
				log::error!(
					target: LOG_TARGET,
					"watch submit extrinsic at {:?} failed: {:?}",
					hash,
					err
				);
				return Err(err.into())
			},
			None => return Err(Error::SubscriptionClosed),
		};

		match status {
			TxStatus::Ready | TxStatus::Broadcast(_) | TxStatus::Future => (),
			TxStatus::InBlock(details) => {
				let events = details.fetch_events().await.expect("events should exist");

				let solution_stored =
					events
						.find_first::<runtime::election_provider_multi_phase::events::SolutionStored>(
						);

				return if let Ok(Some(_)) = solution_stored {
					log::info!(target: LOG_TARGET, "Included at {:?}", details.block_hash());
					Ok(())
				} else {
					Err(Error::Other(format!(
						"No SolutionStored event found at {:?}",
						details.block_hash()
					)))
				}
			},
			TxStatus::Retracted(hash) => {
				log::info!(target: LOG_TARGET, "Retracted at {:?}", hash);
			},
			TxStatus::Finalized(details) => {
				log::info!(target: LOG_TARGET, "Finalized at {:?}", details.block_hash());
				return Ok(())
			},
			_ => return Err(Error::TransactionRejected(format!("{:?}", status))),
		}
	}
}

async fn heads_subscription(
	api: &SubxtClient,
	listen: Listen,
) -> Result<Subscription<Header>, Error> {
	match listen {
		Listen::Head => api.rpc().subscribe_best_block_headers().await,
		Listen::Finalized => api.rpc().subscribe_finalized_block_headers().await,
	}
	.map_err(Into::into)
}

async fn get_latest_head(api: &SubxtClient, listen: Listen) -> Result<Hash, Error> {
	match listen {
		Listen::Head => match api.rpc().block_hash(None).await {
			Ok(Some(hash)) => Ok(hash),
			Ok(None) => Err(Error::Other("Latest block not found".into())),
			Err(e) => Err(e.into()),
		},
		Listen::Finalized => api.rpc().finalized_head().await.map_err(Into::into),
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
			our_score == best_score ||
				our_score.strict_threshold_better(best_score, Perbill::zero()),
		SubmissionStrategy::ClaimBetterThan(epsilon) =>
			our_score.strict_threshold_better(best_score, epsilon),
		SubmissionStrategy::ClaimNoWorseThan(epsilon) =>
			!best_score.strict_threshold_better(our_score, epsilon),
	}
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
		assert!(score_passes_strategy(s(0), s(0), SubmissionStrategy::IfLeading));
		assert!(score_passes_strategy(s(1), s(0), SubmissionStrategy::IfLeading));
		assert!(score_passes_strategy(s(2), s(0), SubmissionStrategy::IfLeading));
		assert!(!score_passes_strategy(s(5), s(10), SubmissionStrategy::IfLeading));
		assert!(!score_passes_strategy(s(9), s(10), SubmissionStrategy::IfLeading));
		assert!(score_passes_strategy(s(10), s(10), SubmissionStrategy::IfLeading));

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
}
