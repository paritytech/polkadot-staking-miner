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
    commands::{Listen, SubmissionStrategy},
    error::Error,
    prelude::{
        runtime, AccountInfo, ChainClient, Config, Hash, Header, RpcClient, Storage, LOG_TARGET,
    },
};
use codec::Decode;
use jsonrpsee::core::ClientError as JsonRpseeError;
use pin_project_lite::pin_project;
use polkadot_sdk::{frame_support::weights::Weight, sp_npos_elections, sp_runtime::Perbill};
use serde::Deserialize;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, Instant},
};
use subxt::{
    backend::rpc::RpcSubscription,
    error::{Error as SubxtError, RpcError},
    tx::{TxInBlock, TxProgress},
};

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

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        let this = self.project();
        let start = this.start.get_or_insert_with(Instant::now);

        match this.inner.poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(v) => {
                let elapsed = start.elapsed();
                Poll::Ready((v, elapsed))
            }
        }
    }
}

pub trait TimedFuture: Sized + Future {
    fn timed(self) -> Timed<Self> {
        Timed {
            inner: self,
            start: None,
        }
    }
}

impl<F: Future> TimedFuture for F {}

/// Custom `RuntimeDispatchInfo` type definition similar to
/// what is in substrate but only tries to decode the `weight` field.
///
/// All other fields are not used by the staking miner.
#[derive(Decode, Default, Debug, Deserialize)]
pub struct RuntimeDispatchInfo {
    /// Weight of this dispatch.
    pub weight: Weight,
}

pub fn kill_main_task_if_critical_err(tx: &tokio::sync::mpsc::UnboundedSender<Error>, err: Error) {
    match err {
        Error::AlreadySubmitted
        | Error::BetterScoreExist
        | Error::IncorrectPhase
        | Error::TransactionRejected(_)
        | Error::Join(_)
        | Error::Feasibility(_)
        | Error::EmptySnapshot => {}
        Error::Subxt(SubxtError::Rpc(rpc_err)) => {
            log::debug!(target: LOG_TARGET, "rpc error: {:?}", rpc_err);

            match rpc_err {
                RpcError::ClientError(e) => {
                    let jsonrpsee_err = match e.downcast::<JsonRpseeError>() {
                        Ok(e) => *e,
                        Err(_) => {
                            let _ = tx.send(Error::Other(
                                "Failed to downcast RPC error; this is a bug please file an issue"
                                    .to_string(),
                            ));
                            return;
                        }
                    };

                    match jsonrpsee_err {
                        JsonRpseeError::Call(e) => {
                            const BAD_EXTRINSIC_FORMAT: i32 = 1001;
                            const VERIFICATION_ERROR: i32 = 1002;
                            use jsonrpsee::types::error::ErrorCode;

                            // Check if the transaction gets fatal errors from the `author` RPC.
                            // It's possible to get other errors such as outdated nonce and similar
                            // but then it should be possible to try again in the next block or round.
                            if e.code() == BAD_EXTRINSIC_FORMAT
                                || e.code() == VERIFICATION_ERROR
                                || e.code() == ErrorCode::MethodNotFound.code()
                            {
                                let _ = tx.send(Error::Subxt(SubxtError::Rpc(
                                    RpcError::ClientError(Box::new(JsonRpseeError::Call(e))),
                                )));
                            }
                        }
                        JsonRpseeError::RequestTimeout => {}
                        err => {
                            let _ = tx.send(Error::Subxt(SubxtError::Rpc(RpcError::ClientError(
                                Box::new(err),
                            ))));
                        }
                    }
                }
                RpcError::SubscriptionDropped => (),
                _ => (),
            }
        }
        err => {
            let _ = tx.send(err);
        }
    }
}

/// Helper to get storage at block.
pub async fn storage_at(block: Option<Hash>, api: &ChainClient) -> Result<Storage, Error> {
    if let Some(block_hash) = block {
        Ok(api.storage().at(block_hash))
    } else {
        api.storage().at_latest().await.map_err(Into::into)
    }
}

pub async fn storage_at_head(api: &Client, listen: Listen) -> Result<Storage, Error> {
    let hash = rpc_get_latest_head(api.rpc(), listen).await?;
    storage_at(Some(hash), api.chain_api()).await
}

/// Wait for the transaction to be in a block.
///
/// **Note:** transaction statuses like `Invalid`/`Usurped`/`Dropped` indicate with some
/// probability that the transaction will not make it into a block but there is no guarantee
/// that this is true. In those cases the stream is closed however, so you currently have no way to find
/// out if they finally made it into a block or not.
pub async fn wait_for_in_block<T, C>(
    mut tx: TxProgress<T, C>,
) -> Result<TxInBlock<T, C>, subxt::Error>
where
    T: subxt::Config + Clone,
    C: subxt::client::OnlineClientT<T> + std::fmt::Debug + Clone,
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

pub async fn rpc_block_subscription(
    rpc: &RpcClient,
    listen: Listen,
) -> Result<RpcSubscription<Header>, Error> {
    match listen {
        Listen::Head => rpc.chain_subscribe_new_heads().await,
        Listen::Finalized => rpc.chain_subscribe_finalized_heads().await,
    }
    .map_err(Into::into)
}

