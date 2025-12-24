use crate::{
	client::Client,
	commands::{
		multi_block::types::{BlockDetails, CurrentSubmission, IncompleteSubmission, Snapshot},
		types::{MultiBlockMonitorConfig, SubmissionStrategy},
	},
	dynamic::multi_block as dynamic,
	error::{ChannelFailureError, Error, TaskFailureError, TimeoutError::*},
	prelude::{AccountId, ExtrinsicParamsBuilder, LOG_TARGET, Storage},
	prometheus,
	runtime::multi_block::{
		self as runtime, runtime_types::pallet_election_provider_multi_block::types::Phase,
	},
	signer::Signer,
	static_types::multi_block as static_types,
	utils::{self, TimedFuture, score_passes_strategy},
};
use codec::Decode;
use futures::TryStreamExt;
use polkadot_sdk::{
	pallet_election_provider_multi_block::unsigned::miner::MinerConfig,
	sp_npos_elections::ElectionScore,
};
use std::collections::HashSet;

use subxt::backend::StreamOf;
use tokio::sync::mpsc;

/// Number of previous rounds to scan for old submissions during clear old rounds cleanup.
/// This provides a reasonable buffer for offline periods while avoiding inefficient
/// scanning of potentially gazillions of historical rounds.
const CLEAR_OLD_ROUNDS_SCAN_ROUNDS: u32 = 5;

/// Groups all the communication channels used by the listener task
struct TaskChannels {
	miner_tx: mpsc::Sender<MinerMessage>,
	clear_old_rounds_tx: mpsc::Sender<ClearOldRoundsMessage>,
	era_pruning_tx: mpsc::Sender<EraPruningMessage>,
}

/// Groups the mutable state tracked by the listener task
struct ListenerState {
	prev_round: Option<u32>,
	prev_phase: Option<Phase>,
	last_block_time: std::time::Instant,
	subscription_recreation_attempts: u32,
}

/// Timeout in seconds for detecting stalled block subscriptions.
/// If no blocks are received within this duration, the subscription will be recreated.
const BLOCK_SUBSCRIPTION_TIMEOUT_SECS: u64 = 60;

/// Timeout in seconds for detecting stalled block processing.
/// If block processing takes longer than this, we'll recreate the subscription.
const BLOCK_PROCESSING_TIMEOUT_SECS: u64 = 90;

/// Maximum number of consecutive subscription recreation attempts before giving up and exiting.
const MAX_SUBSCRIPTION_RECREATION_ATTEMPTS: u32 = 3;

/// Timeout in seconds for era pruning storage queries.
const ERA_PRUNING_TIMEOUT_SECS: u64 = 30;

async fn signed_phase(client: &Client) -> Result<bool, Error> {
	let storage = utils::storage_at_head(client).await?;
	let current_phase = storage
		.fetch_or_default(&runtime::storage().multi_block_election().current_phase())
		.await?;

	Ok(matches!(current_phase, Phase::Signed(_)))
}

/// Action to take after processing a listener iteration
enum ListenerAction {
	/// Continue to the next iteration
	Continue,
	/// Subscription was recreated, continue processing
	SubscriptionRecreated,
	/// Block processing timed out, subscription needs recreation
	BlockProcessingTimeout,
}

/// Type alias for the finalized block subscription stream
type SubscriptionStream = StreamOf<
	Result<
		subxt::blocks::Block<subxt::PolkadotConfig, subxt::OnlineClient<subxt::PolkadotConfig>>,
		subxt::Error,
	>,
>;

/// Process the block after subscription.next() succeeds
///
/// This function contains all the logic that happens AFTER we receive a block
/// from the subscription. This is the part that can hang internally and needs
/// to be wrapped with timeout.
async fn process_block_internal<T>(
	client: &Client,
	at: crate::prelude::Header,
	block_hash: subxt::utils::H256,
	state: &mut ListenerState,
	channels: &TaskChannels,
) -> Result<ListenerAction, Error>
where
	T: MinerConfig<AccountId = AccountId> + Send + Sync,
	T::Solution: Send + Sync,
	T::Pages: Send + Sync,
	T::TargetSnapshotPerBlock: Send + Sync,
	T::VoterSnapshotPerBlock: Send + Sync,
	T::MaxVotesPerVoter: Send + Sync,
{
	let block_state_start = std::time::Instant::now();
	let (_storage, phase, current_round) = get_block_state(client, block_hash).await?;
	let block_state_duration = block_state_start.elapsed();
	prometheus::observe_block_state_duration(block_state_duration.as_millis() as f64);

	// Update prev_round before processing round increment to avoid stale round detection in case of
	// timeouts
	let last_round = state.prev_round;
	state.prev_round = Some(current_round);

	if let Some(last_round) = last_round &&
		current_round > last_round
	{
		on_round_increment(
			last_round,
			current_round,
			&phase,
			&channels.miner_tx,
			&channels.clear_old_rounds_tx,
		)
		.await?;
	}

	let last_phase = state.prev_phase.clone();
	state.prev_phase = Some(phase.clone());
	let block_number = at.number;

	match phase {
		Phase::Off => {
			if let Some(last_phase) = last_phase {
				if !matches!(&last_phase, Phase::Off) {
					log::debug!(target: LOG_TARGET, "Phase transition: {last_phase:?} → Off - starting era pruning");
					// Use send() to ensure EnterOffPhase is delivered and to prevent missed phase
					// transitions that could leave era pruning in wrong state
					if let Err(e) =
						channels.era_pruning_tx.send(EraPruningMessage::EnterOffPhase).await
					{
						log::error!(target: LOG_TARGET, "Era pruning channel closed while sending EnterOffPhase: {e:?}");
						return Err(ChannelFailureError::EraPruning.into());
					}
				}
			} else {
				log::debug!(target: LOG_TARGET, "First block in Off phase - starting era pruning");
				// First block in Off phase
				// Use send() to ensure the message is delivered
				if let Err(e) = channels.era_pruning_tx.send(EraPruningMessage::EnterOffPhase).await
				{
					log::error!(target: LOG_TARGET, "Era pruning channel closed while sending initial EnterOffPhase: {e:?}");
					return Err(ChannelFailureError::EraPruning.into());
				}
			}

			// Send NewBlock message for era pruning (non-critical, we can use try_send)
			let _ = channels
				.era_pruning_tx
				.try_send(EraPruningMessage::NewBlock { block_number: at.number });

			// Off phase - nothing to do for mining
			log::trace!(target: LOG_TARGET, "Block #{block_number}, Phase Off - nothing to do");
			prometheus::set_last_block_processing_time();
			return Ok(ListenerAction::Continue);
		},
		Phase::Signed(_) | Phase::Snapshot(_) => {
			if let Some(last_phase) = last_phase &&
				matches!(&last_phase, Phase::Off)
			{
				log::debug!(target: LOG_TARGET, "Phase transition: Off → {phase:?} - stopping era pruning");
				// Use send() to ensure ExitOffPhase is delivered and to stop era pruning before
				// mining starts
				if let Err(e) = channels.era_pruning_tx.send(EraPruningMessage::ExitOffPhase).await
				{
					log::error!(target: LOG_TARGET, "Era pruning channel closed while sending ExitOffPhase: {e:?}");
					return Err(ChannelFailureError::EraPruning.into());
				}
			}
			// Continue with mining logic for Signed/Snapshot phases
		},
		_ => {
			log::trace!(target: LOG_TARGET, "Block #{block_number}, Phase {phase:?} - nothing to do");
			prometheus::set_last_block_processing_time();
			return Ok(ListenerAction::Continue);
		},
	}

	let block_details_start = std::time::Instant::now();
	let state = BlockDetails::new(client, at, phase, block_hash, current_round).await?;
	let block_details_duration = block_details_start.elapsed();
	prometheus::observe_block_details_duration(block_details_duration.as_millis() as f64);

	let message = MinerMessage::ProcessBlock { state };

	// Use try_send for backpressure - if miner is busy, skip this block
	match channels.miner_tx.try_send(message) {
		Ok(()) => {
			log::trace!(target: LOG_TARGET, "Sent block #{block_number} to miner");
			// Update timestamp of successful listener's block processing
			prometheus::set_last_block_processing_time();
			// Don't wait for response to allow proper backpressure - listener must continue
			// processing blocks
		},
		Err(mpsc::error::TrySendError::Full(_)) => {
			// Miner is busy processing another block - apply backpressure by skipping
			log::trace!(target: LOG_TARGET, "Miner busy, skipping block #{block_number}");
		},
		Err(mpsc::error::TrySendError::Closed(_)) => {
			log::error!(target: LOG_TARGET, "Miner channel closed unexpectedly");
			return Err(ChannelFailureError::Miner.into());
		},
	}

	Ok(ListenerAction::Continue)
}

