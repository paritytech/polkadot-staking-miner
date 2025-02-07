use crate::{
	client::Client,
	commands::{multi_block::types::Snapshot, Listen},
	epm::{
		self, fetch_missing_snapshots, submit_and_watch, MultiBlockTransaction,
	},
	error::Error,
	helpers,
	prelude::{
		runtime::{self},
		AccountId, Header, LOG_TARGET,
	},
	signer::Signer,
	static_types,
};

use polkadot_sdk::{
	pallet_election_provider_multi_block::{types::Phase, unsigned::miner::MinerConfig},
	sp_npos_elections::ElectionScore,
};
use std::sync::Arc;
use subxt::config::Header as _;
use tokio::sync::Mutex;

/// TODO(niklasad1): Add solver algorithm configuration to the monitor command.
#[derive(Debug, Clone, clap::Parser, PartialEq)]
pub struct MonitorConfig {
	#[clap(long, short, env = "SEED")]
	pub seed_or_path: String,

	#[clap(long, value_enum, default_value_t = Listen::Finalized)]
	pub listen: Listen,
}

pub async fn monitor_cmd<T>(client: Client, config: MonitorConfig) -> Result<(), Error>
where
	T: MinerConfig<AccountId = AccountId> + Send + Sync + 'static,
	T::Solution: Send,
	T::Pages: Send,
	T::TargetSnapshotPerBlock: Send,
	T::VoterSnapshotPerBlock: Send,
	T::MaxVotesPerVoter: Send,
{
	let signer = Signer::new(&config.seed_or_path)?;
	let account_info = {
		let addr = runtime::storage().system().account(signer.account_id());
		client
			.chain_api()
			.storage()
			.at_latest()
			.await?
			.fetch(&addr)
			.await?
			.ok_or(Error::AccountDoesNotExists)?
	};

	log::info!(target: LOG_TARGET, "Loaded account {} {{ nonce: {}, free_balance: {}, reserved_balance: {}, frozen_balance: {} }}",
		signer,
		account_info.nonce,
		account_info.data.free,
		account_info.data.reserved,
		account_info.data.frozen,
	);

	let mut subscription = helpers::rpc_block_subscription(client.rpc(), config.listen).await?;
	let (_tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Error>();
	let _submit_lock = Arc::new(Mutex::new(()));
	let mut snapshot = Snapshot::<T>::new(static_types::Pages::get());

	loop {
		let at = tokio::select! {
			maybe_rp = subscription.next() => {
				match maybe_rp {
					Some(Ok(r)) => r,
					Some(Err(e)) => {
						log::error!(target: LOG_TARGET, "subscription failed to decode Header {:?}, this is bug please file an issue", e);
						return Err(e.into());
					}
					// The subscription was dropped, should only happen if:
					//	- the connection was closed.
					//	- the subscription could not keep up with the server.
					None => {
						log::warn!(target: LOG_TARGET, "subscription to `{:?}` terminated. Retrying..", config.listen);
						subscription = helpers::rpc_block_subscription(client.rpc(), config.listen).await?;
						continue
					}
				}
			},
			maybe_err = rx.recv() => {
				match maybe_err {
					Some(err) => return Err(err),
					None => unreachable!("at least one sender kept in the main loop should always return Some; qed"),
				}
			}
		};

		if let Err(e) = process_block(&client, &at, &mut snapshot, &signer, config.listen).await {
			log::error!(target: LOG_TARGET, "Error processing block: {:?}", e);
		}
	}
}

/// For each block, monitor essentially does the following:
///
/// 1. Fetch the current storage.
/// 2. Check if the phase is signed/snapshot, otherwise continue with the next block.
/// 3. Check if the solution has already been submitted, if so quit.
/// 4. Fetch the target and voter snapshots.
/// 5. Mine the solution.
/// 6. Check if our score is better according to submit strategy.
/// 7. Register the solution score.
/// 8. Submit each page of the solution (one per block)
/// 9. Finally wait for verification.
async fn process_block<T>(
	client: &Client,
	at: &Header,
	snapshot: &mut Snapshot<T>,
	signer: &Signer,
	listen: Listen,
) -> Result<(), Error>
where
	T: MinerConfig<AccountId = AccountId> + Send + Sync + 'static,
	T::Solution: Send,
	T::Pages: Send,
	T::TargetSnapshotPerBlock: Send,
	T::VoterSnapshotPerBlock: Send,
	T::MaxVotesPerVoter: Send,
{
	// 1. Fetch the current storage.
	let storage = helpers::storage_at(Some(at.hash()), client.chain_api()).await?;
	let round = storage.fetch_or_default(&runtime::storage().multi_block().round()).await?;
	let n_pages = static_types::Pages::get();
	let phase = storage
		.fetch_or_default(&runtime::storage().multi_block().current_phase())
		.await?;
	// Target snapshot page (most significant page).
	let target_snapshot_page = n_pages - 1;

	log::trace!(target: LOG_TARGET, "Processing block={} round={}, phase={:?}", at.number, round, phase.0);

	snapshot.set_page_length(n_pages);

	// 2. Check if the phase is signed/snapshot, otherwise wait for the next block.
	match phase.0 {
		Phase::Snapshot(page) => {
			if page == target_snapshot_page && snapshot.needs_target_snapshot() {
				let target_snapshot = epm::target_snapshot::<T>(page, &storage).await?;
				snapshot.set_target_snapshot(target_snapshot);
			}

			if snapshot.needs_voter_page(page) {
				let voter_snapshot = epm::paged_voter_snapshot::<T>(page, &storage).await?;
				snapshot.set_voter_page(page, voter_snapshot);
			}

			return Ok(());
		},
		Phase::Signed => {},
		_ => {
			snapshot.clear();
			return Ok(());
		},
	}

	// 3. If the solution has already been submitted, nothing to do
	if has_submitted::<T>(&client, round, signer.account_id()).await? {
		return Ok(());
	}

	// 4. Fetch the target and voter snapshots if needed.
	fetch_missing_snapshots::<T>(snapshot, &storage).await?;

	let desired_targets = storage
		.fetch(&runtime::storage().multi_block().desired_targets())
		.await?
		.unwrap_or(0);
	let target_snapshot = snapshot.target.clone().expect("Target snapshot exists; qed");
	let voter_snapshot = snapshot.voter.iter().map(|(_, v)| v.clone()).collect::<Vec<_>>();

	// 5. Mine solution
	let paged_raw_solution =
		epm::mine_solution::<T>(target_snapshot, voter_snapshot, n_pages, round, desired_targets)
			.await?;

	// 6. Check if our score is better according to submit strategy.
	if !score_better(&client, paged_raw_solution.score, round).await? {
		return Ok(());
	}

	// 7. Register score.
	submit_and_watch::<T>(
		&at,
		&client,
		signer.clone(),
		listen,
		MultiBlockTransaction::register_score(paged_raw_solution.score)?,
	)
	.await?;

	// 8. Submit all solution pages.
	for (page, solution) in paged_raw_solution.solution_pages.into_iter().enumerate() {
		submit_and_watch::<T>(
			&at,
			&client,
			signer.clone(),
			listen,
			MultiBlockTransaction::submit_page::<T>(page as u32, Some(solution))?,
		)
		.await?;
	}

	// 9. Wait for verification.
	// TODO: Implement verification....

	Ok(())
}

async fn has_submitted<T: MinerConfig>(
	client: &Client,
	round: u32,
	who: &subxt::config::substrate::AccountId32,
) -> Result<bool, Error> {
	let scores = client.chain_api().storage().at_latest().await?.fetch_or_default(&runtime::storage().multi_block_signed().sorted_scores(round))
		.await?;

	log::trace!(target: LOG_TARGET, "Checking if our miner has already submitted at round={round} {:?}", scores);

	if scores.0.into_iter().any(|(account_id, _)| &account_id == who) {
		return Ok(true);
	}

	Ok(false)
}

async fn score_better(client: &Client, score: ElectionScore, round: u32) -> Result<bool, Error> {
	let scores = client.chain_api().storage().at_latest().await?.fetch_or_default(&runtime::storage().multi_block_signed().sorted_scores(round))
		.await?;

	log::trace!(target: LOG_TARGET, "Checking if score is better at round={round}, {:?}", scores);

	if scores.0.into_iter().any(|(_, other_score)| other_score.0 > score) {
		return Ok(false);
	}

	Ok(true)
}
