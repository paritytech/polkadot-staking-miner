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
//! Requires a `polkadot binary ` built with `--features fast-runtime` in the path to run integration tests against.

pub mod common;

use assert_cmd::cargo::cargo_bin;
use common::{init_logger, spawn_cli_output_threads, KillChildOnDrop};
use polkadot_staking_miner::{
    prelude::ChainClient,
    runtime::multi_block::{
        self as runtime,
        multi_block_signed::events::{Registered, Stored},
    },
};
use std::collections::HashSet;
use std::{process, time::Instant};
use subxt::utils::AccountId32;

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
        process::Command::new(cargo_bin(env!("CARGO_PKG_NAME")))
            .stdout(process::Stdio::piped())
            .stderr(process::Stdio::piped())
            .args([
                "--uri",
                &format!("ws://127.0.0.1:{}", port),
                "experimental-monitor-multi-block",
                "--seed-or-path",
                "//Alice",
            ])
            .env("RUST_LOG", "trace,polkadot_staking_miner=trace")
            .spawn()
            .unwrap(),
    );

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    spawn_cli_output_threads(
        miner.stdout.take().unwrap(),
        miner.stderr.take().unwrap(),
        tx,
    );

    tokio::spawn(async move {
        let r = rx.recv().await.unwrap();
        log::info!("{}", r);
    });

    miner
}

/// Wait until a solution is ready on chain
///
/// Timeout's after 6 minutes then it's regarded as an error.
pub async fn wait_for_mined_solution(port: u16) -> anyhow::Result<()> {
    const MAX_DURATION_FOR_SUBMIT_SOLUTION: std::time::Duration =
        std::time::Duration::from_secs(60 * 20);

    let now = Instant::now();

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

    let mut blocks_sub = api.blocks().subscribe_finalized().await?;
    let pages = api
        .constants()
        .at(&runtime::constants().multi_block().pages())
        .unwrap();

    while let Some(block) = blocks_sub.next().await {
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

            if score_submitted && pages_submitted.len() == pages as usize {
                log::info!("All pages submitted");
                return Ok(());
            }
        }
    }

    Err(anyhow::anyhow!(
        "score_submitted: {}, pages_submitted: {:?}; timeout after {}s",
        score_submitted,
        pages_submitted,
        MAX_DURATION_FOR_SUBMIT_SOLUTION.as_secs()
    ))
}

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
        process::Command::new("zombienet")
            .stdout(process::Stdio::piped())
            .stderr(process::Stdio::piped())
            .args(["--provider", "native", "-l", "text", "spawn", config_path])
            .env("RUST_LOG", "info,runtime=debug")
            .spawn()
            .unwrap(),
    );

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    spawn_cli_output_threads(
        node_cmd.stdout.take().unwrap(),
        node_cmd.stderr.take().unwrap(),
        tx,
    );

    tokio::spawn(async move {
        while let Some(line) = rx.recv().await {
            log::info!("{}", line);
        }
    });

    log::info!(
        "Waiting for parachain collator on port {} to be ready...",
        rpc_port
    );

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
        }
        Ok(Err(e)) => {
            log::trace!(
                "Connected to port but WebSocket initialization failed: {}",
                e
            );
            // We still continue as the node might be partially ready
        }
        Err(_) => {
            panic!(
                "Timed out waiting for parachain collator on port {} to be ready",
                rpc_port
            );
        }
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