pub async fn rpc_get_latest_head(rpc: &RpcClient, listen: Listen) -> Result<Hash, Error> {
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
pub fn score_passes_strategy(
    our_score: sp_npos_elections::ElectionScore,
    best_score: sp_npos_elections::ElectionScore,
    strategy: SubmissionStrategy,
) -> bool {
    match strategy {
        SubmissionStrategy::Always => true,
        SubmissionStrategy::IfLeading => {
            our_score.strict_threshold_better(best_score, Perbill::zero())
        }
        SubmissionStrategy::ClaimBetterThan(epsilon) => {
            our_score.strict_threshold_better(best_score, epsilon)
        }
        SubmissionStrategy::ClaimNoWorseThan(epsilon) => {
            !best_score.strict_threshold_better(our_score, epsilon)
        }
    }
}

/// Get the account data of the given `account_id` from the storage
/// which can be used to get free balance, reserved balance, etc.
pub async fn account_info(
    storage: &Storage,
    who: &subxt::config::substrate::AccountId32,
) -> Result<AccountInfo, Error> {
    storage
        .fetch(&runtime::storage().system().account(who))
        .await?
        .ok_or(Error::AccountDoesNotExists)
}

/// Wait for the transaction to be included in a block according to the listen strategy
/// which can be either `Listen::Finalized` or `Listen::Head`.
pub async fn wait_tx_in_block_for_strategy(
    tx: TxProgress<Config, ChainClient>,
    listen: Listen,
) -> Result<TxInBlock<Config, ChainClient>, Error> {
    match listen {
        Listen::Finalized => tx.wait_for_finalized().await.map_err(Into::into),
        Listen::Head => wait_for_in_block(tx).await.map_err(Into::into),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::Accuracy;
    use std::str::FromStr;

    #[test]
    fn score_passes_strategy_works() {
        let s = |x| sp_npos_elections::ElectionScore {
            minimal_stake: x,
            ..Default::default()
        };
        let two = Perbill::from_percent(2);

        // anything passes Always
        assert!(score_passes_strategy(
            s(0),
            s(0),
            SubmissionStrategy::Always
        ));
        assert!(score_passes_strategy(
            s(5),
            s(0),
            SubmissionStrategy::Always
        ));
        assert!(score_passes_strategy(
            s(5),
            s(10),
            SubmissionStrategy::Always
        ));

        // if leading
        assert!(!score_passes_strategy(
            s(0),
            s(0),
            SubmissionStrategy::IfLeading
        ));
        assert!(score_passes_strategy(
            s(1),
            s(0),
            SubmissionStrategy::IfLeading
        ));
        assert!(score_passes_strategy(
            s(2),
            s(0),
            SubmissionStrategy::IfLeading
        ));
        assert!(!score_passes_strategy(
            s(5),
            s(10),
            SubmissionStrategy::IfLeading
        ));
        assert!(!score_passes_strategy(
            s(9),
            s(10),
            SubmissionStrategy::IfLeading
        ));
        assert!(!score_passes_strategy(
            s(10),
            s(10),
            SubmissionStrategy::IfLeading
        ));

        // if better by 2%
        assert!(!score_passes_strategy(
            s(50),
            s(100),
            SubmissionStrategy::ClaimBetterThan(two)
        ));
        assert!(!score_passes_strategy(
            s(100),
            s(100),
            SubmissionStrategy::ClaimBetterThan(two)
        ));
        assert!(!score_passes_strategy(
            s(101),
            s(100),
            SubmissionStrategy::ClaimBetterThan(two)
        ));
        assert!(!score_passes_strategy(
            s(102),
            s(100),
            SubmissionStrategy::ClaimBetterThan(two)
        ));
        assert!(score_passes_strategy(
            s(103),
            s(100),
            SubmissionStrategy::ClaimBetterThan(two)
        ));
        assert!(score_passes_strategy(
            s(150),
            s(100),
            SubmissionStrategy::ClaimBetterThan(two)
        ));

        // if no less than 2% worse
        assert!(!score_passes_strategy(
            s(50),
            s(100),
            SubmissionStrategy::ClaimNoWorseThan(two)
        ));
        assert!(!score_passes_strategy(
            s(97),
            s(100),
            SubmissionStrategy::ClaimNoWorseThan(two)
        ));
        assert!(score_passes_strategy(
            s(98),
            s(100),
            SubmissionStrategy::ClaimNoWorseThan(two)
        ));
        assert!(score_passes_strategy(
            s(99),
            s(100),
            SubmissionStrategy::ClaimNoWorseThan(two)
        ));
        assert!(score_passes_strategy(
            s(100),
            s(100),
            SubmissionStrategy::ClaimNoWorseThan(two)
        ));
        assert!(score_passes_strategy(
            s(101),
            s(100),
            SubmissionStrategy::ClaimNoWorseThan(two)
        ));
        assert!(score_passes_strategy(
            s(102),
            s(100),
            SubmissionStrategy::ClaimNoWorseThan(two)
        ));
        assert!(score_passes_strategy(
            s(103),
            s(100),
            SubmissionStrategy::ClaimNoWorseThan(two)
        ));
        assert!(score_passes_strategy(
            s(150),
            s(100),
            SubmissionStrategy::ClaimNoWorseThan(two)
        ));
    }

    #[test]
    fn submission_strategy_from_str_works() {
        assert_eq!(
            SubmissionStrategy::from_str("if-leading"),
            Ok(SubmissionStrategy::IfLeading)
        );
        assert_eq!(
            SubmissionStrategy::from_str("always"),
            Ok(SubmissionStrategy::Always)
        );
        assert_eq!(
            SubmissionStrategy::from_str("  percent-better 99   "),
            Ok(SubmissionStrategy::ClaimBetterThan(Accuracy::from_percent(
                99
            )))
        );
    }
}
