#![cfg(feature = "integration-tests")]
//! Integration tests for the multi-block monitor (pallet-election-multi-block).
//! See nightly.yml for instructions on how to run it compared vs a zombienet setup.
use polkadot_staking_miner::{
	prelude::ChainClient,
	runtime::multi_block::{
		self as runtime,
		multi_block_election_signed::events::{Discarded, Registered, Rewarded, Slashed, Stored},
	},
};
use std::{
	collections::HashSet,
	io::{BufRead, BufReader},
	ops::{Deref, DerefMut},
	process::{Child, ChildStderr, ChildStdout, Stdio},
	time::Instant,
};
use subxt::utils::AccountId32;
use tracing_subscriber::EnvFilter;

#[tokio::test]
async fn submit_works() {
	init_logger();

	// Zombienet is expected to be already running on port 9946 (hardcoded for the time being!)
	const PORT: u16 = 9946;
	let _miner_alice = run_miner(PORT, "//Alice", false);
	let _miner_bob = run_miner(PORT, "//Bob", false);
	let _miner_charlie = run_miner(PORT, "//Charlie", true);
	assert!(wait_for_three_miners_solution(PORT).await.is_ok());
}

fn run_miner(port: u16, seed: &str, shady: bool) -> KillChildOnDrop {
	log::info!("Starting miner with seed {} (shady: {})", seed, shady);

	let mut args = vec![
		"--uri".to_string(),
		format!("ws://127.0.0.1:{}", port),
		"monitor".to_string(),
		"--seed-or-path".to_string(),
		seed.to_string(),
	];

	if shady {
		args.push("--shady".to_string());
	}

	let mut miner = KillChildOnDrop(
		std::process::Command::new(assert_cmd::cargo::cargo_bin!(env!("CARGO_PKG_NAME")))
			.stdout(Stdio::piped())
			.stderr(Stdio::piped())
			.args(args)
			.env("RUST_LOG", "polkadot-staking-miner=trace,runtime::multiblock-election=trace")
			.spawn()
			.unwrap(),
	);

	let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
	spawn_cli_output_threads(miner.stdout.take().unwrap(), miner.stderr.take().unwrap(), tx);

	tokio::spawn(async move {
		let r = rx.recv().await.unwrap();
		log::info!("{}", r);
	});

	miner
}

