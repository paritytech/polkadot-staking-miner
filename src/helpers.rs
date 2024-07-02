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

use crate::{error::Error, prelude::*};
use codec::Decode;
use frame_support::weights::Weight;
use jsonrpsee::core::ClientError as JsonRpseeError;
use pin_project_lite::pin_project;
use serde::Deserialize;
use std::{
	future::Future,
	pin::Pin,
	task::{Context, Poll},
	time::{Duration, Instant},
};
use subxt::{
	error::{Error as SubxtError, RpcError},
	storage::Storage,
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
		Error::AlreadySubmitted |
		Error::BetterScoreExist |
		Error::IncorrectPhase |
		Error::TransactionRejected(_) |
		Error::Join(_) |
		Error::Feasibility(_) |
		Error::EmptySnapshot => {},
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
						},
					};

					match jsonrpsee_err {
						JsonRpseeError::Call(e) => {
							const BAD_EXTRINSIC_FORMAT: i32 = 1001;
							const VERIFICATION_ERROR: i32 = 1002;
							use jsonrpsee::types::error::ErrorCode;

							// Check if the transaction gets fatal errors from the `author` RPC.
							// It's possible to get other errors such as outdated nonce and similar
							// but then it should be possible to try again in the next block or round.
							if e.code() == BAD_EXTRINSIC_FORMAT ||
								e.code() == VERIFICATION_ERROR || e.code() ==
								ErrorCode::MethodNotFound.code()
							{
								let _ = tx.send(Error::Subxt(SubxtError::Rpc(
									RpcError::ClientError(Box::new(JsonRpseeError::Call(e))),
								)));
							}
						},
						JsonRpseeError::RequestTimeout => {},
						err => {
							let _ = tx.send(Error::Subxt(SubxtError::Rpc(RpcError::ClientError(
								Box::new(err),
							))));
						},
					}
				},
				RpcError::SubscriptionDropped => (),
				_ => (),
			}
		},
		err => {
			let _ = tx.send(err);
		},
	}
}

/// Helper to get storage at block.
pub async fn storage_at(
	block: Option<Hash>,
	api: &ChainClient,
) -> Result<Storage<Config, ChainClient>, Error> {
	if let Some(block_hash) = block {
		Ok(api.storage().at(block_hash))
	} else {
		api.storage().at_latest().await.map_err(Into::into)
	}
}
