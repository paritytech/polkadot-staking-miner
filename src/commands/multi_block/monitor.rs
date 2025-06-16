use crate::{
	client::Client,
	commands::{
		multi_block::types::{BlockDetails, CurrentSubmission, IncompleteSubmission, Snapshot},
		types::{Listen, MultiBlockMonitorConfig, SubmissionStrategy},
	},
	dynamic::multi_block as dynamic,
	error::Error,
	prelude::{AccountId, LOG_TARGET, Storage},
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

use tokio::sync::mpsc;

/// Message types for communication between listener and miner
enum MinerMessage {
	/// Request to process a block with given details
	ProcessBlock { state: BlockDetails },

	/// Signal to clear snapshots (phase ended)
	ClearSnapshots,
}

/// The monitor command splits the work into two communicating tasks:
///
/// ### Listener Task
/// - Always follows the chain (head or finalized blocks)
/// - Performs fast phase checks to identify relevant blocks (Snapshot or Signed)
/// - Checks if miner is busy before sending work
/// - Never blocks on slow operations
/// - **Any error causes entire process to exit** (RPC errors handled by reconnecting client)
///
/// ### Miner Task
/// - Single-threaded processor that handles one block at a time
/// - Performs the expensive mining and submission operations
/// - **Critical errors cause process exit** (e.g. account doesn't exist)
/// - **Non-critical errors are recoverable** (log and continue)
/// - Handles snapshot management and cleanup
///
/// ### Communication
/// - Uses bounded channels (buffer=1) for automatic flow control
/// - **Backpressure mechanism**: When miner is busy processing a block:
///   - Channel becomes full, listener's try_send() returns immediately
///   - Listener skips current block and moves to next (fresher) block
///   - No blocking, no buffering of stale work
/// - No submission locks needed since miner is single-threaded by design
///
/// ## Error Handling Strategy
/// - **Listener errors**: All critical - process exits immediately
/// - **Miner errors**: Classified as critical or recoverable
///   - Critical: Account doesn't exist, invalid metadata, persistent RPC failures
///   - Recoverable: Already submitted, wrong phase, temporary mining issues
/// - **RPC issues**: Handled transparently by reconnecting RPC client
///
/// ```text
/// ┌─────────────────┐                    ┌─────────────────┐
/// │  Listener Task  │                    │   Miner Task    │
/// │                 │                    │                 │
/// │ ┌─────────────┐ │  bounded chan(1)   │ ┌─────────────┐ │
/// │ │Block Stream │ │ ──────────────────▶│ │   Process   │ │
/// │ │Subscription │ │                    │ │    Block    │ │
/// │ └─────────────┘ │                    │ └─────────────┘ │
/// │        │        │                    │                 │
/// │        ▼        │                    │                 │
/// │ ┌─────────────┐ │                    │                 │
/// │ │Phase Check  │ │   ┌─ Channel Full? │                 │
/// │ │(fast)       │ │   │                │                 │
/// │ └─────────────┘ │   │    YES ──────▶ │ Skip block      │
/// │        │        │   │                │ (backpressure)  │
/// │        ▼        │   │                │                 │
/// │ ┌─────────────┐ │───┘                │                 │
/// │ │try_send()   │ │    NO ────────────▶│ Accept work     │
/// │ │(non-block)  │ │                    │                 │
/// │ └─────────────┘ │                    │                 │
/// └─────────────────┘                    └─────────────────┘
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
			.fetch(&runtime::storage().system().account(signer.account_id()))
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

	// Spawn the miner task
	let miner_handle = {
		let client = client.clone();
		let signer = signer.clone();
		let config = config.clone();
		tokio::spawn(async move { miner_task::<T>(client, signer, config, miner_rx).await })
	};

	// Spawn the listener task
	let listener_handle = {
		let client = client.clone();
		let config = config.clone();
		tokio::spawn(async move { listener_task::<T>(client, config, miner_tx).await })
	};

	// Wait for either task to complete (which should never happen in normal operation)
	// If either task exits, the whole process should exit
	tokio::select! {
		result = listener_handle => {
			match result {
				Ok(Ok(())) => {
					log::error!(target: LOG_TARGET, "Listener task completed unexpectedly");
					Err(Error::Other("Listener task should never complete".to_string()))
				}
				Ok(Err(e)) => {
					log::error!(target: LOG_TARGET, "Listener task failed: {}", e);
					Err(e)
				}
				Err(e) => {
					log::error!(target: LOG_TARGET, "Listener task panicked: {}", e);
					Err(Error::Other(format!("Listener task panicked: {}", e)))
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
					log::error!(target: LOG_TARGET, "Miner task failed: {}", e);
					Err(e)
				}
				Err(e) => {
					log::error!(target: LOG_TARGET, "Miner task panicked: {}", e);
					Err(Error::Other(format!("Miner task panicked: {}", e)))
				}
			}
		}
	}
}