/// Wait until solutions are ready on chain for three miners, with the third being malicious.
/// This function strictly checks that:
/// 1. All three miners submit scores (Alice, Bob, Charlie)
/// 2. Alice and Bob submit all pages (legitimate solutions)
/// 3. Charlie only submits score (malicious incomplete solution with ElectionScore::max_value())
/// 4. One of Alice/Bob receives a reward and the other gets discarded
/// 5. Charlie gets slashed for incomplete submission
///  - Note that both Alice and Bob will submit an identical solution, so it is not deterministic
///    who will be rewarded. However, that is not the focus of this test!
///
/// The `Slashed` event is sent when the election pallet fails to validate a solution. In the case
/// of this test where the malicious solution has the best score and has no pages submitted, it will
/// be sent after the first Pages() validation blocks.
/// The `Rewarded` event is sent at the end of the SignedValidation phase.
/// In contrast, the `Discarded` event is expected to be sent in the next round
/// after the miner calls `clear_old_round_data()` upon detecting that its previous round's solution
/// was not the winning one.
pub async fn wait_for_three_miners_solution(port: u16) -> anyhow::Result<()> {
	// Timeout will be calculated dynamically based on pallet constants

	let now = Instant::now();
	log::info!("Starting to wait for three miners' solutions on port {}", port);

	// API should be ready immediately since run_zombienet waits for it
	let api = ChainClient::from_url(&format!("ws://127.0.0.1:{}", port)).await?;

	// Read pallet constants to calculate proper timeout
	let constants = read_pallet_constants(&api).await?;

	// In order for the test to be successful, it is expected that the SignedValidation phase lasts
	// at least T::Pages() * 2 otherwise only a solution can be fully validated.
	if constants.signed_validation_phase < 2 * constants.pages {
		return Err(anyhow::anyhow!(
			"Test requirement not met: SignedValidationPhase ({}) must be at least 2 * Pages ({})",
			constants.signed_validation_phase,
			2 * constants.pages
		));
	}

	// Calculate timeout based on block time and phase durations
	let max_duration = calculate_test_timeout(&constants);
	log::info!(
		"🧮 Test timeout calculated: {:?} (Pages: {}, SignedPhase: {}, SignedValidationPhase: {}, UnsignedPhase: {})",
		max_duration,
		constants.pages,
		constants.signed_phase,
		constants.signed_validation_phase,
		constants.unsigned_phase
	);

	// Track Alice's progress
	let mut alice_score_submitted = false;
	let mut alice_pages_submitted = HashSet::new();
	let mut alice_all_pages_submitted = false;

	// Track Bob's progress
	let mut bob_score_submitted = false;
	let mut bob_pages_submitted = HashSet::new();
	let mut bob_all_pages_submitted = false;

	// Track Charlie's progress (malicious miner)
	let mut charlie_score_submitted = false;

	// Track final outcomes
	let mut rewarded_account: Option<AccountId32> = None;
	let mut discarded_account: Option<AccountId32> = None;
	let mut slashed_account: Option<AccountId32> = None;

	let mut blocks_sub = api.blocks().subscribe_finalized().await?;
	let pages = api
		.constants()
		.at(&runtime::constants().multi_block_election().pages())
		.unwrap();

	'outer: while let Some(block) = blocks_sub.next().await {
		if now.elapsed() > max_duration {
			break;
		}

		let block = block?;
		let events = block.events().await?;

		for ev in events.iter() {
			let ev = ev?;

			// Score registration.
			if let Some(item) = ev.as_event::<Registered>()? {
				if item.1 == alice() {
					log::info!("Alice score registered");
					alice_score_submitted = true;
				} else if item.1 == bob() {
					log::info!("Bob score registered");
					bob_score_submitted = true;
				} else if item.1 == charlie() {
					log::info!("Charlie score registered (malicious)");
					charlie_score_submitted = true;
				}
			}

			// Page submission.
			if let Some(item) = ev.as_event::<Stored>()? {
				if item.1 == alice() {
					log::info!(
						"Adding Alice page {} to pages_submitted (was size {})",
						item.2,
						alice_pages_submitted.len()
					);
					alice_pages_submitted.insert(item.2);
				} else if item.1 == bob() {
					log::info!(
						"Adding Bob page {} to pages_submitted (was size {})",
						item.2,
						bob_pages_submitted.len()
					);
					bob_pages_submitted.insert(item.2);
				}
			}

			if !alice_all_pages_submitted && alice_pages_submitted.len() == pages as usize {
				log::info!("All Alice pages submitted");
				alice_all_pages_submitted = true;
			}

			if !bob_all_pages_submitted && bob_pages_submitted.len() == pages as usize {
				log::info!("All Bob pages submitted");
				bob_all_pages_submitted = true;
			}

			// Check for reward event - this is sent by the chain upon successful verification of a
			// solution
			if let Some(item) = ev.as_event::<Rewarded>()? {
				if item.1 == alice() {
					log::info!("Alice rewarded!");
					rewarded_account = Some(alice());
				} else if item.1 == bob() {
					log::info!("Bob rewarded!");
					rewarded_account = Some(bob());
				}
			}

			// Check for slash event - this is sent by the chain once it detects an invalid
			// submissions.
			if let Some(item) = ev.as_event::<Slashed>()? {
				if item.1 == charlie() {
					log::info!("Charlie slashed for incomplete/invalid submission!");
					slashed_account = Some(charlie());
				}
			}

			// Check for discard event - this is sent by the chain at the next round after a miner
			// has called `clear_old_round_data()` to reclaim deposit back for a discarded solution.
			if let Some(item) = ev.as_event::<Discarded>()? {
				if item.1 == alice() {
					log::info!("Alice solution discarded!");
					discarded_account = Some(alice());
				} else if item.1 == bob() {
					log::info!("Bob solution discarded!");
					discarded_account = Some(bob());
				}
			}

			// Check if we have all three outcomes
			if rewarded_account.is_some() &&
				discarded_account.is_some() &&
				slashed_account.is_some()
			{
				let rewarded = rewarded_account.as_ref().unwrap();
				let discarded = discarded_account.as_ref().unwrap();
				let slashed = slashed_account.as_ref().unwrap();

				// Verify that Alice and Bob submitted scores and pages properly
				let rewarded_submitted_properly = if *rewarded == alice() {
					alice_score_submitted && alice_all_pages_submitted
				} else {
					bob_score_submitted && bob_all_pages_submitted
				};

				let discarded_submitted_properly = if *discarded == alice() {
					alice_score_submitted && alice_all_pages_submitted
				} else {
					bob_score_submitted && bob_all_pages_submitted
				};

				// Verify that Charlie only submitted score (malicious behavior)
				let charlie_behaved_maliciously = *slashed == charlie() && charlie_score_submitted;

				if rewarded_submitted_properly &&
					discarded_submitted_properly &&
					charlie_behaved_maliciously
				{
					log::info!(
						"🤑 Successfully completed three-miner test: Alice and Bob submitted solutions (one rewarded, one discarded), Charlie slashed for malicious behavior! Duration: {:?} 🤑",
						now.elapsed()
					);
					return Ok(());
				} else {
					log::error!(
						"Inconsistent state: rewarded properly: {}, discarded properly: {}, charlie malicious: {}",
						rewarded_submitted_properly,
						discarded_submitted_properly,
						charlie_behaved_maliciously
					);
					break 'outer;
				}
			}
		}
	}

	log::info!("Failed to complete three-miner test after {:?}", now.elapsed());
	Err(anyhow::anyhow!(
		"Three-miner test failed: Alice - score: {}, pages: {}/{}, Bob - score: {}, pages: {}/{}, Charlie - score: {}, rewarded: {:?}, discarded: {:?}, slashed: {:?}; timeout after {}s",
		alice_score_submitted,
		alice_pages_submitted.len(),
		pages,
		bob_score_submitted,
		bob_pages_submitted.len(),
		pages,
		charlie_score_submitted,
		rewarded_account.is_some(),
		discarded_account.is_some(),
		slashed_account.is_some(),
		max_duration.as_secs()
	))
}

