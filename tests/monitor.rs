#![cfg(feature = "integration-tests")]
//! Integration tests for the multi-block monitor (pallet-election-multi-block)
//! which requires the following artifacts to be installed in the path:
//! 1. `zombienet`
//! 2. `polkadot --features fast-runtime`
//! 3. `polkadot-parachain`
//! 4. `chainspecs (rc.json and parachain.json)`
//!
//! See ../zombienet-staking-runtimes.toml for further details
//!
//! Requires a `polkadot binary ` built with `--features fast-runtime` in the path to run
//! integration tests against.

use assert_cmd::cargo::cargo_bin;
use polkadot_staking_miner::{
	prelude::ChainClient,
	runtime::multi_block::{
		self as runtime,
		multi_block_election_signed::events::{Discarded, Registered, Rewarded, Stored},
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

	let (_node, port) = run_zombienet().await;
	let _miner_alice = run_miner(port, "//Alice");
	let _miner_bob = run_miner(port, "//Bob");
	assert!(wait_for_two_miners_solution(port).await.is_ok());
}

fn run_miner(port: u16, seed: &str) -> KillChildOnDrop {
	log::info!("Starting miner with seed {}", seed);

	let mut miner = KillChildOnDrop(
		std::process::Command::new(cargo_bin(env!("CARGO_PKG_NAME")))
			.stdout(Stdio::piped())
			.stderr(Stdio::piped())
			.args(["--uri", &format!("ws://127.0.0.1:{}", port), "monitor", "--seed-or-path", seed])
			.env("RUST_LOG", "polkadot_staking_miner=trace,info")
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

/// Wait until solutions are ready on chain for two miners, all pages are submitted,
/// and one user is rewarded while the other is discarded.
/// This function strictly checks that:
/// 1. Both miners submit solutions (scores registered)
/// 2. Both miners submit all pages
/// 3. One miner receives a reward and the other gets discarded
///  - Note that both miners will submit an identical solution, so it is not deterministic who will
///    be rewarded. However, that is not the focus of this test!
///
/// The `Rewarded` event is sent at the end of the SignedValidation phase. In contrast, the
/// Discarded event is expected to be sent in the next round after the miner calls
/// `clear_old_round_data()` upon detecting that its previous round's solution was not the winning
/// one.
///
/// Timeout's after 60 minutes then it's regarded as an error.
/// This timeout needs to be adjusted based on the settings on the SDK's staking-async side.
/// Consider factors such as the duration of the signed phase, the number of solutions that can be
/// verified in the SignedValidation phase, and the length of the Unsigned phase. A limit of 60
/// minutes should be reasonably conservative.
pub async fn wait_for_two_miners_solution(port: u16) -> anyhow::Result<()> {
	const MAX_DURATION_FOR_SUBMIT_SOLUTION: std::time::Duration =
		std::time::Duration::from_secs(60 * 60);

	let now = Instant::now();
	log::info!("Starting to wait for two miners' solutions on port {}", port);

	let api = loop {
		if let Ok(api) = ChainClient::from_url(&format!("ws://127.0.0.1:{}", port)).await {
			break api;
		}

		if now.elapsed() > MAX_DURATION_FOR_SUBMIT_SOLUTION {
			return Err(anyhow::anyhow!("Failed to connect to the API"));
		}

		tokio::time::sleep(std::time::Duration::from_secs(1)).await;
	};

	// Track Alice's progress
	let mut alice_score_submitted = false;
	let mut alice_pages_submitted = HashSet::new();
	let mut alice_all_pages_submitted = false;

	// Track Bob's progress
	let mut bob_score_submitted = false;
	let mut bob_pages_submitted = HashSet::new();
	let mut bob_all_pages_submitted = false;

	// Track final outcomes
	let mut rewarded_account: Option<AccountId32> = None;
	let mut discarded_account: Option<AccountId32> = None;

	let mut blocks_sub = api.blocks().subscribe_finalized().await?;
	let pages = api
		.constants()
		.at(&runtime::constants().multi_block_election().pages())
		.unwrap();

	'outer: while let Some(block) = blocks_sub.next().await {
		if now.elapsed() > MAX_DURATION_FOR_SUBMIT_SOLUTION {
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

			// Check for reward events - this is sent by the chain upon successful verification of a
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

			// Check for discard events - this is sent by the chain at the next round after a miner
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

			// Check if we have both outcomes
			if rewarded_account.is_some() && discarded_account.is_some() {
				let rewarded = rewarded_account.as_ref().unwrap();
				let discarded = discarded_account.as_ref().unwrap();

				// Verify that both miners submitted scores and pages
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

				if rewarded_submitted_properly && discarded_submitted_properly {
					log::info!(
						"🤑 Successfully completed two-miner test: both submitted solutions, one rewarded, one discarded! Duration: {:?} 🤑",
						now.elapsed()
					);
					return Ok(());
				} else {
					log::error!(
						"Inconsistent state: rewarded properly: {}, discarded properly: {}",
						rewarded_submitted_properly,
						discarded_submitted_properly
					);
					break 'outer;
				}
			}
		}
	}

	log::info!("Failed to complete two-miner test after {:?}", now.elapsed());
	Err(anyhow::anyhow!(
		"Two-miner test failed: Alice - score: {}, pages: {}/{}, Bob - score: {}, pages: {}/{}, rewarded: {:?}, discarded: {:?}; timeout after {}s",
		alice_score_submitted,
		alice_pages_submitted.len(),
		pages,
		bob_score_submitted,
		bob_pages_submitted.len(),
		pages,
		rewarded_account.is_some(),
		discarded_account.is_some(),
		MAX_DURATION_FOR_SUBMIT_SOLUTION.as_secs()
	))
}

// TODO: the zombienet process starts multiple child processes: 2 relay chain nodes and a
// polkadot-parachain node. Each of these further spawns additional child processes like workers.
// The `KillChildOnDrop` is only killing the zombienet process but killing a parent process doesn't
// automatically kill all its children. E.g. on most Linux systems like Ubuntu, child processes are
// re-parented to the init process. Since the test is meant to run on a docker container
// environment, proper cleanup is less crucial. For local testing, it's up to the tester to simply
// kill any lingering processes before starting new tests. A better platform-specific solution on
// CI/CD could leverage process grouping so that if process are running in the same process group,
// sending a signal to the group will terminate all processes in the group.
async fn run_zombienet() -> (KillChildOnDrop, u16) {
	// First, parse the zombienet config file to get the RPC port
	let config_path = "zombienet-staking-runtimes.toml";
	let config_content =
		std::fs::read_to_string(config_path).expect("Failed to read zombienet config file");
	let rpc_port = extract_parachain_rpc_port(&config_content)
		.expect("Failed to extract RPC port from zombienet config");

	log::trace!("Found parachain collator RPC port in config: {}", rpc_port);

	// zombienet --provider native -l text spawn zombienet-staking-runtimes.toml
	let mut node_cmd = KillChildOnDrop(
		std::process::Command::new("zombienet")
			.stdout(Stdio::piped())
			.stderr(Stdio::piped())
			.args(["--provider", "native", "-l", "text", "spawn", config_path])
			.env("RUST_LOG", "runtime::multiblock-election=trace")
			.spawn()
			.unwrap(),
	);

	let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
	spawn_cli_output_threads(node_cmd.stdout.take().unwrap(), node_cmd.stderr.take().unwrap(), tx);

	tokio::spawn(async move {
		while let Some(line) = rx.recv().await {
			log::info!("{}", line);
		}
	});

	log::info!("Waiting for parachain collator on port {} to be ready...", rpc_port);

	// Wait for parachain collator to be ready with a 2 minute timeout
	let addr = format!("127.0.0.1:{}", rpc_port);
	let ws_url = format!("ws://{}", addr);

	match tokio::time::timeout(
		std::time::Duration::from_secs(120), // 2 minute timeout
		ChainClient::from_url(&ws_url),
	)
	.await
	{
		Ok(Ok(_)) => {
			log::info!("Parachain collator on port {} is up and running", rpc_port);
		},
		Ok(Err(e)) => {
			log::trace!("Connected to port but WebSocket initialization failed: {}", e);
			// We still continue as the node might be partially ready
		},
		Err(_) => {
			panic!("Timed out waiting for parachain collator on port {} to be ready", rpc_port);
		},
	}

	(node_cmd, rpc_port)
}

// Function to extract the parachain collator's RPC port from the config file content
fn extract_parachain_rpc_port(config_content: &str) -> Option<u16> {
	// Look for the [parachains.collator] section and then find rpc_port
	let collator_section = config_content.split("[parachains.collator]").nth(1)?;
	let port = collator_section
		.lines()
		.find(|line| line.trim().starts_with("rpc_port"))?
		.split('=')
		.nth(1)?
		.trim()
		.parse::<u16>()
		.ok()?;

	Some(port)
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
