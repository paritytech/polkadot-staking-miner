use crate::{
	client::Client,
	commands::{
		multi_block::types::{BlockDetails, CurrentSubmission, IncompleteSubmission, Snapshot},
		types::{MultiBlockMonitorConfig, SubmissionStrategy},
	},
	dynamic::multi_block as dynamic,
	error::Error,
	prelude::{AccountId, ExtrinsicParamsBuilder, LOG_TARGET, Storage},
	prometheus,
	runtime::multi_block::{
		self as runtime, runtime_types::pallet_election_provider_multi_block::types::Phase,
	},
	signer::Signer,
	static_types::multi_block as static_types,
	utils::{self, TimedFuture, score_passes_strategy},
};
use polkadot_sdk::{
	pallet_election_provider_multi_block::unsigned::miner::MinerConfig,
	sp_npos_elections::ElectionScore,
};
use std::collections::HashSet;

use subxt::backend::StreamOf;
use tokio::sync::mpsc;

/// Number of previous rounds to scan for old submissions during janitor cleanup.
/// This provides a reasonable buffer for offline periods while avoiding inefficient
/// scanning of potentially gazillions of historical rounds.
const JANITOR_SCAN_ROUNDS: u32 = 5;

/// Timeout in seconds for detecting stalled block subscriptions.
/// If no blocks are received within this duration, the subscription will be recreated.
const BLOCK_SUBSCRIPTION_TIMEOUT_SECS: u64 = 60;

