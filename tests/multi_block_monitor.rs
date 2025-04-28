#![cfg(feature = "integration-tests")]
//! Integration tests for the multi-block monitor (pallet-election-multi-block)
//! which requires the following artifacts to be installed in the path:
//! 1. `zombienet`
//! 2. `polkadot --features fast-runtime`
//! 3. `polkadot-parachain`
//! 4. `polkadot-omni-node`
//! 6. `chainspecs (rc.json and parachain.json)`
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

    let _node = run_zombienet().await;
    let _miner = run_miner();
    assert!(wait_for_mined_solution().await.is_ok());
}

fn run_miner() -> KillChildOnDrop {
    log::info!("Starting miner");

    let mut miner = KillChildOnDrop(
        process::Command::new(cargo_bin(env!("CARGO_PKG_NAME")))
            .stdout(process::Stdio::piped())
            .stderr(process::Stdio::piped())
            .args([
                "--uri",
                "ws://localhost:9966",
                "experimental-monitor-multi-block",
                "--seed-or-path",
                "//Alice",
            ])
            .env("RUST_LOG", "info,polkadot_staking_miner=trace")
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
pub async fn wait_for_mined_solution() -> anyhow::Result<()> {
    const MAX_DURATION_FOR_SUBMIT_SOLUTION: std::time::Duration =
        std::time::Duration::from_secs(60 * 20);

    let now = Instant::now();

    let api = loop {
        if let Ok(api) = ChainClient::from_url("ws://localhost:9966").await {
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
                    score_submitted = true;
                }
            }

            // Page submission.
            if let Some(item) = ev.as_event::<Stored>()? {
                if item.1 == alice() {
                    pages_submitted.insert(item.0);
                }
            }

            if score_submitted && pages_submitted.len() == pages as usize {
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

async fn run_zombienet() -> KillChildOnDrop {
    // zombienet --provider native -l text spawn zombienet-staking-runtimes.toml
    let mut node_cmd = KillChildOnDrop(
        process::Command::new("zombienet")
            .stdout(process::Stdio::piped())
            .stderr(process::Stdio::piped())
            .args([
                "--provider",
                "native",
                "-l",
                "text",
                "spawn",
                "zombienet-staking-runtimes.toml",
            ])
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

    while let Some(line) = rx.recv().await {
        log::info!("{}", line);

        // We regard this as zombienet started and we can start the miner.
        if line.to_lowercase().contains("node information") {
            break;
        }
    }

    node_cmd
}

fn alice() -> AccountId32 {
    use polkadot_sdk::sp_core::crypto::Pair;

    let pair = polkadot_staking_miner::prelude::Pair::from_string("//Alice", None).unwrap();
    let public = pair.public();
    AccountId32::from(public.0)
}