/// Process a single iteration of the listener loop
///
/// This inner function contains all the logic for processing one block or timeout,
/// with comprehensive error handling that allows the outer loop to decide whether
/// to continue or exit based on error classification.
async fn process_listener_iteration<T>(
	client: &Client,
	subscription: &mut SubscriptionStream,
	state: &mut ListenerState,
	channels: &TaskChannels,
) -> Result<ListenerAction, Error>
where
	T: MinerConfig<AccountId = AccountId> + Send + Sync,
	T::Solution: Send + Sync,
	T::Pages: Send + Sync,
	T::TargetSnapshotPerBlock: Send + Sync,
	T::VoterSnapshotPerBlock: Send + Sync,
	T::MaxVotesPerVoter: Send + Sync,
{
	let (at, block_hash) = match tokio::time::timeout(
		std::time::Duration::from_secs(BLOCK_SUBSCRIPTION_TIMEOUT_SECS),
		subscription.next(),
	)
	.await
	{
		Ok(maybe_block) => {
			match maybe_block {
				Some(Ok(block)) => {
					state.last_block_time = std::time::Instant::now();
					// Reset the subscription recreation attempts counter on successful block
					// receipt
					state.subscription_recreation_attempts = 0;
					(block.header().clone(), block.hash())
				},
				Some(Err(e)) => {
					// Handle reconnection case with the reconnecting RPC client
					if e.is_disconnected_will_reconnect() {
						log::warn!(target: LOG_TARGET, "RPC connection lost, but will reconnect automatically. Continuing...");
						return Ok(ListenerAction::Continue);
					}
					log::error!(target: LOG_TARGET, "subscription failed: {e:?}");
					return Err(e.into());
				},
				// The subscription was dropped unexpectedly
				None => {
					log::error!(target: LOG_TARGET, "Subscription to finalized blocks terminated unexpectedly");
					return Err(Error::Other("Subscription terminated unexpectedly".to_string()));
				},
			}
		},
		Err(_) => {
			log::warn!(target: LOG_TARGET, "No blocks received for {BLOCK_SUBSCRIPTION_TIMEOUT_SECS} seconds - subscription may be stalled, recreating subscription...");
			crate::prometheus::on_listener_subscription_stall();

			// Check if we've exceeded the maximum number of recreation attempts
			state.subscription_recreation_attempts += 1;
			if state.subscription_recreation_attempts > MAX_SUBSCRIPTION_RECREATION_ATTEMPTS {
				log::error!(target: LOG_TARGET, "Exceeded maximum subscription recreation attempts ({MAX_SUBSCRIPTION_RECREATION_ATTEMPTS}), exiting to allow restart");
				return Err(Error::SubscriptionRecreationLimitExceeded {
					max_attempts: MAX_SUBSCRIPTION_RECREATION_ATTEMPTS,
				});
			}

			log::info!(target: LOG_TARGET, "Subscription recreation attempt {}/{MAX_SUBSCRIPTION_RECREATION_ATTEMPTS}", state.subscription_recreation_attempts);

			// Recreate the subscription
			match client.chain_api().blocks().subscribe_finalized().await {
				Ok(new_subscription) => {
					*subscription = new_subscription;
					state.last_block_time = std::time::Instant::now();
					log::info!(target: LOG_TARGET, "Successfully recreated finalized block subscription");
					return Ok(ListenerAction::SubscriptionRecreated);
				},
				Err(e) => {
					log::error!(target: LOG_TARGET, "Failed to recreate subscription: {e:?}");
					return Err(e.into());
				},
			}
		},
	};

	// Now wrap the entire block processing with timeout
	match tokio::time::timeout(
		std::time::Duration::from_secs(BLOCK_PROCESSING_TIMEOUT_SECS),
		process_block_internal::<T>(client, at.clone(), block_hash, state, channels),
	)
	.await
	{
		Ok(result) => result,
		Err(_) => {
			log::warn!(target: LOG_TARGET, "Block processing timed out after {}s for block #{} - may indicate internal hang, recreating subscription...", BLOCK_PROCESSING_TIMEOUT_SECS, at.number);
			prometheus::on_block_processing_stall();

			// Check if we've exceeded the maximum number of recreation attempts
			state.subscription_recreation_attempts += 1;
			if state.subscription_recreation_attempts > MAX_SUBSCRIPTION_RECREATION_ATTEMPTS {
				log::error!(target: LOG_TARGET, "Exceeded maximum subscription recreation attempts ({MAX_SUBSCRIPTION_RECREATION_ATTEMPTS}), exiting to allow restart");
				return Err(Error::SubscriptionRecreationLimitExceeded {
					max_attempts: MAX_SUBSCRIPTION_RECREATION_ATTEMPTS,
				});
			}

			log::info!(target: LOG_TARGET, "Subscription recreation attempt {}/{MAX_SUBSCRIPTION_RECREATION_ATTEMPTS}", state.subscription_recreation_attempts);

			match client.chain_api().blocks().subscribe_finalized().await {
				Ok(new_subscription) => {
					*subscription = new_subscription;
					state.last_block_time = std::time::Instant::now();
					log::info!(target: LOG_TARGET, "Successfully recreated subscription after block processing timeout");
					Ok(ListenerAction::BlockProcessingTimeout)
				},
				Err(e) => {
					log::error!(target: LOG_TARGET, "Failed to recreate subscription after block processing timeout: {e:?}");
					Err(e.into())
				},
			}
		},
	}
}

/// Determine if a listener error is critical and should cause the process to exit
fn is_critical_listener_error(error: &Error) -> bool {
	match error {
		// RPC errors are generally recoverable with the reconnecting client
		Error::Subxt(boxed_err) if matches!(boxed_err.as_ref(), subxt::Error::Rpc(_)) => false,
		// Storage query failures can happen due to stale block hashes
		Error::Subxt(boxed_err) if matches!(boxed_err.as_ref(), subxt::Error::Runtime(_)) => false,
		// Transaction errors are not relevant for the listener
		Error::Subxt(boxed_err) if matches!(boxed_err.as_ref(), subxt::Error::Transaction(_)) =>
			false,
		// Channel failures are critical - indicates tasks have died
		Error::ChannelFailure(_) => true,
		// Task failures are critical - indicates tasks have terminated
		Error::TaskFailure(_) => true,
		// Subscription termination is critical
		Error::Other(msg) if msg.contains("Subscription terminated") => true,
		// Everything else is considered recoverable for the listener
		// This includes temporary issues like:
		// - BlockDetails creation failures
		// - Storage access issues
		// - Temporary network problems
		_ => false,
	}
}

/// Get block state with better error handling for storage queries
async fn get_block_state(
	client: &Client,
	block_hash: polkadot_sdk::sp_core::H256,
) -> Result<(Storage, Phase, u32), Error> {
	let storage_start = std::time::Instant::now();
	let storage = utils::storage_at(Some(block_hash), client.chain_api()).await?;
	let storage_duration = storage_start.elapsed();
	prometheus::observe_storage_query_duration(storage_duration.as_millis() as f64);

	let phase_start = std::time::Instant::now();
	let phase = storage
		.fetch_or_default(&runtime::storage().multi_block_election().current_phase())
		.await?;
	let phase_duration = phase_start.elapsed();
	prometheus::observe_storage_query_duration(phase_duration.as_millis() as f64);

	let round_start = std::time::Instant::now();
	let current_round = storage
		.fetch_or_default(&runtime::storage().multi_block_election().round())
		.await?;
	let round_duration = round_start.elapsed();
	prometheus::observe_storage_query_duration(round_duration.as_millis() as f64);

	Ok((storage, phase, current_round))
}

/// Handle round increment by triggering clear old rounds and snapshot cleanup
async fn on_round_increment(
	last_round: u32,
	current_round: u32,
	phase: &Phase,
	miner_tx: &mpsc::Sender<MinerMessage>,
	clear_old_rounds_tx: &mpsc::Sender<ClearOldRoundsMessage>,
) -> Result<(), Error> {
	log::debug!(target: LOG_TARGET, "Detected round increment {last_round} -> {current_round}");

	// 1. Trigger clear old rounds cleanup
	if let Err(e) =
		clear_old_rounds_tx.try_send(ClearOldRoundsMessage::ClearOldRoundsTick { current_round })
	{
		match e {
			mpsc::error::TrySendError::Full(_) => {
				// this shouldn't happen. Clearing old rounds is triggered once per round.
				// If it's still busy after a round, it means that it has taken
				// insanely long.
				log::warn!(target: LOG_TARGET, "Clear old rounds busy, skipping clear old rounds tick for round {current_round}.");
			},
			mpsc::error::TrySendError::Closed(_) => {
				log::error!(target: LOG_TARGET, "Clear old rounds channel closed unexpectedly during clear old rounds tick");
				return Err(ChannelFailureError::ClearOldRounds.into());
			},
		}
	} else {
		log::trace!(target: LOG_TARGET, "Sent clear old rounds tick for round {current_round}");
	}

	// 2. Clear snapshots
	if matches!(phase, Phase::Off) {
		log::debug!(target: LOG_TARGET, "Round increment in Off phase, signaling snapshot cleanup");
		if let Err(e) = miner_tx.send(MinerMessage::ClearSnapshots).await {
			log::error!(target: LOG_TARGET, "Failed to send clear snapshots signal: {e}");
			return Err(Error::Other(format!("Failed to send clear snapshots signal: {e}")));
		}
	} else {
		// this should really never happen, a new round should always starts with Off phase!
		log::warn!(target: LOG_TARGET, "Round increment in {phase:?} phase, skipping snapshot cleanup");
	}

	Ok(())
}

/// Message types for communication between listener and miner
enum MinerMessage {
	/// Request to process a block with given details
	ProcessBlock { state: BlockDetails },

