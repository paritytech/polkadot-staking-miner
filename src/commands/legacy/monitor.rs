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
    commands::types::{Listen, MonitorConfig, SubmissionStrategy},
    dynamic::legacy as dynamic,
    error::Error,
    prelude::{
        AccountId, ChainClient, ExtrinsicParamsBuilder, Hash, Header, LOG_TARGET, RpcClient,
    },
    prometheus,
    runtime::legacy as runtime,
    signer::Signer,
    static_types::legacy as static_types,
    utils::{
        TimedFuture, kill_main_task_if_critical_err, rpc_block_subscription, score_passes_strategy,
        wait_for_in_block,
    },
};
use codec::{Decode, Encode};
use futures::future::TryFutureExt;
use jsonrpsee::core::ClientError as JsonRpseeError;
use polkadot_sdk::{
    frame_election_provider_support::NposSolution,
    pallet_election_provider_multi_phase::{MinerConfig, RawSolution, SolutionOf},
    sp_npos_elections,
};
use std::sync::Arc;
use subxt::{backend::legacy::rpc_methods::DryRunResult, config::Header as _};
use tokio::sync::Mutex;

pub async fn monitor_cmd<T>(client: Client, config: MonitorConfig) -> Result<(), Error>
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

    let mut subscription = rpc_block_subscription(client.rpc(), config.listen).await?;
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
                        subscription = rpc_block_subscription(client.rpc(), config.listen).await?;
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
            if let Err(err) =
                mine_and_submit_solution::<T>(at, client2, signer2, config2, submit_lock2).await
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
) -> Result<(), Error>
where
    T: MinerConfig<AccountId = AccountId, MaxVotesPerVoter = static_types::MaxVotesPerVoter>
        + Send
        + Sync
        + 'static,
    T::Solution: Send,
{
    let block_hash = at.hash();
    log::trace!(target: LOG_TARGET, "new event at #{:?} ({:?})", at.number(), block_hash);

    // NOTE: as we try to send at each block then the nonce is used guard against
    // submitting twice. Because once a solution has been accepted on chain
    // the "next transaction" at a later block but with the same nonce will be rejected
    let nonce = client
        .rpc()
        .system_account_next_index(signer.account_id())
        .await?;

    ensure_signed_phase(client.chain_api(), block_hash)
        .inspect_err(|e| {
            log::debug!(
                target: LOG_TARGET,
                "ensure_signed_phase failed: {:?}; skipping block: {}",
                e,
                at.number()
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
            at.number()
        )
    })
    .await?;

    tokio::time::sleep(std::time::Duration::from_secs(config.delay as u64)).await;
    let _lock = submit_lock.lock().await;

    let (solution, score) = match dynamic::fetch_snapshot_and_mine_solution::<T>(
        client.chain_api(),
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
        }
        (Err(e), _) => return Err(Error::Other(e.to_string())),
    };

    let best_head = get_latest_head(client.rpc(), config.listen).await?;

    ensure_signed_phase(client.chain_api(), best_head)
        .inspect_err(|e| {
            log::debug!(
                target: LOG_TARGET,
                "ensure_signed_phase failed: {:?}; skipping block: {}",
                e,
                at.number()
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
        }
        (Err(e), _) => {
            log::debug!(
                target: LOG_TARGET,
                "ensure_no_better_solution failed: {:?}; skipping block: {}",
                e,
                at.number()
            );
            return Err(e);
        }
    };

    prometheus::on_submission_attempt();
    match submit_and_watch_solution::<T>(
        &client,
        signer,
        (solution, score, round),
        nonce,
        config.listen,
        config.dry_run,
    )
    .timed()
    .await
    {
        (Ok(_), now) => {
            prometheus::on_submission_success();
            prometheus::observe_submit_and_watch_duration(now.as_millis() as f64);
        }
        (Err(e), _) => {
            log::warn!(
                target: LOG_TARGET,
                "submit_and_watch_solution failed: {e}; skipping block: {}",
                at.number()
            );
        }
    };
    Ok(())
}

