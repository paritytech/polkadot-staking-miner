use crate::{prelude::*, Error, MonitorConfig, SubmissionStrategy};
use pallet_election_provider_multi_phase::RawSolution;
use sp_runtime::Perbill;
use std::sync::Arc;
use subxt::{BasicError as SubxtError, TransactionStatus};
use tokio::sync::Mutex;

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
				let round = Arc::new(Mutex::new(None));

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
							round.clone(),
					));
				}

				/// Construct extrinsic at given block and watch it.
				async fn mine_and_submit_solution(
					tx: tokio::sync::mpsc::UnboundedSender<Error>,
					at: Header,
					api: $crate::chain::$runtime::RuntimeApi,
					signer: Arc<Signer>,
					config: MonitorConfig,
					prev_round: Arc<Mutex<Option<u32>>>,
				) {
					/*async fn flatten<T>(handle: tokio::task::JoinHandle<Result<T, Error>>) -> Result<T, Error> {
					  match handle.await {
					  Ok(Ok(result)) => Ok(result),
					  Ok(Err(err)) => Err(err),
					  Err(err) => panic!("tokio spawn task failed; kill task: {:?}", err),
					  }
					  }*/

					let hash = at.hash();
					log::trace!(target: LOG_TARGET, "new event at #{:?} ({:?})", at.number, hash);


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

					// As a solution is attempted to be submitted lock reject later blocks from proceeding.
					// Until we know whether the current solution was successful or not.
					let mut round_lock = prev_round.lock().await;

					match *round_lock {
						None => (),
						Some(r) if round > r => (),
						_ => return,
					};


					let (solution, score, size) =
					match crate::helpers::[<mine_solution_$runtime>](&api, Some(hash), config.solver)
					.await {
						Ok(s) => s,
						Err(e) => {
							kill_main_task_if_critical_err(&tx, e);
							return;
						}
					};


					log::info!(target: LOG_TARGET, "mined solution with {:?} size: {:?} round: {:?}", score, size, round);

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


					// This might fail with outdated nonce let it just crash if that happens.
					let mut status_sub = match xt.sign_and_submit_then_watch_default(&*signer).await {
						Ok(sub) => sub,
						Err(e) => {
							log::warn!(target: LOG_TARGET, "submit solution failed: {:?}", e);
							kill_main_task_if_critical_err(&tx, e.into());
							return;
						}
					};


					let result = loop {
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
								break false;
							},
							None => {
								log::error!(target: LOG_TARGET, "watch submit extrinsic at {:?} closed", hash);
								break false;
							},
						};

						match status {
							TransactionStatus::Ready
								| TransactionStatus::Broadcast(_)
								| TransactionStatus::Future => (),
							TransactionStatus::InBlock(details) => {
								log::info!(target: LOG_TARGET, "included at {:?}", details.block_hash());
								let events = details.fetch_events().await.expect("events should exist");

								let solution_stored = events.find_first::<$crate::chain::$runtime::epm::events::SolutionStored>();

								if let Ok(Some(event)) = solution_stored {
									log::info!(target: LOG_TARGET, "included at {:?}", event);
								} else {
									log::warn!(target: LOG_TARGET, "no SolutionStored event emitted");
									break false;
								}
								break true
							},
							TransactionStatus::Retracted(hash) => {
								log::info!(target: LOG_TARGET, "Retracted at {:?}", hash);
							},
							TransactionStatus::Finalized(details) => {
								log::info!(target: LOG_TARGET, "Finalized at {:?}", details.block_hash());
								break true;
							}
							_ => {
								log::warn!(target: LOG_TARGET, "Stopping listen due to other status {:?}", status);
								break false;
							},
						}
					};

					if result {
						*round_lock = Some(round);
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
							log::debug!(target: LOG_TARGET, "score: {:?}, other_score: {:?}, worse at: {:?}", score, other_score, at);
							return Err(Error::BetterScoreExist);
						}
						log::debug!(target: LOG_TARGET, "score: {:?}, other_score: {:?}, better at: {:?}", score, other_score, at);
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
	use jsonrpsee::core::Error as RpcError;

	log::debug!(target: LOG_TARGET, "closing task: {:?}", err);

	match err {
		Error::Subxt(SubxtError::InvalidMetadata(e)) => {
			let _ = tx.send(Error::Subxt(SubxtError::InvalidMetadata(e)));
		},
		Error::Subxt(SubxtError::Metadata(e)) => {
			let _ = tx.send(Error::Subxt(SubxtError::Metadata(e)));
		},
		Error::Subxt(SubxtError::Rpc(RpcError::RestartNeeded(e))) => {
			let _ = tx.send(Error::Subxt(SubxtError::Rpc(RpcError::RestartNeeded(e))));
		},
		_ => (),
	}
}
