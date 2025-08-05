use crate::{
	client::Client,
	commands::types::MultiBlockMonitorConfig,
	error::Error,
	prelude::{AccountId, LOG_TARGET, Storage},
	runtime::multi_block::{
		self as runtime, runtime_types::pallet_election_provider_multi_block::types::Phase,
	},
	utils,
};
use polkadot_sdk::pallet_election_provider_multi_block::unsigned::miner::MinerConfig;

/// Timeout in seconds for detecting stalled block subscriptions.
/// If no blocks are received within this duration, the subscription will be recreated.
const BLOCK_SUBSCRIPTION_TIMEOUT_SECS: u64 = 60;

/// Get block state with better error handling for storage queries
async fn get_block_state(
	client: &Client,
	block_hash: polkadot_sdk::sp_core::H256,
) -> Result<(Storage, Phase, u32), Error> {
	let storage = utils::storage_at(Some(block_hash), client.chain_api()).await?;
	let phase = storage
		.fetch_or_default(&runtime::storage().multi_block_election().current_phase())
		.await?;
	let current_round = storage
		.fetch_or_default(&runtime::storage().multi_block_election().round())
		.await?;
	Ok((storage, phase, current_round))
}

/// DUMMY MINER - LISTENER ONLY VERSION
///
/// This is a simplified version that only runs the listener task.
/// It subscribes to finalized blocks and prints block number and phase information.
/// No miner, janitor, or updater tasks are spawned.
pub async fn monitor_cmd<T>(client: Client, _config: MultiBlockMonitorConfig) -> Result<(), Error>
where
	T: MinerConfig<AccountId = AccountId> + Send + Sync + 'static,
	T::Solution: Send + Sync + 'static,
	T::Pages: Send + Sync + 'static,
	T::TargetSnapshotPerBlock: Send + Sync + 'static,
	T::VoterSnapshotPerBlock: Send + Sync + 'static,
	T::MaxVotesPerVoter: Send + Sync + 'static,
{
	log::info!(target: LOG_TARGET, "Starting DUMMY MINER - Listener Only Mode");

	// Run the dummy listener directly
	dummy_listener_task(client).await
}

/// Dummy listener task that only subscribes to blocks and prints info
///
/// This simplified version:
/// - Subscribes to finalized block updates
/// - Fetches phase and round information
/// - Prints block number and phase
/// - No miner or janitor communication
async fn dummy_listener_task(client: Client) -> Result<(), Error> {
	let mut subscription = client.chain_api().blocks().subscribe_finalized().await?;

	log::info!(target: LOG_TARGET, "Dummy listener started, watching for finalized blocks");

	loop {
		let (at, block_hash) = match tokio::time::timeout(
			std::time::Duration::from_secs(BLOCK_SUBSCRIPTION_TIMEOUT_SECS),
			subscription.next(),
		)
		.await
		{
			Ok(maybe_block) => match maybe_block {
				Some(Ok(block)) => (block.header().clone(), block.hash()),
				Some(Err(e)) => {
					if e.is_disconnected_will_reconnect() {
						log::warn!(target: LOG_TARGET, "RPC connection lost, but will reconnect automatically. Continuing...");
						continue;
					}
					log::error!(target: LOG_TARGET, "subscription failed: {e:?}");
					return Err(e.into());
				},
				None => {
					log::error!(target: LOG_TARGET, "Subscription to finalized blocks terminated unexpectedly");
					return Err(Error::Other("Subscription terminated unexpectedly".to_string()));
				},
			},
			Err(_) => {
				log::warn!(target: LOG_TARGET, "No blocks received for {BLOCK_SUBSCRIPTION_TIMEOUT_SECS} seconds - subscription may be stalled, recreating subscription...");
				match client.chain_api().blocks().subscribe_finalized().await {
					Ok(new_subscription) => {
						subscription = new_subscription;
						log::info!(target: LOG_TARGET, "Successfully recreated finalized block subscription");
						continue;
					},
					Err(e) => {
						log::error!(target: LOG_TARGET, "Failed to recreate subscription: {e:?}");
						return Err(e.into());
					},
				}
			},
		};

		// Get block state
		let (_storage, phase, current_round) = get_block_state(&client, block_hash).await?;
		let block_number = at.number;

		// Print the block information
		log::info!(
			target: LOG_TARGET,
			"Received Block #{block_number} - Phase {phase:?} - Round {current_round}"
		);
	}
}