	/// Signal to clear snapshots (phase ended)
	ClearSnapshots,
}

/// Message types for communication between listener and clear old rounds task
enum ClearOldRoundsMessage {
	/// Signal to run clear old rounds cleanup for old submissions
	ClearOldRoundsTick { current_round: u32 },
}

/// Message types for communication between listener and era pruning task
enum EraPruningMessage {
	/// Signal to start era pruning when entering Off phase
	EnterOffPhase,
	/// Signal to stop era pruning immediately when leaving Off phase
	ExitOffPhase,
	/// New block arrived during Off phase, can send one prune_era_step if needed
	NewBlock { block_number: u32 },
}

/// The monitor command splits the work into four communicating tasks:
///
/// ### Listener Task
/// - Always follows the chain (head or finalized blocks)
/// - Performs fast phase checks to identify relevant blocks (Snapshot or Signed)
/// - Checks if miner is busy before sending work
/// - Never blocks on slow operations
/// - **Any error causes entire process to exit** (RPC errors handled by reconnecting client)
/// - Triggers clear old rounds cleanup and snapshot clearing when round increments
///
/// ### Miner Task
/// - Single-threaded processor that handles one block at a time
/// - **Mining**: Processes Snapshot/Signed phases for solution mining and submission
/// - **Cleanup**: Clears snapshots when round increments (in Off phase)
/// - **Critical errors cause process exit** (e.g. account doesn't exist)
/// - **Non-critical errors are recoverable** (log and continue)
///
/// ### Clear old rounds Task
/// - Independent task that handles deposit recovery operations
/// - Automatically scans for and cleans up old submissions from previous rounds
/// - Reclaims deposits from discarded solutions that were not selected
/// - Runs exactly once per round when round number increments
/// - Scans only the last CLEAR_OLD_ROUNDS_SCAN_ROUNDS rounds for old submissions
/// - Calls `clear_old_round_data()` to remove stale data and recover deposits
/// - **Non-blocking**: Does not interfere with mining operations
///
/// ### Era Pruning Task
/// - Independent task that handles lazy era pruning operations using runtime's EraPruningState
///   storage map
/// - Only runs during Off phase (to not interfere with ongoing elections)
/// - Rate limited to maximum one `prune_era_step()` call per block
///
/// ### Communication
/// - **Miner Channel**: Bounded channel (buffer=1) for mining work + snapshot cleanup
/// - **Clear Old Rounds Channel**: Bounded channel (buffer=1) for deposit recovery operations
/// - **Dual Triggers**: Listener sends mining work (Snapshot/Signed) + cleanup signals (round++)
/// - **Backpressure mechanism**: When miner is busy processing a block:
///   - Channel becomes full, listener's try_send() returns immediately
///   - Listener skips current block and moves to next (fresher) block
///   - No blocking, no buffering of stale work
/// - **Separation of concerns**: Mining, clear old rounds, and era pruning operations are
///   completely independent
/// - No submission locks needed since miner is single-threaded by design
///
///
/// ## Error Handling Strategy
/// - **Listener errors**: All critical - process exits immediately
/// - **Miner errors**: Classified as critical or recoverable
///   - Critical: Account doesn't exist, invalid metadata, persistent RPC failures
///   - Recoverable: Already submitted, wrong phase, temporary mining issues
/// - **Clear Old Rounds errors**: Classified as critical or recoverable (same as miner)
/// - **Era pruning errors**: Classified as critical or recoverable (same as miner)
/// - **RPC issues**: Handled transparently by reconnecting RPC client
///
/// ```text
/// 
///                      (finalized blocks)
/// ┌──────────────────────────────────────────────────────────────────────────┐
/// │  ┌─────────────┐                      ┌─────────────┐              ┌─────────────┐
/// └─▶│ Listener    │                      │   Miner     │              │ Blockchain  │
///    │             │  Snapshot/Signed     │             │              │             │
///    │ ┌─────────┐ │ ────────────────────▶│ ┌─────────┐ │ (solutions)  │             │
///    │ │ Stream  │ │  (mining work)       │ │ Mining  │ │───────────▶  │             │
///    │ └─────────┘ │                      │ └─────────┘ │              │             │
///    │      │      │  Round++             │ ┌─────────┐ │              │             │
///    │      ▼      │ ────────────────────▶│ │ Clear   │ │              │             │
///    │ ┌─────────┐ │                      │ │ Snapshot│ │              │             │
///    │ │ Phase   │ │                      │ └─────────┘ │              │             │
///    │ │ Check   │ │  Round++             └─────────────┘              │             │
///    │ └─────────┘ │ ────────────────────▶┌───────────────┐            │             │
///    │     │       │  (deposit cleanup)   │ClearOldRounds │ (cleanup)  │             │
///    │     │       │                      │ ┌─────────┐   │───────────▶│             │
///    │     │       │                      │ │ Cleanup │   │            │             │
///    │     │       │                      │ └─────────┘   │            │             │
///    │     │       │                      └───────────────┘            │             │
///    │     │       │                                                   │             │
///    │     │       │Entering/leaving Off ┌───────────────┐(prune_era_step)           │
///    │     └───────────────────────────▶ │ Era pruning   │───────────▶ │             │
///    │             │   New block in Off  └───────────────┘             │             │
///    │             │                                                   │             │
///    └─────────────┘                                                   └─────────────┘
/// ```
pub async fn monitor_cmd<T>(client: Client, config: MultiBlockMonitorConfig) -> Result<(), Error>
where
	T: MinerConfig<AccountId = AccountId> + Send + Sync + 'static,
	T::Solution: Send + Sync + 'static,
	T::Pages: Send + Sync + 'static,
	T::TargetSnapshotPerBlock: Send + Sync + 'static,
	T::VoterSnapshotPerBlock: Send + Sync + 'static,
	T::MaxVotesPerVoter: Send + Sync + 'static,
{
	crate::dynamic::set_balancing_iterations(config.balancing_iterations);

	let signer = Signer::new(&config.seed_or_path)?;

	// Emit the account info at the start.
	{
		let account_info = client
			.chain_api()
			.storage()
			.at_latest()
			.await?
			.fetch(&runtime::storage().system().account(signer.account_id().clone()))
			.await?
			.ok_or(Error::AccountDoesNotExists)?;
		prometheus::set_balance(account_info.data.free as f64);

		log::info!(
			target: LOG_TARGET,
			"Loaded account {} {{ nonce: {}, free_balance: {}, reserved_balance: {}, frozen_balance: {} }}",
			signer,
			account_info.nonce,
			account_info.data.free,
			account_info.data.reserved,
			account_info.data.frozen,
		);
	}

	// Create bounded channels for communication between listener and miner
	//
	// Buffer size of 1 provides natural backpressure control:
	// - When miner is processing a block, the channel is full
	// - Listener's try_send() will return TrySendError::Full
	// - This causes listener to skip the current block and continue to next
	// - Prevents unbounded memory growth and ensures fresh work
	// - Eliminates the need for explicit busy-checking or submission locks
	let (miner_tx, miner_rx) = mpsc::channel::<MinerMessage>(1);

	// Create bounded channel for communication between listener and clear old rounds task
	// Buffer size of 1 is sufficient since clear old rounds runs once at every round change.
	let (clear_old_rounds_tx, clear_old_rounds_rx) = mpsc::channel::<ClearOldRoundsMessage>(1);

	// Create bounded channel for communication between listener and era pruning task
	// Buffer size of 1 is sufficient - era pruning is non-critical cleanup, skipping blocks is fine
	let (era_pruning_tx, era_pruning_rx) = mpsc::channel::<EraPruningMessage>(1);

	// Spawn the miner task
	let miner_handle = {
		let client = client.clone();
		let signer = signer.clone();
		let config = config.clone();
		tokio::spawn(async move { miner_task::<T>(client, signer, config, miner_rx).await })
	};

	// Spawn the clear old rounds task
	let clear_old_rounds_handle = {
		let client = client.clone();
		let signer = signer.clone();
		tokio::spawn(async move {
			clear_old_rounds_task::<T>(client, signer, clear_old_rounds_rx).await
		})
	};

	// Spawn the era pruning task
	let era_pruning_handle = {
		let client = client.clone();
		let signer = signer.clone();
		tokio::spawn(
			async move { lazy_era_pruning_task::<T>(client, signer, era_pruning_rx).await },
		)
	};

	// Spawn the listener task
	let listener_handle = {
		let client = client.clone();
		tokio::spawn(async move {
			listener_task::<T>(client, miner_tx, clear_old_rounds_tx, era_pruning_tx).await
		})
	};

	// Wait for any task to complete (which should never happen in normal operation)
	// If any task exits, the whole process should exit
	tokio::select! {
		result = listener_handle => {
			match result {
				Ok(Ok(())) => {
					log::error!(target: LOG_TARGET, "Listener task completed unexpectedly");
					Err(TaskFailureError::Listener.into())
				}
				Ok(Err(e)) => {
					log::error!(target: LOG_TARGET, "Listener task failed: {e}");
					Err(e)
				}
				Err(e) => {
					log::error!(target: LOG_TARGET, "Listener task panicked: {e}");
					Err(TaskFailureError::Listener.into())
				}
			}
		}
		result = miner_handle => {
			match result {
				Ok(Ok(())) => {
					log::error!(target: LOG_TARGET, "Miner task completed unexpectedly");
					Err(TaskFailureError::Miner.into())
				}
				Ok(Err(e)) => {
					log::error!(target: LOG_TARGET, "Miner task failed: {e}");
					Err(e)
				}
				Err(e) => {
					log::error!(target: LOG_TARGET, "Miner task panicked: {e}");
					Err(TaskFailureError::Miner.into())
				}
			}
		}
		result = clear_old_rounds_handle => {
			match result {
				Ok(Ok(())) => {
					log::error!(target: LOG_TARGET, "Clear old rounds task completed unexpectedly");
					Err(TaskFailureError::ClearOldRounds.into())
				}
				Ok(Err(e)) => {
					log::error!(target: LOG_TARGET, "Clear old rounds task failed: {e}");
					Err(e)
				}
				Err(e) => {
					log::error!(target: LOG_TARGET, "Clear old rounds task panicked: {e}");
					Err(TaskFailureError::ClearOldRounds.into())
				}
			}
		}
		result = era_pruning_handle => {
			match result {
				Ok(Ok(())) => {
					log::error!(target: LOG_TARGET, "Era pruning task completed unexpectedly");
					Err(TaskFailureError::EraPruning.into())
				}
				Ok(Err(e)) => {
					log::error!(target: LOG_TARGET, "Era pruning task failed: {e}");
					Err(e)
				}
				Err(e) => {
					log::error!(target: LOG_TARGET, "Era pruning task panicked: {e}");
					Err(TaskFailureError::EraPruning.into())
				}
			}
		}
	}
}

