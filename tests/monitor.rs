//! Requires a `polkadot binary ` built with `--features fast-runtime` in the path to run integration tests against.

pub mod common;

use assert_cmd::cargo::cargo_bin;
use common::{
	init_logger, run_staking_miner_playground, spawn_cli_output_threads, test_submit_solution,
	wait_for_mined_solution, ElectionCompute, KillChildOnDrop, Target,
	MAX_DURATION_FOR_SUBMIT_SOLUTION,
};
use polkadot_staking_miner::opt::Chain;
use regex::Regex;
use std::{process, time::Instant};

#[tokio::test]
async fn submit_monitor_works_basic() {
	init_logger();
	// TODO: https://github.com/paritytech/staking-miner-v2/issues/673
	// test_submit_solution(Target::Node(Chain::Polkadot)).await;
	// test_submit_solution(Target::Node(Chain::Kusama)).await;
	// TODO: https://github.com/paritytech/polkadot-staking-miner/issues/806
	// test_submit_solution(Target::StakingMinerPlayground).await;
	test_submit_solution(Target::Node(Chain::Westend)).await;
}

// TODO: https://github.com/paritytech/polkadot-staking-miner/issues/806
#[tokio::test]
#[ignore]
async fn default_trimming_works() {
	init_logger();
	let (_drop, ws_url) = run_staking_miner_playground();
	let mut miner = KillChildOnDrop(
		process::Command::new(cargo_bin(env!("CARGO_PKG_NAME")))
			.stdout(process::Stdio::piped())
			.stderr(process::Stdio::piped())
			.env("RUST_LOG", "runtime=debug,polkadot-staking-miner=debug")
			.args(["--uri", &ws_url, "monitor", "--seed-or-path", "//Alice", "seq-phragmen"])
			.spawn()
			.unwrap(),
	);

	let ready_solution_task = tokio::spawn(async move { wait_for_mined_solution(&ws_url).await });

	assert!(has_trimming_output(&mut miner).await);

	let ready_solution =
		ready_solution_task.await.unwrap().expect("A solution should be mined now");
	assert!(ready_solution.compute == ElectionCompute::Signed);
}

// Helper that parses the CLI output to find logging outputs based on the following:
//
// i) DEBUG runtime::election-provider: 🗳 from 934 assignments, truncating to 1501 for weight, removing 0
// ii) DEBUG runtime::election-provider: 🗳 from 931 assignments, truncating to 755 for length, removing 176
//
// Thus, the only way to ensure that trimming actually works.
async fn has_trimming_output(miner: &mut KillChildOnDrop) -> bool {
	let trimming_re = Regex::new(
		r#"from (\d+) assignments, truncating to (\d+) for (?P<target>weight|length), removing (?P<removed>\d+)"#,
	)
	.unwrap();

	let mut got_truncate_len = false;
	let mut got_truncate_weight = false;

	let now = Instant::now();
	let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();

	spawn_cli_output_threads(miner.stdout.take().unwrap(), miner.stderr.take().unwrap(), tx);

	while !got_truncate_weight || !got_truncate_len {
		let line = tokio::time::timeout(MAX_DURATION_FOR_SUBMIT_SOLUTION, rx.recv())
			.await
			.expect("Logger timeout; no items produced")
			.expect("Logger channel dropped");
		println!("{line}");
		log::info!("{line}");

		if let Some(caps) = trimming_re.captures(&line) {
			let trimmed_items: usize = caps.name("removed").unwrap().as_str().parse().unwrap();

			if caps.name("target").unwrap().as_str() == "weight" && trimmed_items > 0 {
				got_truncate_weight = true;
			}

			if caps.name("target").unwrap().as_str() == "length" && trimmed_items > 0 {
				got_truncate_len = true;
			}
		}

		if now.elapsed() > MAX_DURATION_FOR_SUBMIT_SOLUTION {
			break
		}
	}

	assert!(got_truncate_weight, "Trimming weight logs were not found");
	assert!(got_truncate_len, "Trimming length logs were not found");

	got_truncate_len && got_truncate_weight
}