/// Timeout in seconds for detecting stalled block processing.
/// If block processing takes longer than this, we'll recreate the subscription.
const BLOCK_PROCESSING_TIMEOUT_SECS: u64 = 90;

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
	prev_round: &mut Option<u32>,
	miner_tx: &mpsc::Sender<MinerMessage>,
	janitor_tx: &mpsc::Sender<JanitorMessage>,
) -> Result<ListenerAction, Error>
where
	T: MinerConfig<AccountId = AccountId> + Send + Sync,
	T::Solution: Send + Sync,
	T::Pages: Send + Sync,
	T::TargetSnapshotPerBlock: Send + Sync,
	T::VoterSnapshotPerBlock: Send + Sync,
	T::MaxVotesPerVoter: Send + Sync,
{
	log::trace!(target: LOG_TARGET, "Listener: Getting block state for block #{}", at.number);
	let block_state_start = std::time::Instant::now();
	let (_storage, phase, current_round) = get_block_state(client, block_hash).await?;
	let block_state_duration = block_state_start.elapsed();
	log::trace!(target: LOG_TARGET, "Listener: Got block state in {}ms for block #{}, phase: {:?}, round: {}", block_state_duration.as_millis(), at.number, phase, current_round);
	prometheus::observe_block_state_duration(block_state_duration.as_millis() as f64);

	// Update prev_round before processing round increment to avoid stale round detection in case of
	// timeouts
	let last_round = *prev_round;
	*prev_round = Some(current_round);

	if let Some(last_round) = last_round {
		if current_round > last_round {
			log::trace!(target: LOG_TARGET, "Listener: Processing round increment from {} to {} for block #{}", last_round, current_round, at.number);
			let round_increment_start = std::time::Instant::now();
			on_round_increment(last_round, current_round, &phase, miner_tx, janitor_tx).await?;
			let round_increment_duration = round_increment_start.elapsed();
			log::trace!(target: LOG_TARGET, "Listener: Round increment processing took {}ms for block #{}", round_increment_duration.as_millis(), at.number);
		}
	}
	let block_number = at.number;

	match phase {
		Phase::Signed(_) | Phase::Snapshot(_) => {
			// Relevant phases for mining - continue processing
		},
		_ => {
			log::trace!(target: LOG_TARGET, "Block #{block_number}, Phase {phase:?} - nothing to do");
			return Ok(ListenerAction::Continue);
		},
	}

	log::trace!(target: LOG_TARGET, "Listener: Creating BlockDetails for block #{block_number}");
	let block_details_start = std::time::Instant::now();
	let state = BlockDetails::new(client, at, phase, block_hash, current_round).await?;
	let block_details_duration = block_details_start.elapsed();
	log::trace!(target: LOG_TARGET, "Listener: BlockDetails creation took {}ms for block #{}", block_details_duration.as_millis(), block_number);
	prometheus::observe_block_details_duration(block_details_duration.as_millis() as f64);

	let message = MinerMessage::ProcessBlock { state };

	// Use try_send for backpressure - if miner is busy, skip this block
	match miner_tx.try_send(message) {
		Ok(()) => {
			log::trace!(target: LOG_TARGET, "Sent block #{block_number} to miner");
			// Update timestamp of successful block processing
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
			return Err(Error::Other("Miner channel closed unexpectedly".to_string()));
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
	prev_round: &mut Option<u32>,
	last_block_time: &mut std::time::Instant,
	miner_tx: &mpsc::Sender<MinerMessage>,
	janitor_tx: &mpsc::Sender<JanitorMessage>,
) -> Result<ListenerAction, Error>
where
	T: MinerConfig<AccountId = AccountId> + Send + Sync,
	T::Solution: Send + Sync,
	T::Pages: Send + Sync,
	T::TargetSnapshotPerBlock: Send + Sync,
	T::VoterSnapshotPerBlock: Send + Sync,
	T::MaxVotesPerVoter: Send + Sync,
{
	log::trace!(target: LOG_TARGET, "Listener: Waiting for next block from subscription...");
	let (at, block_hash) = match tokio::time::timeout(
		std::time::Duration::from_secs(BLOCK_SUBSCRIPTION_TIMEOUT_SECS),
		subscription.next(),
	)
	.await
	{
		Ok(maybe_block) => {
			match maybe_block {
				Some(Ok(block)) => {
					*last_block_time = std::time::Instant::now();
					log::trace!(target: LOG_TARGET, "Listener: Received block #{} from subscription", block.header().number);
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
			// Recreate the subscription
			match client.chain_api().blocks().subscribe_finalized().await {
				Ok(new_subscription) => {
					*subscription = new_subscription;
					*last_block_time = std::time::Instant::now();
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
	log::trace!(target: LOG_TARGET, "Listener: Starting block processing with timeout for block #{}", at.number);
	match tokio::time::timeout(
		std::time::Duration::from_secs(BLOCK_PROCESSING_TIMEOUT_SECS),
		process_block_internal::<T>(
			client,
			at.clone(),
			block_hash,
			prev_round,
			miner_tx,
			janitor_tx,
		),
	)
	.await
	{
		Ok(result) => result,
		Err(_) => {
			log::warn!(target: LOG_TARGET, "Block processing timed out after {}s for block #{} - may indicate internal hang, recreating subscription...", BLOCK_PROCESSING_TIMEOUT_SECS, at.number);
			prometheus::on_block_processing_stall();
			match client.chain_api().blocks().subscribe_finalized().await {
				Ok(new_subscription) => {
					*subscription = new_subscription;
					*last_block_time = std::time::Instant::now();
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
		// Channel errors are critical - indicates miner task has died
		Error::Other(msg) if msg.contains("channel closed") => true,
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
	log::trace!(target: LOG_TARGET, "get_block_state: Getting storage for block {block_hash:?}");
	let storage_start = std::time::Instant::now();
	let storage = utils::storage_at(Some(block_hash), client.chain_api()).await?;
	let storage_duration = storage_start.elapsed();
	log::trace!(target: LOG_TARGET, "get_block_state: Got storage in {}ms", storage_duration.as_millis());
	prometheus::observe_storage_query_duration(storage_duration.as_millis() as f64);

	log::trace!(target: LOG_TARGET, "get_block_state: Fetching current_phase");
	let phase_start = std::time::Instant::now();
	let phase = storage
		.fetch_or_default(&runtime::storage().multi_block_election().current_phase())
		.await?;
	let phase_duration = phase_start.elapsed();
	log::trace!(target: LOG_TARGET, "get_block_state: Got current_phase in {}ms: {:?}", phase_duration.as_millis(), phase);
	prometheus::observe_storage_query_duration(phase_duration.as_millis() as f64);

	log::trace!(target: LOG_TARGET, "get_block_state: Fetching round");
	let round_start = std::time::Instant::now();
	let current_round = storage
		.fetch_or_default(&runtime::storage().multi_block_election().round())
		.await?;
	let round_duration = round_start.elapsed();
	log::trace!(target: LOG_TARGET, "get_block_state: Got round in {}ms: {}", round_duration.as_millis(), current_round);
	prometheus::observe_storage_query_duration(round_duration.as_millis() as f64);

	Ok((storage, phase, current_round))
}

/// Handle round increment by triggering janitor and snapshot cleanup
async fn on_round_increment(
	last_round: u32,
	current_round: u32,
	phase: &Phase,
	miner_tx: &mpsc::Sender<MinerMessage>,
	janitor_tx: &mpsc::Sender<JanitorMessage>,
) -> Result<(), Error> {
	log::debug!(target: LOG_TARGET, "Detected round increment {last_round} -> {current_round}");

	// 1. Trigger janitor cleanup
	if let Err(e) = janitor_tx.try_send(JanitorMessage::JanitorTick { current_round }) {
		match e {
			mpsc::error::TrySendError::Full(_) => {
				// this shouldn't happen. Janitor is triggered once per round.
				// If it's still busy after a round, it means that it has taken
				// insanely long.
				log::warn!(target: LOG_TARGET, "Janitor busy, skipping janitor tick for round {current_round}.");
			},
			mpsc::error::TrySendError::Closed(_) => {
				log::error!(target: LOG_TARGET, "Janitor channel closed unexpectedly during janitor tick");
				return Err(Error::Other("Janitor channel closed unexpectedly".to_string()));
			},
		}
	} else {
		log::trace!(target: LOG_TARGET, "Sent janitor tick for round {current_round}");
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

/// Message types for communication between listener and janitor
enum JanitorMessage {
	/// Signal to run janitor cleanup for old submissions
	JanitorTick { current_round: u32 },
}

/// The monitor command splits the work into three communicating tasks:
///
/// ### Listener Task
/// - Always follows the chain (head or finalized blocks)
/// - Performs fast phase checks to identify relevant blocks (Snapshot or Signed)
/// - Checks if miner is busy before sending work
/// - Never blocks on slow operations
/// - **Any error causes entire process to exit** (RPC errors handled by reconnecting client)
/// - Triggers janitor cleanup and snapshot clearing when round increments
///
/// ### Miner Task
/// - Single-threaded processor that handles one block at a time
/// - **Mining**: Processes Snapshot/Signed phases for solution mining and submission
/// - **Cleanup**: Clears snapshots when round increments (in Off phase)
/// - **Critical errors cause process exit** (e.g. account doesn't exist)
/// - **Non-critical errors are recoverable** (log and continue)
///
/// ### Janitor Task
/// - Independent task that handles deposit recovery operations
/// - Automatically scans for and cleans up old submissions from previous rounds
/// - Reclaims deposits from discarded solutions that were not selected
/// - Runs exactly once per round when round number increments
/// - Scans only the last JANITOR_SCAN_ROUNDS rounds for old submissions
/// - Calls `clear_old_round_data()` to remove stale data and recover deposits
/// - **Non-blocking**: Does not interfere with mining operations
///
/// ### Communication
/// - **Miner Channel**: Bounded channel (buffer=1) for mining work + snapshot cleanup
/// - **Janitor Channel**: Bounded channel (buffer=1) for deposit recovery operations
/// - **Dual Triggers**: Listener sends mining work (Snapshot/Signed) + cleanup signals (round++)
/// - **Backpressure mechanism**: When miner is busy processing a block:
///   - Channel becomes full, listener's try_send() returns immediately
///   - Listener skips current block and moves to next (fresher) block
///   - No blocking, no buffering of stale work
/// - **Separation of concerns**: Mining and janitor operations are completely independent
/// - No submission locks needed since miner is single-threaded by design
///
/// ## Error Handling Strategy
/// - **Listener errors**: All critical - process exits immediately
/// - **Miner errors**: Classified as critical or recoverable
///   - Critical: Account doesn't exist, invalid metadata, persistent RPC failures
///   - Recoverable: Already submitted, wrong phase, temporary mining issues
/// - **Janitor errors**: Classified as critical or recoverable (same as miner)
/// - **RPC issues**: Handled transparently by reconnecting RPC client
///
/// ```text
/// (finalized blocks)
/// ┌──────────────────────────────────────────────────────────────────────────┐
/// │   ┌─────────────┐                      ┌─────────────┐            ┌─────────────┐
/// └──▶│ Listener    │                      │   Miner     │            │ Blockchain  │
///     │             │  Snapshot/Signed     │             │            │             │
///     │ ┌─────────┐ │ ────────────────────▶│ ┌─────────┐ │ (solutions)│             │
///     │ │ Stream  │ │  (mining work)       │ │ Mining  │ │───────────▶│             │
///     │ └─────────┘ │                      │ └─────────┘ │            │             │
///     │      │      │  Round++             │ ┌─────────┐ │            │             │
///     │      ▼      │ ────────────────────▶│ │ Clear   │ │            │             │
///     │ ┌─────────┐ │                      │ │ Snapshot│ │            │             │
///     │ │ Phase   │ │                      │ └─────────┘ │            │             │
///     │ │ Check   │ │  Round++             └─────────────┘            │             │
///     │ └─────────┘ │ ────────────────────▶┌─────────────┐            │             │
///     │             │  (deposit cleanup)   │  Janitor    │ (cleanup)  │             │
///     │             │                      │ ┌─────────┐ │───────────▶│             │
///     │             │                      │ │ Cleanup │ │            │             │
///     │             │                      │ └─────────┘ │            │             │
///     └─────────────┘                      └─────────────┘            └─────────────┘
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

	// Create bounded channel for communication between listener and janitor
	// Buffer size of 1 is sufficient since janitor runs once at every round change.
	let (janitor_tx, janitor_rx) = mpsc::channel::<JanitorMessage>(1);

	// Spawn the miner task
	let miner_handle = {
		let client = client.clone();
		let signer = signer.clone();
		let config = config.clone();
		tokio::spawn(async move { miner_task::<T>(client, signer, config, miner_rx).await })
	};

	// Spawn the janitor task
	let janitor_handle = {
		let client = client.clone();
		let signer = signer.clone();
		tokio::spawn(async move { janitor_task::<T>(client, signer, janitor_rx).await })
	};

	// Spawn the listener task
	let listener_handle = {
		let client = client.clone();
		tokio::spawn(async move { listener_task::<T>(client, miner_tx, janitor_tx).await })
	};

	// Wait for any task to complete (which should never happen in normal operation)
	// If any task exits, the whole process should exit
	tokio::select! {
		result = listener_handle => {
			match result {
				Ok(Ok(())) => {
					log::error!(target: LOG_TARGET, "Listener task completed unexpectedly");
					Err(Error::Other("Listener task should never complete".to_string()))
				}
				Ok(Err(e)) => {
					log::error!(target: LOG_TARGET, "Listener task failed: {e}");
					Err(e)
				}
				Err(e) => {
					log::error!(target: LOG_TARGET, "Listener task panicked: {e}");
					Err(Error::Other(format!("Listener task panicked: {e}")))
				}
			}
		}
		result = miner_handle => {
			match result {
				Ok(Ok(())) => {
					log::error!(target: LOG_TARGET, "Miner task completed unexpectedly");
					Err(Error::Other("Miner task should never complete".to_string()))
				}
				Ok(Err(e)) => {
					log::error!(target: LOG_TARGET, "Miner task failed: {e}");
					Err(e)
				}
				Err(e) => {
					log::error!(target: LOG_TARGET, "Miner task panicked: {e}");
					Err(Error::Other(format!("Miner task panicked: {e}")))
				}
			}
		}
		result = janitor_handle => {
			match result {
				Ok(Ok(())) => {
					log::error!(target: LOG_TARGET, "Janitor task completed unexpectedly");
					Err(Error::Other("Janitor task should never complete".to_string()))
				}
				Ok(Err(e)) => {
					log::error!(target: LOG_TARGET, "Janitor task failed: {e}");
					Err(e)
				}
				Err(e) => {
					log::error!(target: LOG_TARGET, "Janitor task panicked: {e}");
					Err(Error::Other(format!("Janitor task panicked: {e}")))
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
///   - transition from Done/Export(0) to Off to inform the janitor task
/// - Using backpressure to skip blocks when miner is busy
/// - Managing phase transitions and snapshot cleanup
/// - Never blocking on slow operations to avoid subscription buffering
/// - Any error causes the entire process to exit
async fn listener_task<T>(
	client: Client,
	miner_tx: mpsc::Sender<MinerMessage>,
	janitor_tx: mpsc::Sender<JanitorMessage>,
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
	let mut prev_round: Option<u32> = None;
	let mut last_block_time = std::time::Instant::now();

	log::trace!(target: LOG_TARGET, "Listener task started, watching for finalized blocks");

	loop {
		match process_listener_iteration::<T>(
			&client,
			&mut subscription,
			&mut prev_round,
			&mut last_block_time,
			&miner_tx,
			&janitor_tx,
		)
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
				let result = process_block::<T>(
					client.clone(),
					state,
					&mut snapshot,
					signer.clone(),
					process_config,
				)
				.await;

				match result {
					Ok(()) => {
						log::trace!(target: LOG_TARGET, "Block processing completed successfully");
					},
					Err(e) =>
						if is_critical_miner_error(&e) {
							log::error!(target: LOG_TARGET, "Critical miner error - process will exit: {e:?}");
							return Err(e);
						} else {
							log::warn!(target: LOG_TARGET, "Block processing failed, continuing: {e:?}");
						},
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
	Err(Error::Other("Miner task completed unexpectedly".to_string()))
}

/// Janitor task - handles deposit recovery operations independently from mining
///
/// This task runs in its own thread and processes janitor cleanup requests without
/// blocking the miner task. It maintains separation of concerns by focusing solely
/// on deposit recovery while the miner focuses on mining and submission operations.
async fn janitor_task<T>(
	client: Client,
	signer: Signer,
	mut janitor_rx: mpsc::Receiver<JanitorMessage>,
) -> Result<(), Error>
where
	T: MinerConfig<AccountId = AccountId> + Send + Sync + 'static,
	T::Solution: Send + Sync + 'static,
	T::Pages: Send + Sync + 'static,
	T::TargetSnapshotPerBlock: Send + Sync + 'static,
	T::VoterSnapshotPerBlock: Send + Sync + 'static,
	T::MaxVotesPerVoter: Send + Sync + 'static,
{
	log::trace!(target: LOG_TARGET, "Janitor task started");

	// Track the last round we've scanned to avoid rescanning
	let mut last_scanned_round: Option<u32> = None;

	while let Some(message) = janitor_rx.recv().await {
		match message {
			JanitorMessage::JanitorTick { current_round } => {
				log::trace!(target: LOG_TARGET, "Running janitor cleanup for round {current_round}");

				let start_time = std::time::Instant::now();
				let result = run_janitor_cleanup::<T>(
					client.clone(),
					signer.clone(),
					current_round,
					&mut last_scanned_round,
				)
				.await;
				let duration = start_time.elapsed().as_millis() as f64;
				prometheus::observe_janitor_cleanup_duration(duration);

				match result {
					Ok(cleaned_count) => {
						prometheus::on_janitor_cleanup_success(cleaned_count);
						if cleaned_count > 0 {
							log::info!(target: LOG_TARGET, "Janitor cleaned up {} old submissions in {}ms", cleaned_count, duration as u64);
						} else {
							log::trace!(target: LOG_TARGET, "Janitor found no old submissions to clean up");
						}
					},
					Err(e) => {
						prometheus::on_janitor_cleanup_failure();
						if is_critical_miner_error(&e) {
							log::error!(target: LOG_TARGET, "Critical janitor error - process will exit: {e:?}");
							return Err(e);
						} else {
							log::warn!(target: LOG_TARGET, "Janitor cleanup failed, continuing: {e:?}");
						}
					},
				}
			},
		}
	}

	// This should never be reached as janitor runs indefinitely
	log::error!(target: LOG_TARGET, "Janitor task loop exited unexpectedly");
	Err(Error::Other("Janitor task completed unexpectedly".to_string()))
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

	// Update balance
	let account_info = storage
		.fetch(&runtime::storage().system().account(signer.account_id().clone()))
		.await?
		.ok_or(Error::AccountDoesNotExists)?;
	prometheus::set_balance(account_info.data.free as f64);

	// Handle different phases
	match phase {
		Phase::Snapshot(_) => {
			dynamic::fetch_missing_snapshots_lossy::<T>(snapshot, &storage, round).await?;
			return Ok(());
		},
		Phase::Signed(blocks_remaining) => {
			if blocks_remaining <= config.min_signed_phase_blocks {
				log::trace!(
					target: LOG_TARGET,
					"Signed phase has only {} blocks remaining (need at least {}), skipping mining to avoid incomplete submission",
					blocks_remaining,
					config.min_signed_phase_blocks
				);
				return Ok(());
			}
			log::trace!(target: LOG_TARGET, "Signed phase with {blocks_remaining} blocks remaining - checking for mining opportunity");
		},
		_ => {
			log::trace!(target: LOG_TARGET, "Phase {phase:?} - nothing to do");
			return Ok(());
		},
	}

	// Fetch snapshots if needed
	dynamic::fetch_missing_snapshots::<T>(snapshot, &storage, round).await?;
	let (target_snapshot, voter_snapshot) = snapshot.get();

	// Check if we already submitted for this round
	if has_submitted(&utils::storage_at_head(&client).await?, round, signer.account_id(), n_pages)
		.await?
	{
		log::trace!(target: LOG_TARGET, "Already submitted for round {round}, skipping");
		return Ok(());
	}

	// Mine the solution
	log::debug!(target: LOG_TARGET, "Mining solution for block #{block_number} round {round}");

	let paged_raw_solution = match dynamic::mine_solution::<T>(
		target_snapshot,
		voter_snapshot,
		n_pages,
		round,
		desired_targets,
		block_number,
		config.do_reduce,
	)
	.timed()
	.await
	{
		(Ok(sol), dur) => {
			log::info!(target: LOG_TARGET, "Mining solution took {}ms for block #{}", dur.as_millis(), block_number);
			prometheus::observe_mined_solution_duration(dur.as_millis() as f64);
			sol
		},
		(Err(e), dur) => {
			log::error!(target: LOG_TARGET, "Mining failed after {}ms: {:?}", dur.as_millis(), e);
			return Err(e);
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
	const CHECK_EXISTING_SUBMISSION_TIMEOUT_SECS: u64 = 300; // 5 minutes should be enough for checking submissions
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
			log::error!(target: LOG_TARGET, "Check existing submission timed out after {} seconds for block #{}", CHECK_EXISTING_SUBMISSION_TIMEOUT_SECS, block_number);
			prometheus::on_check_existing_submission_timeout();
			return Err(Error::CheckExistingSubmissionTimeout {
				timeout_secs: CHECK_EXISTING_SUBMISSION_TIMEOUT_SECS,
			});
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

			// Verify we're still in signed phase before bailing
			if !signed_phase(&client).await? {
				log::warn!(target: LOG_TARGET, "Phase changed, cannot bail existing submission");
				return Ok(());
			}

			dynamic::bail(&client, &signer).await?;
		},
		CurrentSubmission::Incomplete(s) => {
			if s.score() == paged_raw_solution.score {
				// Same score, just submit missing pages
				let missing_pages: Vec<(u32, T::Solution)> = s
					.get_missing_pages()
					.map(|page| (page, paged_raw_solution.solution_pages[page as usize].clone()))
					.collect();

				log::info!(target: LOG_TARGET, "Submitting {} missing pages for existing submission", missing_pages.len());

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
				return Ok(());
			}

			log::debug!(target: LOG_TARGET, "Reverting incomplete submission to submit new solution");

			// Verify we're still in signed phase before bailing
			if !signed_phase(&client).await? {
				log::warn!(target: LOG_TARGET, "Phase changed, cannot bail incomplete submission");
				return Ok(());
			}

			dynamic::bail(&client, &signer).await?;
		},
		CurrentSubmission::NotStarted => {
			log::debug!(target: LOG_TARGET, "No existing submission found");
		},
	};

	// Check if our score is competitive (skip this check in shady mode)
	if !should_bypass_competitive_check(config.shady) &&
		!score_better(&storage_head, paged_raw_solution.score, round, config.submission_strategy)
			.await?
	{
		log::debug!(target: LOG_TARGET, "Our score is not competitive, skipping submission");
		return Ok(());
	}

	prometheus::set_score(paged_raw_solution.score);
	log::info!(target: LOG_TARGET, "Submitting solution with score {:?} for round {}", paged_raw_solution.score, round);

	// Submit the solution
	match dynamic::submit(
		&client,
		&signer,
		paged_raw_solution,
		config.chunk_size,
		round,
		config.min_signed_phase_blocks,
	)
	.timed()
	.await
	{
		(Ok(_), dur) => {
			log::info!(
				target: LOG_TARGET,
				"Successfully submitted solution for round {} in {}ms",
				round,
				dur.as_millis()
			);
			prometheus::observe_submit_and_watch_duration(dur.as_millis() as f64);
		},
		(Err(e), dur) => {
			log::error!(
				target: LOG_TARGET,
				"Submission failed after {}ms: {:?}",
				dur.as_millis(),
				e
			);
			return Err(e);
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
	let tx = utils::wait_tx_in_finalized_block(tx_status, "registering malicious score").await?;
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

/// Run janitor cleanup to reclaim deposits from old discarded submissions
///
/// This function scans the last few previous rounds looking for submissions from our account
/// that were discarded (not selected as winners). We only check the last few rounds because:
/// 1. The miner is expected to run 24/7, so in the happy path we only need to clean up the previous
///    round (if any cleanup is needed)
/// 2. If the miner was offline for a few rounds, JANITOR_SCAN_ROUNDS rounds provides a reasonable
///    buffer
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
async fn run_janitor_cleanup<T>(
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
			let min_round = current_round.saturating_sub(JANITOR_SCAN_ROUNDS);
			(*last + 1).max(min_round)
		},
		None => {
			// First run - scan the full window
			current_round.saturating_sub(JANITOR_SCAN_ROUNDS)
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

	prometheus::set_janitor_old_submissions_found(found_count);

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
	log::info!(target: LOG_TARGET, "Janitor: Fire-and-forget submission for round {round} cleanup");
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
		Error::TxFinalizationTimeout { .. } |
		Error::MiningTimeout { .. } |
		Error::CheckExistingSubmissionTimeout { .. } |
		Error::BailTimeout { .. } |
		Error::SubmitTimeout { .. } => false,
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