/// Listener task that follows the chain and signals the miner when appropriate
///
/// This task is responsible for:
/// - Subscribing to finalized block updates
/// - Performing fast phase checks to identify
///   - signed/snapshot phases to inform the miner task
///   - transition from Done/Export(0) to Off to inform the clear old rounds task
/// - Using backpressure to skip blocks when miner is busy
/// - Managing phase transitions and snapshot cleanup
/// - Never blocking on slow operations to avoid subscription buffering
/// - Any error causes the entire process to exit
async fn listener_task<T>(
	client: Client,
	miner_tx: mpsc::Sender<MinerMessage>,
	clear_old_rounds_tx: mpsc::Sender<ClearOldRoundsMessage>,
	era_pruning_tx: mpsc::Sender<EraPruningMessage>,
) -> Result<(), Error>
where
	T: MinerConfig<AccountId = AccountId> + Send + Sync + 'static,
	T::Solution: Send + Sync + 'static,
	T::Pages: Send + Sync + 'static,
	T::TargetSnapshotPerBlock: Send + Sync + 'static,
	T::VoterSnapshotPerBlock: Send + Sync + 'static,
	T::MaxVotesPerVoter: Send + Sync + 'static,
{
	let mut subscription = client.chain_api().blocks().subscribe_finalized().await?;
	let mut state = ListenerState {
		prev_round: None,
		prev_phase: None,
		last_block_time: std::time::Instant::now(),
		subscription_recreation_attempts: 0,
	};

	let channels = TaskChannels { miner_tx, clear_old_rounds_tx, era_pruning_tx };

	log::trace!(target: LOG_TARGET, "Listener task started, watching for finalized blocks");

	loop {
		match process_listener_iteration::<T>(&client, &mut subscription, &mut state, &channels)
			.await
		{
			Ok(ListenerAction::Continue) => continue,
			Ok(ListenerAction::SubscriptionRecreated) => {
				log::info!(target: LOG_TARGET, "Successfully processed subscription recreation");
				continue;
			},
			Ok(ListenerAction::BlockProcessingTimeout) => {
				log::info!(target: LOG_TARGET, "Successfully processed subscription recreation after block processing timeout");
				continue;
			},
			Err(e) => {
				// Classify the error to decide whether to retry or exit
				if is_critical_listener_error(&e) {
					log::error!(target: LOG_TARGET, "Critical listener error, exiting: {e:?}");
					return Err(e);
				} else {
					log::warn!(target: LOG_TARGET, "Non-critical listener error, continuing: {e:?}");
					// Add a small delay to prevent tight error loops
					tokio::time::sleep(std::time::Duration::from_millis(100)).await;
					continue;
				}
			},
		}
	}
}

/// Miner task that processes mining requests
///
/// This task is responsible for:
/// - Processing mining requests one at a time (single-threaded)
/// - Performing expensive mining and submission operations
/// - Handling snapshot fetching and management
/// - Providing natural backpressure via bounded channel
/// - Critical errors cause process exit, others are recoverable
///
/// No submission lock is needed since this task is inherently single-threaded.
/// Backpressure is automatically applied when the bounded channel is full.
async fn miner_task<T>(
	client: Client,
	signer: Signer,
	config: MultiBlockMonitorConfig,
	mut miner_rx: mpsc::Receiver<MinerMessage>,
) -> Result<(), Error>
where
	T: MinerConfig<AccountId = AccountId> + Send + Sync + 'static,
	T::Solution: Send + Sync + 'static,
	T::Pages: Send + Sync + 'static,
	T::TargetSnapshotPerBlock: Send + Sync + 'static,
	T::VoterSnapshotPerBlock: Send + Sync + 'static,
	T::MaxVotesPerVoter: Send + Sync + 'static,
{
	log::trace!(target: LOG_TARGET, "Miner task started");

	// Miner owns the snapshot exclusively
	let mut snapshot = Snapshot::<T>::new(static_types::Pages::get());

	while let Some(message) = miner_rx.recv().await {
		match message {
			MinerMessage::ProcessBlock { state } => {
				let process_config = ProcessConfig {
					submission_strategy: config.submission_strategy,
					do_reduce: config.do_reduce,
					chunk_size: config.chunk_size,
					min_signed_phase_blocks: config.min_signed_phase_blocks,
					shady: config.shady,
				};
				if let Err(e) = process_block::<T>(
					client.clone(),
					state,
					&mut snapshot,
					signer.clone(),
					process_config,
				)
				.await
				{
					if is_critical_miner_error(&e) {
						log::error!(target: LOG_TARGET, "Critical miner error - process will exit: {e:?}");
						return Err(e);
					} else {
						log::warn!(target: LOG_TARGET, "Block processing failed, continuing: {e:?}");
					}
				}
			},
			MinerMessage::ClearSnapshots => {
				log::trace!(target: LOG_TARGET, "Clearing snapshots");
				snapshot.clear();
			},
		}
	}

	// This should never be reached as miner runs indefinitely
	log::error!(target: LOG_TARGET, "Miner task loop exited unexpectedly");
	Err(TaskFailureError::Miner.into())
}

/// Clear old rounds task - handles deposit recovery operations independently from mining
///
/// This task runs in its own thread and processes clear old rounds cleanup requests without
/// blocking the miner task. It maintains separation of concerns by focusing solely
/// on deposit recovery while the miner focuses on mining and submission operations.
async fn clear_old_rounds_task<T>(
	client: Client,
	signer: Signer,
	mut clear_old_rounds_rx: mpsc::Receiver<ClearOldRoundsMessage>,
) -> Result<(), Error>
where
	T: MinerConfig<AccountId = AccountId> + Send + Sync + 'static,
	T::Solution: Send + Sync + 'static,
	T::Pages: Send + Sync + 'static,
	T::TargetSnapshotPerBlock: Send + Sync + 'static,
	T::VoterSnapshotPerBlock: Send + Sync + 'static,
	T::MaxVotesPerVoter: Send + Sync + 'static,
{
	log::trace!(target: LOG_TARGET, "Clear old rounds task started");

	// Track the last round we've scanned to avoid rescanning
	let mut last_scanned_round: Option<u32> = None;

	while let Some(message) = clear_old_rounds_rx.recv().await {
		match message {
			ClearOldRoundsMessage::ClearOldRoundsTick { current_round } => {
				log::trace!(target: LOG_TARGET, "Running clear old rounds cleanup for round {current_round}");

				let start_time = std::time::Instant::now();
				let result = run_clear_old_rounds_cleanup::<T>(
					client.clone(),
					signer.clone(),
					current_round,
					&mut last_scanned_round,
				)
				.await;
				let duration = start_time.elapsed().as_millis() as f64;
				prometheus::observe_clear_old_rounds_cleanup_duration(duration);

				match result {
					Ok(cleaned_count) => {
						prometheus::on_clear_old_rounds_cleanup_success(cleaned_count);
						if cleaned_count > 0 {
							log::info!(target: LOG_TARGET, "Clear old rounds cleaned up {} old submissions in {}ms", cleaned_count, duration as u64);
						} else {
							log::trace!(target: LOG_TARGET, "Clear old rounds found no old submissions to clean up");
						}
					},
					Err(e) => {
						prometheus::on_clear_old_rounds_cleanup_failure();
						if is_critical_miner_error(&e) {
							log::error!(target: LOG_TARGET, "Critical clear old rounds error - process will exit: {e:?}");
							return Err(e);
						} else {
							log::warn!(target: LOG_TARGET, "Clear old rounds cleanup failed, continuing: {e:?}");
						}
					},
				}
			},
		}
	}

	// This should never be reached as clear old rounds runs indefinitely
	log::error!(target: LOG_TARGET, "Clear old rounds task loop exited unexpectedly");
	Err(TaskFailureError::ClearOldRounds.into())
}