/// Listener task that follows the chain and signals the miner when appropriate
///
/// This task is responsible for:
/// - Subscribing to block updates (head or finalized)
/// - Performing fast phase checks to identify signed/snapshot phases
/// - Using backpressure to skip blocks when miner is busy
/// - Managing phase transitions and snapshot cleanup
/// - Never blocking on slow operations to avoid subscription buffering
/// - Any error causes the entire process to exit
async fn listener_task<T>(
	client: Client,
	config: MultiBlockMonitorConfig,
	miner_tx: mpsc::Sender<MinerMessage>,
) -> Result<(), Error>
where
	T: MinerConfig<AccountId = AccountId> + Send + Sync + 'static,
	T::Solution: Send + Sync + 'static,
	T::Pages: Send + Sync + 'static,
	T::TargetSnapshotPerBlock: Send + Sync + 'static,
	T::VoterSnapshotPerBlock: Send + Sync + 'static,
	T::MaxVotesPerVoter: Send + Sync + 'static,
{
	let mut subscription = match config.listen {
		Listen::Head => client.chain_api().blocks().subscribe_best().await?,
		Listen::Finalized => client.chain_api().blocks().subscribe_finalized().await?,
	};

	let mut prev_block_signed_phase = false;

	log::trace!(target: LOG_TARGET, "Listener task started, watching for {:?} blocks", config.listen);

	loop {
		let (at, block_hash) = tokio::select! {
			maybe_block = subscription.next() => {
				match maybe_block {
					Some(block_result) => {
						match block_result {
							Ok(block) => (block.header().clone(), block.hash()),
							Err(e) => {
								// Handle reconnection case with the reconnecting RPC client
								if e.is_disconnected_will_reconnect() {
									log::warn!(target: LOG_TARGET, "RPC connection lost, but will reconnect automatically. Continuing...");
									continue;
								}
								log::error!(target: LOG_TARGET, "subscription failed: {:?}", e);
								return Err(e.into());
							}
						}
					}
					// The subscription was dropped unexpectedly
					None => {
						log::error!(target: LOG_TARGET, "Subscription to `{:?}` terminated unexpectedly", config.listen);
						return Err(Error::Other("Subscription terminated unexpectedly".to_string()));
					}
				}
			}
		};

		// Early exit optimization: check the phase before calling BlockDetails::new(), where we
		// we fetch `storage_at()`, `round()`, and `desired_targets()`.
		// This approach saves us 3 RPC calls.
		let storage = utils::storage_at(Some(block_hash), client.chain_api()).await?;
		let phase = storage
			.fetch_or_default(&runtime::storage().multi_block_election().current_phase())
			.await?;

		match phase {
			Phase::Done | Phase::Off => {
				if prev_block_signed_phase {
					log::debug!(target: LOG_TARGET, "Election round complete (phase {:?}), signaling snapshot cleanup", phase);
					if let Err(e) = miner_tx.send(MinerMessage::ClearSnapshots).await {
						log::error!(target: LOG_TARGET, "Failed to send clear snapshots signal: {}", e);
						return Err(Error::Other(format!(
							"Failed to send clear snapshots signal: {}",
							e
						)));
					}
					prev_block_signed_phase = false;
				}
				log::trace!(target: LOG_TARGET, "Block #{}, Phase {:?} - nothing to do", at.number, phase);
				continue;
			},
			Phase::Signed(_) | Phase::Snapshot(_) => {
				// Relevant phases for mining - continue processing
			},
			_ => {
				log::trace!(target: LOG_TARGET, "Block #{}, Phase {:?} - nothing to do", at.number, phase);
				continue;
			},
		}

		// We're in a relevant phase (Signed or Snapshot)
		prev_block_signed_phase = true;

		let block_number = at.number;
		let state = BlockDetails::new(&client, at, phase, block_hash).await?;

		// Use try_send for backpressure - if miner is busy, skip this block
		let message = MinerMessage::ProcessBlock { state };

		match miner_tx.try_send(message) {
			Ok(()) => {
				log::trace!(target: LOG_TARGET, "Sent block #{} to miner", block_number);
				// Don't wait for response to allow proper backpressure - listener must continue
				// processing blocks
			},
			Err(mpsc::error::TrySendError::Full(_)) => {
				// Miner is busy processing another block - apply backpressure by skipping
				log::trace!(target: LOG_TARGET, "Miner busy, skipping block #{}", block_number);
			},
			Err(mpsc::error::TrySendError::Closed(_)) => {
				log::error!(target: LOG_TARGET, "Miner channel closed unexpectedly");
				return Err(Error::Other("Miner channel closed unexpectedly".to_string()));
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
				};
				let result = process_block::<T>(
					client.clone(),
					state,
					&mut snapshot,
					signer.clone(),
					config.listen,
					process_config,
				)
				.await;

				match result {
					Ok(()) => {
						log::trace!(target: LOG_TARGET, "Block processing completed successfully");
					},
					Err(e) =>
						if is_critical_miner_error(&e) {
							log::error!(target: LOG_TARGET, "Critical miner error - process will exit: {:?}", e);
							return Err(e);
						} else {
							log::warn!(target: LOG_TARGET, "Block processing failed, continuing: {:?}", e);
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
}

async fn process_block<T>(
	client: Client,
	state: BlockDetails,
	snapshot: &mut Snapshot<T>,
	signer: Signer,
	listen: Listen,
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

	log::trace!(target: LOG_TARGET, "Processing block #{} (round {}, phase {:?})", block_number, round, phase);

	// Update balance
	let account_info = storage
		.fetch(&runtime::storage().system().account(signer.account_id()))
		.await?
		.ok_or(Error::AccountDoesNotExists)?;
	prometheus::set_balance(account_info.data.free as f64);

	// Handle different phases
	match phase {
		Phase::Snapshot(_) => {
			dynamic::fetch_missing_snapshots_lossy::<T>(snapshot, &storage, round).await?;
			return Ok(());
		},
		Phase::Signed(_) => {
			log::trace!(target: LOG_TARGET, "Signed phase - checking for mining opportunity");
		},
		_ => {
			log::trace!(target: LOG_TARGET, "Phase {:?} - nothing to do", phase);
			return Ok(());
		},
	}

	// Fetch snapshots if needed
	dynamic::fetch_missing_snapshots::<T>(snapshot, &storage, round).await?;
	let (target_snapshot, voter_snapshot) = snapshot.get();

	// Check if we already submitted for this round
	if has_submitted(
		&utils::storage_at_head(&client, listen).await?,
		round,
		signer.account_id(),
		n_pages,
	)
	.await?
	{
		log::trace!(target: LOG_TARGET, "Already submitted for round {}, skipping", round);
		return Ok(());
	}

	// Mine the solution
	log::debug!(target: LOG_TARGET, "Mining solution for block #{} round {}", block_number, round);
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

	// Get latest storage state (chain may have progressed while we were mining)
	let storage_head = utils::storage_at_head(&client, listen).await?;

	// Handle existing submissions
	match get_submission(&storage_head, round, signer.account_id(), n_pages).await? {
		CurrentSubmission::Done(score) => {
			// We have already submitted the solution with a score
			if !score_passes_strategy(paged_raw_solution.score, score, config.submission_strategy) {
				log::debug!(target: LOG_TARGET, "Our new score doesn't beat existing submission, skipping");
				return Ok(());
			}
			log::debug!(target: LOG_TARGET, "Reverting previous submission to submit better solution");
			dynamic::bail(&client, &signer, listen).await?;
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
						listen,
						round,
					)
					.await?;
				} else {
					dynamic::inner_submit_pages_chunked::<T>(
						&client,
						&signer,
						missing_pages,
						listen,
						config.chunk_size,
						round,
					)
					.await?;
				}
				return Ok(());
			}

			log::debug!(target: LOG_TARGET, "Reverting incomplete submission to submit new solution");
			dynamic::bail(&client, &signer, listen).await?;
		},
		CurrentSubmission::NotStarted => {
			log::debug!(target: LOG_TARGET, "No existing submission found");
		},
	};

	// Check if our score is competitive
	if !score_better(&storage_head, paged_raw_solution.score, round, config.submission_strategy)
		.await?
	{
		log::debug!(target: LOG_TARGET, "Our score is not competitive, skipping submission");
		return Ok(());
	}

	prometheus::set_score(paged_raw_solution.score);
	log::info!(target: LOG_TARGET, "Submitting solution with score {:?} for round {}", paged_raw_solution.score, round);

	// Submit the solution
	match dynamic::submit(&client, &signer, paged_raw_solution, listen, config.chunk_size, round)
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
				.submission_metadata_storage(round, who),
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

/// Determine if a miner error is critical and should cause the process to exit
fn is_critical_miner_error(error: &Error) -> bool {
	match error {
		Error::Join(_) |
		Error::Feasibility(_) |
		Error::EmptySnapshot |
		Error::FailedToSubmitPages(_) => false,
		Error::Subxt(boxed_err) if matches!(boxed_err.as_ref(), subxt::Error::Runtime(_)) => false, /* e.g. Subxt(Runtime(Module(ModuleError(<MultiBlockElectionSigned::Duplicate>)))) */
		Error::Subxt(boxed_err) if matches!(boxed_err.as_ref(), subxt::Error::Transaction(_)) =>
			false, /* e.g. Subxt(Transaction(Invalid("Transaction is invalid (eg because of a bad */
		// nonce, signature etc)"))))
		Error::Subxt(boxed_err) if matches!(boxed_err.as_ref(), subxt::Error::Rpc(_)) => false, /* e.g. Subxt(Rpc(ClientError(User(UserError { code: -32801, message: "Invalid block hash" })))) */
		// Everything else we consider it critical e.g.
		//  - Error::AccountDoesNotExists
		//  - Error::InvalidMetadata(_)
		//  - Error::InvalidChain(_)
		// - Error::Rpc(_) i.e. persistent RPC failures (after reconnecting client gave up)
		// - Error::Subxt(_) i.e. any other subxt error
		_ => true,
	}
}