/// Ensure that now is the signed phase.
async fn ensure_signed_phase(api: &ChainClient, block_hash: Hash) -> Result<(), Error> {
    use polkadot_sdk::pallet_election_provider_multi_phase::Phase;

    let addr = runtime::storage()
        .election_provider_multi_phase()
        .current_phase();
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
    let addr = runtime::storage()
        .election_provider_multi_phase()
        .signed_submission_indices();
    let indices = api.storage().at(block_hash).fetch_or_default(&addr).await?;

    for (_score, _, idx) in indices.0 {
        let submission = dynamic::signed_submission_at::<T>(idx, Some(block_hash), api).await?;

        if let Some(submission) = submission {
            if &submission.who == us {
                return Err(Error::AlreadySubmitted);
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
        return Ok(());
    }

    let addr = runtime::storage()
        .election_provider_multi_phase()
        .signed_submission_indices();
    let indices = api.storage().at(block_hash).fetch_or_default(&addr).await?;

    log::debug!(target: LOG_TARGET, "submitted solutions: {:?}", indices.0);

    if indices.0.last().map_or(true, |(best_score, _, _)| {
        score_passes_strategy(score, best_score.0, strategy)
    }) {
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
) -> Result<(), Error> {
    let tx = dynamic::signed_solution(RawSolution {
        solution,
        score,
        round,
    })?;

    // By default we are using the epoch length as the mortality length.
    // If that doesn't is available then we just use the default mortality provided by subxt.
    //
    // TODO: https://github.com/paritytech/polkadot-staking-miner/issues/730
    //
    // The extrinsic mortality length is static and it doesn't know when the signed phase ends.
    let xt_cfg = if let Ok(len) = client
        .chain_api()
        .constants()
        .at(&runtime::constants().babe().epoch_duration())
    {
        ExtrinsicParamsBuilder::default()
            .nonce(nonce)
            .mortal(len)
            .build()
    } else {
        ExtrinsicParamsBuilder::default().nonce(nonce).build()
    };

    let xt = client
        .chain_api()
        .tx()
        .create_signed(&tx, &*signer, xt_cfg)
        .await?;

    if dry_run {
        let dry_run_bytes = client.rpc().dry_run(xt.encoded(), None).await?;

        match dry_run_bytes.into_dry_run_result()? {
            DryRunResult::Success => (),
            DryRunResult::DispatchError(e) => {
                let dispatch_err = subxt::error::DispatchError::decode_from(
                    e.as_ref(),
                    client.chain_api().metadata(),
                )?;
                return Err(Error::TransactionRejected(dispatch_err.to_string()));
            }
            DryRunResult::TransactionValidityError => {
                return Err(Error::TransactionRejected(
                    "TransactionValidityError".into(),
                ));
            }
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
                )));
            }
        }
        Listen::Finalized => {
            let finalized_block = tx_progress.wait_for_finalized().await?;
            let block_hash = finalized_block.block_hash();
            let finalized_success = finalized_block.wait_for_success().await?;

            let solution_stored = finalized_success
                .find_first::<runtime::election_provider_multi_phase::events::SolutionStored>(
            );

            if let Ok(Some(_)) = solution_stored {
                log::info!(target: LOG_TARGET, "Finalized at {:?}", block_hash);
            } else {
                return Err(Error::Other(format!(
                    "No SolutionStored event found at {:?}",
                    block_hash,
                )));
            }
        }
    };

    Ok(())
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

async fn dry_run_works(rpc: &RpcClient) -> Result<(), Error> {
    match rpc.dry_run(&[], None).await {
        Ok(_) => Ok(()),
        Err(e) => {
            if let subxt_rpcs::Error::Client(boxed_err) = &e {
                match boxed_err.downcast_ref::<JsonRpseeError>() {
                    Some(JsonRpseeError::Call(e)) => {
                        if e.message() == "RPC call is unsafe to be called externally" {
                            return Err(Error::Other(
                                "dry-run requires a RPC endpoint with `--rpc-methods unsafe`; \
                                either connect to another RPC endpoint or disable dry-run"
                                    .to_string(),
                            ));
                        }
                    }
                    _ => {
                        return Err(Error::Other(
                            "Failed to downcast RPC error; this is a bug please file an issue"
                                .to_string(),
                        ));
                    }
                }
            }
            Err(e.into())
        }
    }
}