/// Returns (era_count, era_index) where era_count is the total number of eras in the map
/// and era_index is Some(oldest_era) if there is at least one era to prune, None if map is empty
async fn get_pruneable_era_index(client: &Client) -> Result<(u32, Option<u32>), Error> {
	let storage = utils::storage_at_head(client).await?;

	let iter = match storage.iter(runtime::storage().staking().era_pruning_state_iter()).await {
		Ok(iter) => iter,
		Err(_) => {
			// Handle older runtimes that don't have EraPruningState storage
			return Ok((0, None));
		},
	};

	let mut era_indices = iter
		.try_fold(Vec::new(), |mut acc, storage_entry| async move {
			// The generated metadata from `subxt-cli` incorrectly returns `()` (unit type)
			// instead of the actual era index type for the storage key (see https://github.com/paritytech/subxt/issues/2091).
			// TODO: use the properly typed `keys` field instead of manually parsing
			// `key_bytes`. Or use the new subxt Storage APIs when available.

			// Format: concat(twox64(era_index), era_index) - extract the last 4 bytes as u32
			if storage_entry.key_bytes.len() >= 4 {
				let era_bytes = &storage_entry.key_bytes[storage_entry.key_bytes.len() - 4..];
				if let Ok(era_index) = u32::decode(&mut &era_bytes[..]) {
					acc.push(era_index);
				}
			}
			Ok(acc)
		})
		.await?;

	era_indices.sort_unstable();

	Ok((era_indices.len() as u32, era_indices.first().copied()))
}

/// Call prune_era_step for the given era
///
/// Returns:
/// - Ok(()) if transaction was submitted successfully
/// - Err(_) if there was an error submitting the transaction
async fn call_prune_era_step(client: &Client, signer: &Signer, era: u32) -> Result<(), Error> {
	let tx = runtime::tx().staking().prune_era_step(era);

	let nonce = client.chain_api().tx().account_nonce(signer.account_id()).await?;
	let xt_cfg = ExtrinsicParamsBuilder::default().nonce(nonce).build();
	let xt = client.chain_api().tx().create_signed(&tx, &**signer, xt_cfg).await?;

	// Wait for finalization to avoid to spam the same exact transaction at the next block, and get
	// an InvalidTransaction::Stale ("Transaction is outdated") in return.
	let tx_progress = xt.submit_and_watch().await?;
	let tx = utils::wait_tx_in_finalized_block(tx_progress).await?;

	log::trace!(target: LOG_TARGET, "Successfully submitted prune_era_step for era {era}, tx_hash: {:?}", tx.extrinsic_hash());

	prometheus::on_era_pruning_submission_success();

	Ok(())
}

/// Lazy era pruning task
///
/// This task implements era pruning using the runtime's EraPruningState storage map.
/// It operates only during Off phases with rate limiting (one prune_era_step per block).
async fn lazy_era_pruning_task<T>(
	client: Client,
	signer: Signer,
	mut era_pruning_rx: mpsc::Receiver<EraPruningMessage>,
) -> Result<(), Error>
where
	T: MinerConfig<AccountId = AccountId> + Send + Sync + 'static,
	T::Solution: Send + Sync + 'static,
	T::Pages: Send + Sync + 'static,
	T::TargetSnapshotPerBlock: Send + Sync + 'static,
	T::VoterSnapshotPerBlock: Send + Sync + 'static,
	T::MaxVotesPerVoter: Send + Sync + 'static,
{
	log::trace!(target: LOG_TARGET, "Era pruning task started");

	let mut pruning_active = false;

	while let Some(message) = era_pruning_rx.recv().await {
		match message {
			EraPruningMessage::EnterOffPhase => {
				log::debug!(target: LOG_TARGET, "Entering Off phase, starting pruning session");
				pruning_active = true;
			},

			EraPruningMessage::ExitOffPhase => {
				log::debug!(target: LOG_TARGET, "Exiting Off phase, stopping pruning session");
				pruning_active = false;
			},

			EraPruningMessage::NewBlock { block_number } => {
				if !pruning_active {
					log::trace!(target: LOG_TARGET, "Era pruning inactive, skipping block #{block_number}");
					continue;
				}

				match tokio::time::timeout(
					std::time::Duration::from_secs(ERA_PRUNING_TIMEOUT_SECS),
					get_pruneable_era_index(&client),
				)
				.await
				{
					Ok(Ok((era_count, Some(oldest_era)))) => {
						log::trace!(target: LOG_TARGET, "Found era {oldest_era} to prune in block #{block_number} (map size: {era_count})");
						prometheus::set_era_pruning_storage_map_size(era_count);

						match call_prune_era_step(&client, &signer, oldest_era).await {
							Ok(()) => {
								log::debug!(target: LOG_TARGET, "prune_era_step({oldest_era}) submitted successfully");
							},
							Err(e) => {
								log::warn!(target: LOG_TARGET, "Failed to prune era {oldest_era}: {e:?}");
								prometheus::on_era_pruning_submission_failure();
							},
						}
					},
					Ok(Ok((era_count, None))) => {
						log::trace!(target: LOG_TARGET, "No eras to prune in block #{block_number} (map size: {era_count})");
						prometheus::set_era_pruning_storage_map_size(era_count);
					},
					Ok(Err(e)) => {
						log::warn!(target: LOG_TARGET, "Failed to query EraPruningState in block #{block_number}: {e:?}");
					},
					Err(_) => {
						log::error!(target: LOG_TARGET, "Era pruning storage query timed out after {ERA_PRUNING_TIMEOUT_SECS}s for block #{block_number} - continuing with next block");
						prometheus::on_era_pruning_timeout();
					},
				}
			},
		}
	}

	// This should never be reached as era pruning runs indefinitely
	log::error!(target: LOG_TARGET, "Era pruning task loop exited unexpectedly");
	Err(TaskFailureError::EraPruning.into())
}

/// Process a single block
///
/// This function handles the core mining logic for a single block:
/// 1. Update account balance and page length
/// 2. Handle snapshot vs signed phase appropriately
/// 3. Fetch missing snapshots if needed
/// 4. Check if already submitted for this round
/// 5. Mine the solution
/// 6. Handle existing submissions (complete/incomplete)
/// 7. Check score competitiveness
/// 8. Submit the solution
///
/// No submission lock is needed since the miner task is single-threaded.
/// Retransmission scenarios are handled after miner restarts or runtime upgrades.
struct ProcessConfig {
	submission_strategy: SubmissionStrategy,
	do_reduce: bool,
	chunk_size: usize,
	min_signed_phase_blocks: u32,
	shady: bool,
}

