/// The header type. We re-export it here, but we can easily get it from block as well.
pub type Hash = subxt::utils::H256;
/// Default URI to connect to.
///
/// This will never work on a remote node, so we might as well try a local node.  
pub const DEFAULT_URI: &str = "ws://127.0.0.1:9944";
/// Default port to start the prometheus server on.
pub const DEFAULT_PROMETHEUS_PORT: u16 = 9999;
/// The logging target.
pub const LOG_TARGET: &str = "polkadot-staking-miner";

/// Subxt client used by the staking miner on all chains.
pub type ChainClient = subxt::OnlineClient<subxt::PolkadotConfig>;
/// Config used by the staking-miner
pub type Config = subxt::PolkadotConfig;
/// Shared client.
pub static SHARED_CLIENT: once_cell::sync::OnceCell<crate::client::Client> =
	once_cell::sync::OnceCell::new();
/// The account id type.
pub type AccountId = polkadot_sdk::sp_runtime::AccountId32;
/// The accuracy that we use for election computations.
pub type Accuracy = polkadot_sdk::sp_runtime::Perbill;
/// Storage type.
pub type Storage = subxt::storage::Storage<Config, ChainClient>;
