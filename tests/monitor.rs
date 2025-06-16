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
		multi_block_election_signed::events::{Registered, Rewarded, Stored},
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
	let _miner = run_miner(port);
	assert!(wait_for_mined_solution(port).await.is_ok());
}

fn run_miner(port: u16) -> KillChildOnDrop {
	log::info!("Starting miner");

	let mut miner = KillChildOnDrop(
		std::process::Command::new(cargo_bin(env!("CARGO_PKG_NAME")))
			.stdout(Stdio::piped())
			.stderr(Stdio::piped())
			.args([
				"--uri",
				&format!("ws://127.0.0.1:{}", port),
				"monitor",
				"--seed-or-path",
				"//Alice",
			])
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

/// Wait until a solution is ready on chain, all pages are submitted, and the user is rewarded.
/// This function strictly checks that:
/// 1. The solution score is registered
/// 2. All solution pages are successfully submitted
/// 3. The user receives a reward after satisfying conditions 1 and 2
///
/// Timeout's after 40 minutes then it's regarded as an error.
pub async fn wait_for_mined_solution(port: u16) -> anyhow::Result<()> {
	const MAX_DURATION_FOR_SUBMIT_SOLUTION: std::time::Duration =
		std::time::Duration::from_secs(60 * 40);

	let now = Instant::now();
	log::info!("Starting to wait for a mined solution on port {}", port);

	let api = loop {
		if let Ok(api) = ChainClient::from_url(&format!("ws://127.0.0.1:{}", port)).await {
			break api;
		}

		if now.elapsed() > MAX_DURATION_FOR_SUBMIT_SOLUTION {
			return Err(anyhow::anyhow!("Failed to connect to the API"));
		}

		tokio::time::sleep(std::time::Duration::from_secs(1)).await;
	};

	let mut score_submitted = false;
	let mut pages_submitted = HashSet::new();
	let mut all_pages_submitted = false;

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
					log::info!("Score registered");
					score_submitted = true;
				}
			}

			// Page submission.
			if let Some(item) = ev.as_event::<Stored>()? {
				if item.1 == alice() {
					log::info!(
						"Adding page {} to pages_submitted (was size {})",
						item.2,
						pages_submitted.len()
					);
					pages_submitted.insert(item.2);
				}
			}

			if !all_pages_submitted && pages_submitted.len() == pages as usize {
				log::info!("All pages submitted");
				all_pages_submitted = true;
			}

			// Verify that the user receives a reward.
			if let Some(item) = ev.as_event::<Rewarded>()? {
				if item.1 == alice() {
					log::info!("Rewarded!");

					if score_submitted && all_pages_submitted {
						log::info!(
							"🤑 Successfully mined solution: score registered, all {} pages submitted, and user rewarded! Duration: {:?} 🤑",
							pages,
							now.elapsed()
						);
						return Ok(());
					}
					// It should never happen that the user gets rewarded without score and all
					// pages being submitted. Log the error outside both loops.
					break 'outer;
				}
			}
		}
	}

	log::info!("Failed to mine solution after {:?}", now.elapsed());
	Err(anyhow::anyhow!(
		"Test failed: score_submitted: {}, all_pages_submitted: {} ({}/{} pages); timeout after {}s",
		score_submitted,
		all_pages_submitted,
		pages_submitted.len(),
		pages,
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
