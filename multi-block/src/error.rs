use crate::prelude::*;

#[derive(thiserror::Error, Debug)]
pub enum Error {
	#[error("Failed to parse log directive: `{0}´")]
	LogParse(#[from] tracing_subscriber::filter::ParseError),
	#[error("I/O error: `{0}`")]
	Io(#[from] std::io::Error),
	#[error("subxt error: `{0}`")]
	Subxt(#[from] subxt::Error),
	#[error("Invalid chain: `{0}`, staking-miner supports only polkadot, kusama and westend")]
	InvalidChain(String),
	#[error("Invalid metadata: {0}")]
	InvalidMetadata(String),
	#[error("The account does not exist")]
	AccountDoesNotExists,
	#[error("Crypto error: `{0:?}`")]
	Crypto(sp_core::crypto::SecretStringError),
	#[error("Other error: `{0}`")]
	Other(String),
}
