use crate::{
	error::Error,
	helpers::{self, TimedFuture},
	opt::{Listen, MonitorConfig, SubmissionStrategy},
	prelude::*,
	prometheus,
	signer::Signer,
};
use codec::Encode;
use frame_election_provider_support::NposSolution;
use pallet_election_provider_multi_phase::{RawSolution, SolutionOf};
use sp_runtime::Perbill;
use std::sync::Arc;
use subxt::{rpc::Subscription, tx::TxStatus, Error as SubxtError};
use tokio::sync::Mutex;

macro_rules! monitor_cmd_for {
	($runtime:tt) => {
		paste::paste! {
			/// The monitor command.
			pub async fn [<run_$runtime>] (api: SubxtClient, config: MonitorConfig) -> Result<(), Error> {
				use crate::chain::[<$runtime>]::{self as chain, runtime};

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
					tokio::spawn(mine_and_submit_solution(
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

				/// Construct extrinsic at given block and watch it.
				async fn mine_and_submit_solution(
					tx: tokio::sync::mpsc::UnboundedSender<Error>,
					at: Header,
					api: SubxtClient,
					mut signer: Signer,
					config: MonitorConfig,
					submit_lock: Arc<Mutex<()>>,
				) {

					let hash = at.hash();
					log::trace!(target: LOG_TARGET, "new event at #{:?} ({:?})", at.number, hash);

					// NOTE: as we try to send at each block then the nonce is used guard against
					// submitting twice. Because once a solution has been accepted on chain
					// the "next transaction" at a later block but with the same nonce will be rejected
					let nonce = match api.rpc().system_account_next_index(signer.account_id()).await {
						Ok(none) => none,
						Err(e) => {
							kill_main_task_if_critical_err(&tx, e.into());
							return;
						}
 					};
					signer.set_nonce(nonce);

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

					let round = match api.storage().fetch(&runtime::storage().election_provider_multi_phase().round(), Some(hash)).await {
						Ok(Some(round)) => round,
						// Default round is 1
						// https://github.com/paritytech/substrate/blob/49b06901eb65f2c61ff0934d66987fd955d5b8f5/frame/election-provider-multi-phase/src/lib.rs#L1188
						Ok(None) => 1,
						Err(e) => {
							log::error!(target: LOG_TARGET, "Mining solution failed: {:?}", e);
							kill_main_task_if_critical_err(&tx, e.into());
							return;
						},
					};

					let _lock = submit_lock.lock().await;

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

					let (solution, score) =
					match helpers::[<mine_solution_$runtime>](&api, Some(hash), config.solver).timed().await {
						(Ok((solution, score, size)), elapsed) => {
							let elapsed_ms = elapsed.as_millis();

							let encoded_len = solution.encoded_size();
							let final_weight = chain::MinerConfig::solution_weight(
								size.voters,
								size.targets,
								solution.voter_count() as u32,
								// this is same as desired_targets
								solution.unique_targets().len() as u32,
							);

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
						}
						(Err(e), _) => {
							kill_main_task_if_critical_err(&tx, e);
							return;
						}
					};

					let best_head = match get_latest_head(&api, config.listen).await {
						Ok(head) => head,
						Err(e) => {
							kill_main_task_if_critical_err(&tx, e);
							return;
						}
					};

					if let Err(e) = ensure_signed_phase(&api, best_head).await {
						log::debug!(
							target: LOG_TARGET,
							"ensure_signed_phase failed: {:?}; skipping block: {}",
							e,
							at.number
						);
						kill_main_task_if_critical_err(&tx, e);
						return;
					}

					match ensure_no_better_solution(&api, best_head, score, config.submission_strategy).timed().await {
						(Ok(_), now) => {
							log::trace!(target: LOG_TARGET, "Solution validity verification took: {} ms", now.as_millis());
						}
						(Err(e), _) => {
							log::debug!(
								target: LOG_TARGET,
								"ensure_no_better_solution failed: {:?}; skipping block: {}",
								e,
								at.number
							);
							kill_main_task_if_critical_err(&tx, e);
							return;
						}
					};

					prometheus::on_submission_attempt();
					match submit_and_watch_solution(&api, signer, (solution, score, round), hash).timed().await {
						(Ok(_), now) => {
							prometheus::on_submission_success();
							prometheus::observe_submit_and_watch_duration(now.as_millis() as f64);
						}
						(Err(e), _) => {
							log::warn!(
								target: LOG_TARGET,
								"submit_and_watch_solution failed: {:?}; skipping block: {}",
								e,
								at.number
							);
							kill_main_task_if_critical_err(&tx, e);
						}
					};
				}

				/// Ensure that now is the signed phase.
				async fn ensure_signed_phase(api: &SubxtClient, hash: Hash) -> Result<(), Error> {
					use pallet_election_provider_multi_phase::Phase;

					let addr = chain::runtime::storage().election_provider_multi_phase().current_phase();
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
				async fn ensure_no_previous_solution(
					api: &SubxtClient,
					at: Hash,
					us: &AccountId,
				) -> Result<(), Error> {

					let addr = chain::runtime::storage().election_provider_multi_phase().signed_submission_indices();
					let indices = api
						.storage()
						.fetch_or_default(&addr, Some(at))
						.await?;

					for (_score, idx) in indices.0 {
						let addr = runtime::storage().election_provider_multi_phase().signed_submissions_map(&idx);

						let submission = api
							.storage()
							.fetch(&addr, Some(at))
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
					api: &SubxtClient,
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

					let addr = runtime::storage().election_provider_multi_phase().signed_submission_indices();
					let indices = api
						.storage()
						.fetch_or_default(&addr, Some(at))
						.await?;

					log::debug!(target: LOG_TARGET, "submitted solutions: {:?}", indices.0);

					for (other_score, _) in indices.0 {
						if !score.strict_threshold_better(other_score, epsilon) {
							return Err(Error::BetterScoreExist);
						}
					}

					Ok(())
				}

				async fn submit_and_watch_solution(
					api: &SubxtClient,
					signer: Signer,
					(solution, score, round): (SolutionOf<chain::MinerConfig>, sp_npos_elections::ElectionScore, u32),
					hash: Hash
				) -> Result<(), Error> {

					let tx = runtime::tx().election_provider_multi_phase().submit(RawSolution { solution, score, round });

					let mut status_sub = api.tx().sign_and_submit_then_watch_default(&tx, &*signer).await.map_err(|e| {
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
								return Err(err.into());
							},
							None => {
								return Err(Error::Other(format!("Submit solution failed; watch_extrinsic at {:?} closed", hash)));
							},
						};

						match status {
							TxStatus::Ready | TxStatus::Broadcast(_) | TxStatus::Future => (),
							TxStatus::InBlock(details) => {

								let events = details.fetch_events().await.expect("events should exist");

								let solution_stored = events.find_first::<chain::epm::events::SolutionStored>();

								return if let Ok(Some(_)) = solution_stored {
									log::info!(target: LOG_TARGET, "Included at {:?}", details.block_hash());
									Ok(())
								} else {
									Err(Error::Other(format!("No SolutionStored event found at {:?}", details.block_hash())))
								};
							},
							TxStatus::Retracted(hash) => {
								log::info!(target: LOG_TARGET, "Retracted at {:?}", hash);
							},
							TxStatus::Finalized(details) => {
								log::info!(target: LOG_TARGET, "Finalized at {:?}", details.block_hash());
								return Ok(());
							}
							_ => {
								return Err(Error::Other(format!("Stopping listen due to other status {:?}", status)));
							},
						}
					}
				}
			}
		}
	}
}

#[cfg(feature = "polkadot")]
monitor_cmd_for!(polkadot);
#[cfg(feature = "kusama")]
monitor_cmd_for!(kusama);
#[cfg(feature = "westend")]
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

async fn heads_subscription(
	api: &SubxtClient,
	listen: Listen,
) -> Result<Subscription<Header>, Error> {
	match listen {
		Listen::Head => api.rpc().subscribe_blocks().await,
		Listen::Finalized => api.rpc().subscribe_finalized_blocks().await,
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
