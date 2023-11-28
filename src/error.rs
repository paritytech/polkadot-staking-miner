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
	RpcError(#[from] jsonrpsee::core::Error),
	#[error("subxt error: `{0}`")]
	Subxt(#[from] subxt::Error),
	#[error("SecretUri error: `{0}`")]
	SecretUri(#[from] subxt_signer::SecretUriError),
	#[error("Keypair error: `{0}`")]
	Keypair(#[from] subxt_signer::sr25519::Error),
	#[error("Codec error: `{0}`")]
	Codec(#[from] codec::Error),
	#[error("Incorrect phase")]
	IncorrectPhase,
	#[error("Submission is already submitted")]
	AlreadySubmitted,
	#[error("The account does not exist")]
	AccountDoesNotExists,
	#[error("Submission with better score already exist")]
	BetterScoreExist,
	#[error("Invalid chain: `{0}`, staking-miner supports only polkadot, kusama and westend")]
	InvalidChain(String),
	#[error("Other error: `{0}`")]
	Other(String),
	#[error("Invalid metadata: {0}")]
	InvalidMetadata(String),
	#[error("Transaction rejected: {0}")]
	TransactionRejected(String),
	#[error("Dynamic transaction error: {0}")]
	DynamicTransaction(String),
	#[error("Feasibility error: {0}")]
	Feasibility(String),
	#[error("{0}")]
	JoinError(#[from] tokio::task::JoinError),
	#[error("Empty snapshot")]
	EmptySnapshot,
}