/// Pallet constants needed for test timeout calculation
#[derive(Debug)]
struct PalletConstants {
	pages: u32,
	signed_phase: u32,
	signed_validation_phase: u32,
	unsigned_phase: u32,
}

/// Read the MultiBlockElection pallet constants
async fn read_pallet_constants(api: &ChainClient) -> anyhow::Result<PalletConstants> {
	let pages = api
		.constants()
		.at(&runtime::constants().multi_block_election().pages())
		.map_err(|e| anyhow::anyhow!("Failed to read Pages constant: {}", e))?;

	let signed_phase = api
		.constants()
		.at(&subxt::dynamic::constant("MultiBlockElection", "SignedPhase"))
		.map_err(|e| anyhow::anyhow!("Failed to read SignedPhase constant: {}", e))?
		.to_value()
		.map_err(|e| anyhow::anyhow!("Failed to decode SignedPhase: {}", e))?;
	let signed_phase: u32 = scale_value::serde::from_value(signed_phase)
		.map_err(|e| anyhow::anyhow!("Failed to deserialize SignedPhase: {}", e))?;

	let signed_validation_phase = api
		.constants()
		.at(&subxt::dynamic::constant("MultiBlockElection", "SignedValidationPhase"))
		.map_err(|e| anyhow::anyhow!("Failed to read SignedValidationPhase constant: {}", e))?
		.to_value()
		.map_err(|e| anyhow::anyhow!("Failed to decode SignedValidationPhase: {}", e))?;
	let signed_validation_phase: u32 = scale_value::serde::from_value(signed_validation_phase)
		.map_err(|e| anyhow::anyhow!("Failed to deserialize SignedValidationPhase: {}", e))?;

	let unsigned_phase = api
		.constants()
		.at(&subxt::dynamic::constant("MultiBlockElection", "UnsignedPhase"))
		.map_err(|e| anyhow::anyhow!("Failed to read UnsignedPhase constant: {}", e))?
		.to_value()
		.map_err(|e| anyhow::anyhow!("Failed to decode UnsignedPhase: {}", e))?;
	let unsigned_phase: u32 = scale_value::serde::from_value(unsigned_phase)
		.map_err(|e| anyhow::anyhow!("Failed to deserialize UnsignedPhase: {}", e))?;

	Ok(PalletConstants { pages, signed_phase, signed_validation_phase, unsigned_phase })
}