async fn process_block<T>(
	client: Client,
	state: BlockDetails,
	snapshot: &mut Snapshot<T>,
	signer: Signer,
	config: ProcessConfig,
) -> Result<(), Error>
where
	T: MinerConfig<AccountId = AccountId> + Send + Sync + 'static,
	T::Solution: Send + Sync + 'static,
	T::Pages: Send + Sync + 'static,
	T::TargetSnapshotPerBlock: Send,
	T::VoterSnapshotPerBlock: Send,
	T::MaxVotesPerVoter: Send + Sync + 'static,
{
	let BlockDetails { storage, phase, round, n_pages, desired_targets, block_number, .. } = state;

	log::trace!(target: LOG_TARGET, "Processing block #{block_number} (round {round}, phase {phase:?})");

	// Update balance with timeout to prevent hanging
	const BALANCE_FETCH_TIMEOUT_SECS: u64 = 60; // 1 minute
	let account_info = match tokio::time::timeout(
		std::time::Duration::from_secs(BALANCE_FETCH_TIMEOUT_SECS),
		async {
			let start_time = std::time::Instant::now();
			let result = storage
				.fetch(&runtime::storage().system().account(signer.account_id().clone()))
				.await?;
			let duration = start_time.elapsed();
			prometheus::observe_balance_fetch_duration(duration.as_millis() as f64);
			Ok::<_, Error>(result)
		},
	)
	.await
	{
		Ok(Ok(Some(info))) => info,
		Ok(Ok(None)) => return Err(Error::AccountDoesNotExists),
		Ok(Err(e)) => return Err(e),
		Err(_) => {
			log::warn!(target: LOG_TARGET, "Balance fetch timed out after {BALANCE_FETCH_TIMEOUT_SECS} seconds, continuing without update");
			prometheus::on_balance_fetch_timeout();
			return Ok(());
		},
	};
	prometheus::set_balance(account_info.data.free as f64);

	// Handle different phases
	match phase {
		Phase::Snapshot(_) => {
			dynamic::fetch_missing_snapshots_lossy::<T>(snapshot, &storage, round).await?;
			return Ok(());
		},
		Phase::Signed(blocks_remaining) =>
			if blocks_remaining <= config.min_signed_phase_blocks {
				return Ok(());
			},
		_ => {
			log::trace!(target: LOG_TARGET, "Phase {phase:?} - nothing to do");
			return Ok(());
		},
	}

	// Fetch snapshots if needed
	dynamic::fetch_missing_snapshots::<T>(snapshot, &storage, round).await?;
	let (target_snapshot, voter_snapshot) = snapshot.get();

	// Check if we already submitted for this round with timeout to prevent hanging
	const CHECK_EXISTING_SUBMISSION_TIMEOUT_SECS: u64 = 300; // 5 minutes
	let already_submitted = match tokio::time::timeout(
		std::time::Duration::from_secs(CHECK_EXISTING_SUBMISSION_TIMEOUT_SECS),
		async {
			let start_time = std::time::Instant::now();
			let storage = utils::storage_at_head(&client).await?;
			let result = has_submitted(&storage, round, signer.account_id(), n_pages).await?;
			let duration = start_time.elapsed();
			prometheus::observe_check_existing_submission_duration(duration.as_millis() as f64);
			Ok::<_, Error>(result)
		},
	)
	.await
	{
		Ok(Ok(result)) => result,
		Ok(Err(e)) => return Err(e),
		Err(_) => {
			log::error!(target: LOG_TARGET, "Check existing submission timed out after {CHECK_EXISTING_SUBMISSION_TIMEOUT_SECS} seconds for round {round}");
			prometheus::on_check_existing_submission_timeout();
			return Err(Error::Timeout(CheckExistingSubmission {
				timeout_secs: CHECK_EXISTING_SUBMISSION_TIMEOUT_SECS,
			}));
		},
	};

	if already_submitted {
		log::trace!(target: LOG_TARGET, "Already submitted for round {round}, skipping");
		return Ok(());
	}

	// Mine the solution with timeout to prevent indefinite hanging
	const MINING_TIMEOUT_SECS: u64 = 600; // 10 minutes
	log::debug!(target: LOG_TARGET, "Mining solution for block #{block_number} round {round}");

	let paged_raw_solution = match tokio::time::timeout(
		std::time::Duration::from_secs(MINING_TIMEOUT_SECS),
		dynamic::mine_solution::<T>(
			target_snapshot,
			voter_snapshot,
			n_pages,
			round,
			desired_targets,
			block_number,
			config.do_reduce,
		)
		.timed(),
	)
	.await
	{
		Ok((Ok(sol), dur)) => {
			log::info!(target: LOG_TARGET, "Mining solution took {}ms for block #{}", dur.as_millis(), block_number);
			prometheus::observe_mined_solution_duration(dur.as_millis() as f64);
			sol
		},
		Ok((Err(e), dur)) => {
			log::error!(target: LOG_TARGET, "Mining failed after {}ms: {:?}", dur.as_millis(), e);
			return Err(e);
		},
		Err(_) => {
			log::error!(target: LOG_TARGET, "Mining solution timed out after {MINING_TIMEOUT_SECS} seconds for block #{block_number}");
			prometheus::on_mining_timeout();
			return Err(Error::Timeout(Mining { timeout_secs: MINING_TIMEOUT_SECS }));
		},
	};

	// Validate the solution similar to OffChainWorker logic (see
	// OffchainWorkerMiner::check_solution -> Pallet::snapshot_independent_checks in the unsigned
	// pallet). These checks prevent submitting invalid solutions on chain.

	// Ensure round is current
	if round != paged_raw_solution.round {
		log::error!(
			target: LOG_TARGET,
			"Solution validation failed: solution is for round {} but current round is {}",
			paged_raw_solution.round,
			round
		);
		return Err(Error::WrongRound {
			solution_round: paged_raw_solution.round,
			current_round: round,
		});
	}

	// Ensure solution pages are no more than the snapshot
	let solution_page_count = paged_raw_solution.solution_pages.len() as u32;
	let max_pages = static_types::Pages::get();
	if solution_page_count > max_pages {
		log::error!(
			target: LOG_TARGET,
			"Solution validation failed: solution has {solution_page_count} pages but maximum is {max_pages}"
		);
		return Err(Error::WrongPageCount { solution_pages: solution_page_count, max_pages });
	}

	// Validate that the solution has the expected number of unique targets
	let solution_winner_count =
		paged_raw_solution.winner_count_single_page_target_snapshot() as u32;
	if desired_targets != solution_winner_count {
		log::error!(
			target: LOG_TARGET,
			"Solution validation failed: desired_targets ({desired_targets}) != solution winner count ({solution_winner_count})"
		);
		return Err(Error::SolutionValidation { desired_targets, solution_winner_count });
	}

	log::debug!(
		target: LOG_TARGET,
		"Solution validation passed: desired_targets ({}) == solution winner count ({}), pages ({}) <= max ({}), round ({}) matches current ({})",
		desired_targets,
		solution_winner_count,
		solution_page_count,
		max_pages,
		paged_raw_solution.round,
		round
	);

	// Handle shady behavior if enabled
	if config.shady {
		return execute_shady_behavior(&client, &signer, &phase).await;
	}

	// Handle existing submissions with timeout to prevent indefinite hanging
	let (storage_head, existing_submission) = match tokio::time::timeout(
		std::time::Duration::from_secs(CHECK_EXISTING_SUBMISSION_TIMEOUT_SECS),
		async {
			let start_time = std::time::Instant::now();
			// Get latest storage state (chain may have progressed while we were mining)
			let storage_head = utils::storage_at_head(&client).await?;
			let existing_submission =
				get_submission(&storage_head, round, signer.account_id(), n_pages).await?;
			let duration = start_time.elapsed();
			prometheus::observe_check_existing_submission_duration(duration.as_millis() as f64);
			Ok::<_, Error>((storage_head, existing_submission))
		},
	)
	.await
	{
		Ok(result) => result?,
		Err(_) => {
			log::error!(target: LOG_TARGET, "Check existing submission timed out after {CHECK_EXISTING_SUBMISSION_TIMEOUT_SECS} seconds for block #{block_number}");
			prometheus::on_check_existing_submission_timeout();
			return Err(Error::Timeout(CheckExistingSubmission {
				timeout_secs: CHECK_EXISTING_SUBMISSION_TIMEOUT_SECS,
			}));
		},
	};

	// Handle existing submissions
	match existing_submission {
		CurrentSubmission::Done(score) => {
			// We have already submitted the solution with a score
			if !score_passes_strategy(paged_raw_solution.score, score, config.submission_strategy) {
				log::debug!(target: LOG_TARGET, "Our new score doesn't beat existing submission, skipping");
				return Ok(());
			}
			log::debug!(target: LOG_TARGET, "Reverting previous submission to submit better solution");

			// Verify we're still in signed phase before bailing with timeout
			const PHASE_CHECK_TIMEOUT_SECS: u64 = 60; // 1 minute
			let still_signed = match tokio::time::timeout(
				std::time::Duration::from_secs(PHASE_CHECK_TIMEOUT_SECS),
				async {
					let start_time = std::time::Instant::now();
					let result = signed_phase(&client).await?;
					let duration = start_time.elapsed();
					prometheus::observe_phase_check_duration(duration.as_millis() as f64);
					Ok::<_, Error>(result)
				},
			)
			.await
			{
				Ok(Ok(result)) => result,
				Ok(Err(e)) => return Err(e),
				Err(_) => {
					log::error!(target: LOG_TARGET, "Phase check timed out after {PHASE_CHECK_TIMEOUT_SECS} seconds for round {round}");
					prometheus::on_phase_check_timeout();
					return Err(Error::Timeout(PhaseCheck {
						timeout_secs: PHASE_CHECK_TIMEOUT_SECS,
					}));
				},
			};

			if !still_signed {
				log::warn!(target: LOG_TARGET, "Phase changed, cannot bail existing submission");
				return Ok(());
			}

			// Bail operation with timeout to prevent indefinite hanging
			const BAIL_TIMEOUT_SECS: u64 = 120; // 2 minutes
			match tokio::time::timeout(std::time::Duration::from_secs(BAIL_TIMEOUT_SECS), async {
				let start_time = std::time::Instant::now();
				dynamic::bail(&client, &signer).await?;
				let duration = start_time.elapsed();
				prometheus::observe_bail_duration(duration.as_millis() as f64);
				Ok::<_, Error>(())
			})
			.await
			{
				Ok(result) => result?,
				Err(_) => {
					log::error!(target: LOG_TARGET, "Bail operation timed out after {BAIL_TIMEOUT_SECS} seconds for block #{block_number}");
					prometheus::on_bail_timeout();
					return Err(Error::Timeout(Bail { timeout_secs: BAIL_TIMEOUT_SECS }));
				},
			};
		},
		CurrentSubmission::Incomplete(s) => {
			if s.score() == paged_raw_solution.score {
				// Same score, just submit missing pages
				let missing_pages: Vec<(u32, T::Solution)> = s
					.get_missing_pages()
					.map(|page| (page, paged_raw_solution.solution_pages[page as usize].clone()))
					.collect();

				log::info!(target: LOG_TARGET, "Submitting {} missing pages for existing submission", missing_pages.len());

				// Submit missing pages with timeout to prevent indefinite hanging
				const MISSING_PAGES_TIMEOUT_SECS: u64 = 1800; // 30 minutes
				match tokio::time::timeout(
					std::time::Duration::from_secs(MISSING_PAGES_TIMEOUT_SECS),
					async {
						let start_time = std::time::Instant::now();
						if config.chunk_size == 0 {
							dynamic::inner_submit_pages_concurrent::<T>(
								&client,
								&signer,
								missing_pages,
								round,
								config.min_signed_phase_blocks,
							)
							.await?;
						} else {
							dynamic::inner_submit_pages_chunked::<T>(
								&client,
								&signer,
								missing_pages,
								config.chunk_size,
								round,
								config.min_signed_phase_blocks,
							)
							.await?;
						}
						let duration = start_time.elapsed();
						prometheus::observe_missing_pages_duration(duration.as_millis() as f64);
						Ok::<_, Error>(())
					},
				)
				.await
				{
					Ok(Ok(())) => {},
					Ok(Err(e)) => return Err(e),
					Err(_) => {
						log::error!(target: LOG_TARGET, "Missing pages submission timed out after {MISSING_PAGES_TIMEOUT_SECS} seconds");
						prometheus::on_missing_pages_timeout();
						return Err(Error::Timeout(MissingPages {
							timeout_secs: MISSING_PAGES_TIMEOUT_SECS,
						}));
					},
				}
				return Ok(());
			}

			log::debug!(target: LOG_TARGET, "Reverting incomplete submission to submit new solution");

			// Verify we're still in signed phase before bailing with timeout
			const PHASE_CHECK_TIMEOUT_SECS: u64 = 60; // 1 minute
			let still_signed = match tokio::time::timeout(
				std::time::Duration::from_secs(PHASE_CHECK_TIMEOUT_SECS),
				async {
					let start_time = std::time::Instant::now();
					let result = signed_phase(&client).await?;
					let duration = start_time.elapsed();
					prometheus::observe_phase_check_duration(duration.as_millis() as f64);
					Ok::<_, Error>(result)
				},
			)
			.await
			{
				Ok(Ok(result)) => result,
				Ok(Err(e)) => return Err(e),
				Err(_) => {
					log::error!(target: LOG_TARGET, "Phase check timed out after {PHASE_CHECK_TIMEOUT_SECS} seconds for round {round}");
					prometheus::on_phase_check_timeout();
					return Err(Error::Timeout(PhaseCheck {
						timeout_secs: PHASE_CHECK_TIMEOUT_SECS,
					}));
				},
			};

			if !still_signed {
				log::warn!(target: LOG_TARGET, "Phase changed, cannot bail incomplete submission");
				return Ok(());
			}

			// Bail operation with timeout to prevent indefinite hanging
			const BAIL_TIMEOUT_SECS: u64 = 120; // 2 minutes
			match tokio::time::timeout(std::time::Duration::from_secs(BAIL_TIMEOUT_SECS), async {
				let start_time = std::time::Instant::now();
				dynamic::bail(&client, &signer).await?;
				let duration = start_time.elapsed();
				prometheus::observe_bail_duration(duration.as_millis() as f64);
				Ok::<_, Error>(())
			})
			.await
			{
				Ok(result) => result?,
				Err(_) => {
					log::error!(target: LOG_TARGET, "Bail operation timed out after {BAIL_TIMEOUT_SECS} seconds for block #{block_number}");
					prometheus::on_bail_timeout();
					return Err(Error::Timeout(Bail { timeout_secs: BAIL_TIMEOUT_SECS }));
				},
			};
		},
		CurrentSubmission::NotStarted => {
			log::debug!(target: LOG_TARGET, "No existing submission found");
		},
	};

	// Check if our score is competitive (skip this check in shady mode) with timeout
	if !should_bypass_competitive_check(config.shady) {
		const SCORE_CHECK_TIMEOUT_SECS: u64 = 60; // 1 minute
		let is_competitive = match tokio::time::timeout(
			std::time::Duration::from_secs(SCORE_CHECK_TIMEOUT_SECS),
			async {
				let start_time = std::time::Instant::now();
				let result = score_better(
					&storage_head,
					paged_raw_solution.score,
					round,
					config.submission_strategy,
				)
				.await?;
				let duration = start_time.elapsed();
				prometheus::observe_score_check_duration(duration.as_millis() as f64);
				Ok::<_, Error>(result)
			},
		)
		.await
		{
			Ok(Ok(result)) => result,
			Ok(Err(e)) => return Err(e),
			Err(_) => {
				log::error!(target: LOG_TARGET, "Score check timed out after {SCORE_CHECK_TIMEOUT_SECS} seconds for round {round}");
				prometheus::on_score_check_timeout();
				return Err(Error::Timeout(ScoreCheck { timeout_secs: SCORE_CHECK_TIMEOUT_SECS }));
			},
		};

		if !is_competitive {
			log::debug!(target: LOG_TARGET, "Our score is not competitive, skipping submission");
			return Ok(());
		}
	}

	prometheus::set_score(paged_raw_solution.score);
	log::info!(target: LOG_TARGET, "Submitting solution with score {:?} for round {}", paged_raw_solution.score, round);

	// Submit the solution with timeout to prevent indefinite hanging
	const SUBMIT_TIMEOUT_SECS: u64 = 1800; // 30 minutes
	match tokio::time::timeout(
		std::time::Duration::from_secs(SUBMIT_TIMEOUT_SECS),
		dynamic::submit(
			&client,
			&signer,
			paged_raw_solution,
			config.chunk_size,
			round,
			config.min_signed_phase_blocks,
		)
		.timed(),
	)
	.await
	{
		Ok((Ok(_), dur)) => {
			log::info!(
				target: LOG_TARGET,
				"Successfully submitted solution for round {} in {}ms",
				round,
				dur.as_millis()
			);
			prometheus::observe_submit_and_watch_duration(dur.as_millis() as f64);
		},
		Ok((Err(e), dur)) => {
			log::error!(
				target: LOG_TARGET,
				"Submission failed after {}ms: {:?}",
				dur.as_millis(),
				e
			);
			return Err(e);
		},
		Err(_) => {
			log::error!(target: LOG_TARGET, "Submit operation timed out after {SUBMIT_TIMEOUT_SECS} seconds");
			prometheus::on_submit_timeout();
			return Err(Error::Timeout(Submit { timeout_secs: SUBMIT_TIMEOUT_SECS }));
		},
	};

	Ok(())
}

