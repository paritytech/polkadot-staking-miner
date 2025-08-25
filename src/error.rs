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

#[derive(thiserror::Error, Debug)]
pub enum Error {
	#[error("Failed to parse log directive: `{0}´")]
	LogParse(#[from] tracing_subscriber::filter::ParseError),
	#[error("I/O error: `{0}`")]
	Io(#[from] std::io::Error),
	#[error("RPC error: `{0}`")]
	Rpc(#[from] jsonrpsee::core::ClientError),
	#[error("subxt error: `{0}`")]
	Subxt(#[from] Box<subxt::Error>),
	#[error("Crypto error: `{0:?}`")]
	Crypto(polkadot_sdk::sp_core::crypto::SecretStringError),
	#[error("Codec error: `{0}`")]
	Codec(#[from] codec::Error),
	#[error("The account does not exist")]
	AccountDoesNotExists,

	#[error(
		"Invalid chain: `{0}`, staking-miner supports only polkadot, kusama, westend, node and asset-hub-next"
	)]
	InvalidChain(String),
	#[error("Other error: `{0}`")]
	Other(String),
	#[error("Invalid metadata: {0}")]
	InvalidMetadata(String),
	#[error("Dynamic transaction error: {0}")]
	DynamicTransaction(String),
	#[error("Feasibility error: {0}")]
	Feasibility(String),
	#[error("{0}")]
	Join(#[from] tokio::task::JoinError),
	#[error("Empty snapshot")]
	EmptySnapshot,
	#[error("Missing event for transaction: {0}")]
	MissingTxEvent(String),
	#[error("Failed to submit {0} pages")]
	FailedToSubmitPages(usize),
	#[error(
		"Signed phase has insufficient blocks remaining: {blocks_remaining} (need at least {min_blocks})"
	)]
	InsufficientSignedPhaseBlocks { blocks_remaining: u32, min_blocks: u32 },
	#[error("Phase changed from Signed to {new_phase:?} during submission for round {round}")]
	PhaseChangedDuringSubmission { new_phase: String, round: u32 },
	#[error(
		"Solution validation failed: desired_targets ({desired_targets}) != solution winner count ({solution_winner_count})"
	)]
	SolutionValidation { desired_targets: u32, solution_winner_count: u32 },
	#[error("Wrong page count: solution has {solution_pages} pages but maximum is {max_pages}")]
	WrongPageCount { solution_pages: u32, max_pages: u32 },
	#[error(
		"Wrong round: solution is for round {solution_round} but current round is {current_round}"
	)]
	WrongRound { solution_round: u32, current_round: u32 },
	#[error(
		"Transaction finalization timed out after {timeout_secs} seconds for operation: {operation}"
	)]
	TxFinalizationTimeout { operation: String, timeout_secs: u64 },
}

impl From<subxt_rpcs::Error> for Error {
	fn from(e: subxt_rpcs::Error) -> Self {
		Self::Subxt(Box::new(subxt::Error::Rpc(e.into())))
	}
}

impl From<subxt::Error> for Error {
	fn from(e: subxt::Error) -> Self {
		Self::Subxt(Box::new(e))
	}
}

impl From<subxt::backend::legacy::rpc_methods::DryRunDecodeError> for Error {
	fn from(_e: subxt::backend::legacy::rpc_methods::DryRunDecodeError) -> Self {
		Self::Other("Failed to decode dry run result".to_string())
	}
}
