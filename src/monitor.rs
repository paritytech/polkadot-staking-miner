use crate::{prelude::*, Error, MonitorConfig, SubmissionStrategy};
use pallet_election_provider_multi_phase::RawSolution;
use sp_runtime::Perbill;
use std::sync::Arc;
use subxt::{BasicError as SubxtError, TransactionStatus};

macro_rules! monitor_cmd_for {
	($runtime:tt) => {
		paste::paste! {
			/// The monitor command.
			pub(crate) async fn [<run_$runtime>] (api: $crate::chain::$runtime::RuntimeApi, config: MonitorConfig, signer: Arc<Signer>) -> Result<(), Error> {
				let mut subscription = if config.listen == "head" {
					api.client.rpc().subscribe_blocks().await
				} else {
					api.client.rpc().subscribe_finalized_blocks().await
				}?;

				let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Error>();

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
									log::warn!(target: LOG_TARGET, "subscription to `subscribeNewHeads/subscribeFinalizedHeads` terminated. Retrying..");
									subscription = if config.listen == "head" {
										api.client.rpc().subscribe_blocks().await
									} else {
										api.client.rpc().subscribe_finalized_blocks().await
									}?;

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
					tokio::spawn(mine_and_submit_solution(
							tx.clone(),
							at,
							api.clone(),
							signer.clone(),
							config.clone(),
					));
				}

				/// Construct extrinsic at given block and watch it.
				async fn mine_and_submit_solution(
					tx: tokio::sync::mpsc::UnboundedSender<Error>,
					at: Header,
					api: $crate::chain::$runtime::RuntimeApi,
					signer: Arc<Signer>,
					config: MonitorConfig,
				) {
					use crate::helpers::*;

					let hash = at.hash();
					log::trace!(target: LOG_TARGET, "new event at #{:?} ({:?})", at.number, hash);

					// TODO(niklasad1): hack to copy thread local storage from `parameter_types`.
					[<tls_update_runtime_constants_$runtime>](&api);

					if let Err(e) = ensure_signed_phase(&api, hash).await {
						log::debug!(
							target: LOG_TARGET,
							"ensure_signed_phase failed: {:?}; skipping block: {}",
							e,
							at.number
						);

						kill_main_task_if_critical_err(&tx, e);
						return;
					}

					let round = match api.storage().election_provider_multi_phase().round(Some(hash)).await {
						Ok(round) => round,
						Err(e) => {
							log::error!(target: LOG_TARGET, "Mining solution failed: {:?}", e);

							kill_main_task_if_critical_err(&tx, e.into());
							return;
						},
					};


					let now = std::time::Instant::now();
					let (solution, score, size) =
					match crate::helpers::[<mine_solution_$runtime>](&api, Some(hash), config.solver)
					.await {
						Ok(s) => s,
						Err(e) => {
							kill_main_task_if_critical_err(&tx, e);
							return;
						}
					};


					log::trace!(target: LOG_TARGET, "Mined solution with {:?} size: {:?} round: {:?}, took: {} ms", score, size, round, now.elapsed().as_millis());

					let xt = match api
						.tx()
						.election_provider_multi_phase()
						.submit(RawSolution { solution, score, round }) {
							Ok(xt) => xt,
							Err(e) => {
								kill_main_task_if_critical_err(&tx, e.into());
								return;
							}
						};


					let now = std::time::Instant::now();
					if let Err(e) = ensure_signed_phase(&api, hash).await {
						log::debug!(
							target: LOG_TARGET,
							"ensure_signed_phase failed: {:?}; skipping block: {}",
							e,
							at.number
						);
						kill_main_task_if_critical_err(&tx, e);
						return;
					}


					if let Err(e) = ensure_no_previous_solution(&api, hash, signer.account_id()).await {
						log::debug!(
							target: LOG_TARGET,
							"ensure_no_previous_solution failed: {:?}; skipping block: {}",
							e,
							at.number
						);

						kill_main_task_if_critical_err(&tx, e);
						return;
					}


					if let Err(e) = ensure_no_better_solution(&api, hash, score, config.submission_strategy).await {
						log::debug!(
							target: LOG_TARGET,
							"ensure_no_better_solution failed: {:?}; skipping block: {}",
							e,
							at.number
						);
						kill_main_task_if_critical_err(&tx, e);
						return;
					}

					let mut status_sub = match xt.sign_and_submit_then_watch_default(&*signer).await {
						Ok(sub) => sub,
						Err(e) => {
							log::warn!(target: LOG_TARGET, "submit solution failed: {:?}", e);
							kill_main_task_if_critical_err(&tx, e.into());
							return;
						}
					};


					log::trace!(target: LOG_TARGET, "Solution validity verification took: {} ms", now.elapsed().as_millis());

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
								kill_main_task_if_critical_err(&tx, err.into());
								break;
							},
							None => {
								log::error!(target: LOG_TARGET, "watch submit extrinsic at {:?} closed", hash);
								break;
							},
						};

						match status {
							TransactionStatus::Ready
								| TransactionStatus::Broadcast(_)
								| TransactionStatus::Future => (),
							TransactionStatus::InBlock(details) => {

								let events = details.fetch_events().await.expect("events should exist");

								let solution_stored = events.find_first::<$crate::chain::$runtime::epm::events::SolutionStored>();

								if let Ok(Some(_)) = solution_stored {
									log::info!(target: LOG_TARGET, "Included at {:?}", details.block_hash());
								} else {
									log::warn!(target: LOG_TARGET, "no SolutionStored event emitted");
									break
								}
								break
							},
							TransactionStatus::Retracted(hash) => {
								log::info!(target: LOG_TARGET, "Retracted at {:?}", hash);
							},
							TransactionStatus::Finalized(details) => {
								log::info!(target: LOG_TARGET, "Finalized at {:?}", details.block_hash());
								break;
							}
							_ => {
								log::warn!(target: LOG_TARGET, "Stopping listen due to other status {:?}", status);
								break;
							},
						}
					}
				}

				/// Ensure that now is the signed phase.
				async fn ensure_signed_phase(api: &$crate::chain::$runtime::RuntimeApi, hash: Hash) -> Result<(), Error> {
					use pallet_election_provider_multi_phase::Phase;

					match api.storage().election_provider_multi_phase().current_phase(Some(hash)).await {
						Ok(Phase::Signed) => Ok(()),
						Ok(_phase) => Err(Error::IncorrectPhase),
						Err(e) => Err(e.into()),
					}
				}

				/// Ensure that our current `us` have not submitted anything previously.
				async fn ensure_no_previous_solution(
					api: &$crate::chain::$runtime::RuntimeApi,
					at: Hash,
					us: &AccountId,
				) -> Result<(), Error> {
					let indices = api
						.storage()
						.election_provider_multi_phase()
						.signed_submission_indices(Some(at))
						.await?;

					for (_score, idx) in indices.0 {
						let submission = api
							.storage()
							.election_provider_multi_phase()
							.signed_submissions_map(&idx, Some(at))
							.await?;

						if let Some(submission) = submission {
							if &submission.who == us {
								return Err(Error::AlreadySubmitted);
							}
						}
					}

					Ok(())
				}

				async fn ensure_no_better_solution(
					api: &$crate::chain::$runtime::RuntimeApi,
					at: Hash,
					score: sp_npos_elections::ElectionScore,
					strategy: SubmissionStrategy,
				) -> Result<(), Error> {
					let epsilon = match strategy {
						// don't care about current scores.
						SubmissionStrategy::Always => return Ok(()),
						SubmissionStrategy::IfLeading => Perbill::zero(),
						SubmissionStrategy::ClaimBetterThan(epsilon) => epsilon,
					};

					let indices = api
						.storage()
						.election_provider_multi_phase()
						.signed_submission_indices(Some(at))
						.await?;

					log::debug!(target: LOG_TARGET, "submitted solutions: {:?}", indices.0);

					for (other_score, _) in indices.0 {
						if !score.strict_threshold_better(other_score, epsilon) {
							return Err(Error::BetterScoreExist);
						}
					}

					Ok(())

				}
			}
		}
	}
}