/// Whether the computed score is better than the current best score
async fn score_better(
	storage: &Storage,
	score: ElectionScore,
	round: u32,
	submission_strategy: SubmissionStrategy,
) -> Result<bool, Error> {
	let scores = storage
		.fetch_or_default(&runtime::storage().multi_block_election_signed().sorted_scores(round))
		.await?;

	if scores
		.0
		.into_iter()
		.any(|(_, other_score)| !score_passes_strategy(score, other_score.0, submission_strategy))
	{
		return Ok(false);
	}

	Ok(true)
}

/// Whether the current account has registered the score and submitted all pages for the given
/// round.
async fn get_submission(
	storage: &Storage,
	round: u32,
	who: &subxt::config::substrate::AccountId32,
	n_pages: u32,
) -> Result<CurrentSubmission, Error> {
	let maybe_submission = storage
		.fetch(
			&runtime::storage()
				.multi_block_election_signed()
				.submission_metadata_storage(round, who.clone()),
		)
		.await?;

	let Some(submission) = maybe_submission else {
		return Ok(CurrentSubmission::NotStarted);
	};

	let pages: HashSet<u32> = submission
		.pages
		.0
		.into_iter()
		.enumerate()
		.filter_map(|(i, submitted)| if submitted { Some(i as u32) } else { None })
		.collect();

	if pages.len() == n_pages as usize {
		Ok(CurrentSubmission::Done(submission.claimed_score.0))
	} else {
		Ok(CurrentSubmission::Incomplete(IncompleteSubmission::new(
			submission.claimed_score.0,
			pages,
			n_pages,
		)))
	}
}

/// Whether the current account has registered the score and submitted all pages for the given
/// round.
async fn has_submitted(
	storage: &Storage,
	round: u32,
	who: &subxt::config::substrate::AccountId32,
	n_pages: u32,
) -> Result<bool, Error> {
	match get_submission(storage, round, who, n_pages).await? {
		CurrentSubmission::Done(_) => Ok(true),
		_ => Ok(false),
	}
}

/// Execute shady behavior: register malicious max score without submitting pages
async fn execute_shady_behavior(
	client: &Client,
	signer: &Signer,
	phase: &Phase,
) -> Result<(), Error> {
	log::warn!(target: LOG_TARGET, "🔥 SHADY MODE: Registering malicious max score with no page submission!");

	// Get blocks remaining for shady behavior mortality
	let blocks_remaining = match phase {
		Phase::Signed(remaining) => *remaining,
		_ => {
			log::error!(target: LOG_TARGET, "Shady behavior attempted but not in SignedPhase: {phase:?}");
			return Err(Error::Other("Not in SignedPhase for shady behavior".to_string()));
		},
	};

	// Create a malicious score with max values
	let malicious_score = ElectionScore {
		minimal_stake: u128::MAX,
		sum_stake: u128::MAX,
		sum_stake_squared: u128::MAX,
	};

	// Just register the score and return - no page submission
	// TODO: In the future, we might want to add more variants, such as register score - submit an
	// invalid page (to simulate an early failure in the validation)
	let mut i = 0;
	let tx_status = loop {
		let nonce = client.chain_api().tx().account_nonce(signer.account_id()).await?;

		// Register score only
		match dynamic::submit_inner(
			client,
			signer.clone(),
			dynamic::MultiBlockTransaction::register_score(malicious_score)?,
			nonce,
			blocks_remaining,
		)
		.await
		{
			Ok(tx) => break tx,
			Err(Error::Subxt(boxed_err))
				if matches!(boxed_err.as_ref(), subxt::Error::Transaction(_)) =>
			{
				i += 1;
				if i >= 10 {
					return Err(Error::Subxt(boxed_err));
				}
				log::debug!(target: LOG_TARGET, "Failed to register malicious score: {boxed_err:?}; retrying");
				tokio::time::sleep(std::time::Duration::from_secs(6)).await;
			},
			Err(e) => return Err(e),
		}
	};

	// Wait for the malicious score registration to be included
	let tx = utils::wait_tx_in_finalized_block(tx_status).await?;
	let events = tx.wait_for_success().await?;
	if !events.has::<runtime::multi_block_election_signed::events::Registered>()? {
		return Err(Error::MissingTxEvent("Register malicious score".to_string()));
	};

	log::warn!(target: LOG_TARGET, "🔥 SHADY MODE: Malicious max score registered successfully at block {:?} - NO PAGES WILL BE SUBMITTED!", tx.block_hash());
	prometheus::set_score(malicious_score);
	Ok(())
}