/// The timeout is dynamically calculated based on the MultiBlockElection pallet constants and an
/// average block production rate of one block every six seconds. The calculation includes all
/// phases of a complete election round plus a buffer (in case block production takes longer
/// than 6s and to take into account initial `Off` blocks, potential network issues, etc)
fn calculate_test_timeout(constants: &PalletConstants) -> std::time::Duration {
	const BLOCK_TIME_SECS: u32 = 6;
	const BUFFER_MULTIPLIER: f64 = 2.0;

	let total_blocks = constants.pages + 1 + // snapshot
		constants.signed_phase +
		constants.signed_validation_phase +
		constants.unsigned_phase +
        1 + // done
		constants.pages; // export
	let total_secs = total_blocks * BLOCK_TIME_SECS;
	let buffered_secs = (total_secs as f64 * BUFFER_MULTIPLIER) as u64;

	std::time::Duration::from_secs(buffered_secs)
}

fn alice() -> AccountId32 {
	use polkadot_sdk::sp_core::crypto::Pair;

	let pair = polkadot_staking_miner::prelude::Pair::from_string("//Alice", None).unwrap();
	let public = pair.public();
	AccountId32::from(public.0)
}

fn bob() -> AccountId32 {
	use polkadot_sdk::sp_core::crypto::Pair;

	let pair = polkadot_staking_miner::prelude::Pair::from_string("//Bob", None).unwrap();
	let public = pair.public();
	AccountId32::from(public.0)
}

fn charlie() -> AccountId32 {
	use polkadot_sdk::sp_core::crypto::Pair;

	let pair = polkadot_staking_miner::prelude::Pair::from_string("//Charlie", None).unwrap();
	let public = pair.public();
	AccountId32::from(public.0)
}

pub fn init_logger() {
	let _ = tracing_subscriber::fmt()
		.with_env_filter(EnvFilter::from_default_env())
		.try_init();
}

pub struct KillChildOnDrop(pub Child);

impl Drop for KillChildOnDrop {
	fn drop(&mut self) {
		let _ = self.0.kill();
	}
}

impl Deref for KillChildOnDrop {
	type Target = Child;

	fn deref(&self) -> &Self::Target {
		&self.0
	}
}

impl DerefMut for KillChildOnDrop {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.0
	}
}

pub fn spawn_cli_output_threads(
	stdout: ChildStdout,
	stderr: ChildStderr,
	tx: tokio::sync::mpsc::UnboundedSender<String>,
) {
	let tx2 = tx.clone();
	std::thread::spawn(move || {
		for line in BufReader::new(stdout).lines() {
			let line = line.expect("Failed to read line from stdout");
			println!("OK: {}", line);
			let _ = tx2.send(line);
		}
	});
	std::thread::spawn(move || {
		for line in BufReader::new(stderr).lines() {
			let line = line.expect("Failed to read line from stderr");
			println!("ERR: {}", line);
			let _ = tx.send(line);
		}
	});
}