monitor_cmd_for!(polkadot);
monitor_cmd_for!(kusama);
monitor_cmd_for!(westend);

fn kill_main_task_if_critical_err(tx: &tokio::sync::mpsc::UnboundedSender<Error>, err: Error) {
	use jsonrpsee::{core::Error as RpcError, types::error::CallError};

	match err {
		Error::Subxt(SubxtError::Rpc(RpcError::Call(CallError::Custom(e)))) => {
			const BAD_EXTRINSIC_FORMAT: i32 = 1001;
			const VERIFICATION_ERROR: i32 = 1002;

			// Check if the transaction gets fatal errors from `author` RPC.
			// It's possible to get other errors such as outdated nonce and similar
			// but then it should be possible to try again in the next block or round.
			if e.code() == BAD_EXTRINSIC_FORMAT || e.code() == VERIFICATION_ERROR {
				let _ =
					tx.send(Error::Subxt(SubxtError::Rpc(RpcError::Call(CallError::Custom(e)))));
			}
		},
		Error::Subxt(SubxtError::Rpc(RpcError::RequestTimeout)) |
		Error::Subxt(SubxtError::Rpc(RpcError::Call(CallError::Failed(_)))) => (),
		// Regard the rest of subxt errors has fatal (including rpc)
		Error::Subxt(e) => {
			let _ = tx.send(Error::Subxt(e));
		},
		_ => (),
	}
}