/// Check if competitive score validation should be bypassed
fn should_bypass_competitive_check(shady: bool) -> bool {
	if shady {
		log::warn!(target: LOG_TARGET, "🔥 SHADY MODE: Bypassing competitive score check!");
		true
	} else {
		false
	}
}

/// Run clear old rounds cleanup to reclaim deposits from old discarded submissions
///
/// This function scans the last few previous rounds looking for submissions from our account
/// that were discarded (not selected as winners). We only check the last few rounds because:
/// 1. The miner is expected to run 24/7, so in the happy path we only need to clean up the previous
///    round (if any cleanup is needed)
/// 2. If the miner was offline for a few rounds, CLEAR_OLD_ROUNDS_SCAN_ROUNDS rounds provides a
///    reasonable buffer
/// 3. Checking all previous rounds back to 0 would be inefficient and pointless since submissions
///    older than a few rounds are likely already cleaned up.
///
/// For each found submission, it calls `clear_old_round_data()` to:
/// - Remove the stale submission data from chain storage
/// - Reclaim the deposit that was locked for the submission
/// - Clean up blockchain state for better chain health
///
/// # Returns
/// * `Ok(count)` - Number of old submissions successfully cleaned up
/// * `Err(error)` - If a critical error occurred during cleanup
///
/// # Notes
/// * Only submissions from previous rounds can be cleaned (round < current_round)
/// * Only the original submitter can clean their own submissions
/// * This is safe to call repeatedly - it will skip already cleaned submissions
/// * Non-critical errors (like already cleaned submissions) are logged but don't fail the operation
async fn run_clear_old_rounds_cleanup<T>(
	client: Client,
	signer: Signer,
	current_round: u32,
	last_scanned_round: &mut Option<u32>,
) -> Result<u32, Error>
where
	T: MinerConfig<AccountId = AccountId> + Send + Sync + 'static,
	T::Solution: Send + Sync + 'static,
	T::Pages: Send + Sync + 'static,
	T::TargetSnapshotPerBlock: Send + Sync + 'static,
	T::VoterSnapshotPerBlock: Send + Sync + 'static,
	T::MaxVotesPerVoter: Send + Sync + 'static,
{
	let storage = utils::storage_at_head(&client).await?;
	let mut cleaned_count = 0u32;
	let mut found_count = 0u32;

	// Determine the starting round to avoid rescanning
	let start_round = match last_scanned_round {
		Some(last) => {
			// Start from the round after the last scanned, but ensure we don't go beyond
			// the scan window
			let min_round = current_round.saturating_sub(CLEAR_OLD_ROUNDS_SCAN_ROUNDS);
			(*last + 1).max(min_round)
		},
		None => {
			// First run - scan the full window
			current_round.saturating_sub(CLEAR_OLD_ROUNDS_SCAN_ROUNDS)
		},
	};

	// Handle edge case where current_round is 0 (no previous rounds exist)
	if current_round == 0 {
		log::trace!(target: LOG_TARGET, "Current round is 0, no previous rounds to clean");
		return Ok(0);
	}

	// Skip if there's nothing new to scan
	if start_round >= current_round {
		log::trace!(target: LOG_TARGET, "No new rounds to scan (start: {start_round}, current: {current_round})");
		return Ok(0);
	}

	for old_round in start_round..current_round {
		log::trace!(target: LOG_TARGET, "Scanning round {old_round} for old submissions");

		// Check if we have a submission for this old round
		let maybe_submission = storage
			.fetch(
				&runtime::storage()
					.multi_block_election_signed()
					.submission_metadata_storage(old_round, signer.account_id().clone()),
			)
			.await?;

		if let Some(submission) = maybe_submission {
			found_count += 1;
			log::debug!(
				target: LOG_TARGET,
				"Found old submission in round {} with {} pages, attempting cleanup",
				old_round,
				submission.pages.0.len()
			);

			// Calculate witness pages - count the number of true values in the pages bitfield
			let witness_pages =
				submission.pages.0.iter().filter(|&&submitted| submitted).count() as u32;

			if witness_pages == 0 {
				log::warn!(
					target: LOG_TARGET,
					"Skipping cleanup for round {old_round} - no pages were submitted"
				);
				continue;
			}

			// Attempt to clear the old round data
			match clear_old_round_data(&client, &signer, old_round, witness_pages).await {
				Ok(()) => {
					cleaned_count += 1;
					log::info!(
						target: LOG_TARGET,
						"Successfully cleaned up old submission from round {old_round} ({witness_pages} witness pages)"
					);
				},
				Err(e) => {
					log::warn!(
						target: LOG_TARGET,
						"Failed to clean up old submission from round {old_round}: {e:?}"
					);
				},
			}
		}
	}

	prometheus::set_clear_old_rounds_old_submissions_found(found_count);

	// Update the last scanned round to current_round - 1 (since we scanned up to but not including
	// current_round)
	*last_scanned_round = Some(current_round.saturating_sub(1));

	Ok(cleaned_count)
}

/// Clear old round data to reclaim deposits
///
/// This function submits a `clear_old_round_data` extrinsic to the blockchain to clean up
/// stale submission data from a previous election round and reclaim the associated deposit.
async fn clear_old_round_data(
	client: &Client,
	signer: &Signer,
	round: u32,
	witness_pages: u32,
) -> Result<(), Error> {
	log::debug!(
		target: LOG_TARGET,
		"Clearing old round data for round {round} with {witness_pages} witness pages"
	);

	// Construct the extrinsic call using the static types from the runtime module
	let tx = runtime::tx()
		.multi_block_election_signed()
		.clear_old_round_data(round, witness_pages);

	let nonce = client.chain_api().tx().account_nonce(signer.account_id()).await?;
	let xt_cfg = ExtrinsicParamsBuilder::default().nonce(nonce).build();
	let xt = client.chain_api().tx().create_signed(&tx, &**signer, xt_cfg).await?;

	// Submit without waiting for finalization to avoid blocking (fire-and-forget approach)
	// This prevents potential resource contention with listener task's storage queries
	let tx_hash = xt.submit().await?;

	log::debug!(target: LOG_TARGET, "Successfully submitted clear_old_round_data for round {round}, tx_hash: {tx_hash:?}");
	log::info!(target: LOG_TARGET, "Clearing old rounds: Fire-and-forget submission for round {round} cleanup");
	Ok(())
}

/// Determine if a miner error is critical and should cause the process to exit
fn is_critical_miner_error(error: &Error) -> bool {
	match error {
		Error::Join(_) |
		Error::Feasibility(_) |
		Error::EmptySnapshot |
		Error::FailedToSubmitPages(_) |
		Error::SolutionValidation { .. } |
		Error::WrongPageCount { .. } |
		Error::WrongRound { .. } |
		Error::Timeout(_) => false,
		Error::Subxt(boxed_err) if matches!(boxed_err.as_ref(), subxt::Error::Runtime(_)) => false, /* e.g. Subxt(Runtime(Module(ModuleError(<MultiBlockElectionSigned::Duplicate>)))) */
		Error::Subxt(boxed_err) if matches!(boxed_err.as_ref(), subxt::Error::Transaction(_)) =>
			false, /* e.g. Subxt(Transaction(Invalid("Transaction is invalid (eg because of a bad */
		// nonce, signature etc)"))))
		Error::Subxt(boxed_err) if matches!(boxed_err.as_ref(), subxt::Error::Rpc(_)) => false, /* e.g. Subxt(Rpc(ClientError(User(UserError { code: -32801, message: "Invalid block hash" })))) */
		// Phase timing errors should not be critical - these are expected conditions
		Error::InsufficientSignedPhaseBlocks { .. } => false,
		Error::PhaseChangedDuringSubmission { .. } => false,
		// Everything else we consider it critical e.g.
		//  - Error::AccountDoesNotExists
		//  - Error::InvalidMetadata(_)
		//  - Error::InvalidChain(_)
		// - Error::Rpc(_) i.e. persistent RPC failures (after reconnecting client gave up)
		// - Error::Subxt(_) i.e. any other subxt error
		_ => true,
	}
}
