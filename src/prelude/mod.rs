#[cfg(legacy)]
mod legacy;

#[cfg(experimental_multi_block)]
mod multi_block;

#[cfg(legacy)]
pub use legacy::*;

#[cfg(experimental_multi_block)]
pub use multi_block::*;

/// The header type. We re-export it here, but we can easily get it from block as well.
pub type Header =
	subxt::config::substrate::SubstrateHeader<u32, subxt::config::substrate::BlakeTwo256>;
/// The header type. We re-export it here, but we can easily get it from block as well.
pub type Hash = subxt::utils::H256;
/// Balance type
pub type Balance = u128;
/// Default URI to connect to.
///
/// This will never work on a remote node, so we might as well try a local node.
pub const DEFAULT_URI: &str = "ws://127.0.0.1:9944";
/// Default port to start the prometheus server on.
pub const DEFAULT_PROMETHEUS_PORT: u16 = 9999;
/// The logging target.
pub const LOG_TARGET: &str = "polkadot-staking-miner";
/// RPC client.
pub type RpcClient = subxt::backend::legacy::LegacyRpcMethods<subxt::PolkadotConfig>;
/// Subxt client used by the staking miner on all chains.
pub type ChainClient = subxt::OnlineClient<subxt::PolkadotConfig>;
/// Config used by the staking-miner
pub type Config = subxt::PolkadotConfig;
/// Shared client.
pub static SHARED_CLIENT: once_cell::sync::OnceCell<crate::client::Client> =
	once_cell::sync::OnceCell::new();
pub use polkadot_sdk::sp_runtime::traits::{Block as BlockT, Header as HeaderT};
/// The account id type.
pub type AccountId = polkadot_sdk::sp_runtime::AccountId32;
/// The key pair type being used. We "strongly" assume sr25519 for simplicity.
pub type Pair = polkadot_sdk::sp_core::sr25519::Pair;
/// The accuracy that we use for election computations.
pub type Accuracy = polkadot_sdk::sp_runtime::Perbill;
pub type SignedSubmission<S> =
	polkadot_sdk::pallet_election_provider_multi_phase::SignedSubmission<AccountId, Balance, S>;
/// Storage type.
pub type Storage = subxt::storage::Storage<Config, ChainClient>;
