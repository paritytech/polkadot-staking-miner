use assert_cmd::cargo::cargo_bin;
use polkadot_staking_miner::{
	opt::Chain,
	prelude::{runtime, ChainClient},
};
use std::{
	io::{BufRead, BufReader, Read},
	net::SocketAddr,
	ops::{Deref, DerefMut},
	process::{self, Child, ChildStderr, ChildStdout},
	time::{Duration, Instant},
};
use tracing_subscriber::EnvFilter;

pub use runtime::{
	election_provider_multi_phase::events::SolutionStored,
	runtime_types::pallet_election_provider_multi_phase::{ElectionCompute, ReadySolution},
};

pub const MAX_DURATION_FOR_SUBMIT_SOLUTION: Duration = Duration::from_secs(6 * 60);

pub fn init_logger() {
	let _ = tracing_subscriber::fmt()
		.with_env_filter(EnvFilter::from_default_env())
		.try_init();
}

/// Read the WS address from the output.
///
/// This is hack to get the actual sockaddr because
/// substrate assigns a random port if the specified port was already binded.
pub fn find_ws_url_from_output(read: impl Read + Send) -> (String, String) {
	let mut data = String::new();

	let ws_url = BufReader::new(read)
		.lines()
		.take(1024 * 1024)
		.find_map(|line| {
			let line =
				line.expect("failed to obtain next line from stdout for WS address discovery");
			log::info!("{}", line);

			data.push_str(&line);

			// Read socketaddr from output "Running JSON-RPC server: addr=127.0.0.1:9944, allowed origins=["*"]"
			let line_end = line
				.rsplit_once("Running JSON-RPC WS server: addr=")
				// newest message (jsonrpsee merging http and ws servers):
				.or_else(|| line.rsplit_once("Running JSON-RPC server: addr="))
				.map(|(_, line)| line)?;

			// get the socketaddr only.
			let addr_str = line_end.split_once(',').unwrap().0;

			// expect a valid sockaddr.
			let addr: SocketAddr = addr_str
				.parse()
				.unwrap_or_else(|_| panic!("valid SocketAddr expected but got '{addr_str}'"));

			Some(format!("ws://{}", addr))
		})
		.expect("We should get a WebSocket address");

	(ws_url, data)
}

pub fn run_staking_miner_playground() -> (KillChildOnDrop, String) {
	let mut node_cmd = KillChildOnDrop(
		process::Command::new("staking-miner-playground")
			.stdout(process::Stdio::piped())
			.stderr(process::Stdio::piped())
			.args(["--dev", "--offchain-worker=Never"])
			.env("RUST_LOG", "runtime=debug")
			.spawn()
			.unwrap(),
	);

	let stderr = node_cmd.stderr.take().unwrap();
	let (ws_url, _) = find_ws_url_from_output(stderr);
	(node_cmd, ws_url)
}

/// Start a polkadot node on chain polkadot-dev, kusama-dev or westend-dev.
pub fn run_polkadot_node(chain: Chain) -> (KillChildOnDrop, String) {
	let chain_str = match chain {
		Chain::Polkadot => "chainspecs/polkadot-dev.json",
		Chain::Kusama => "chainspecs/kusama-dev.json",
		Chain::Westend => "westend-dev",
	};

	let mut node_cmd = KillChildOnDrop(
		process::Command::new("polkadot")
			.stdout(process::Stdio::piped())
			.stderr(process::Stdio::piped())
			.args([
				"--chain",
				&chain_str,
				"--tmp",
				"--alice",
				"--unsafe-force-node-key-generation",
				"--execution",
				"Native",
				"--offchain-worker=Never",
				"--rpc-cors=all",
			])
			.spawn()
			.unwrap(),
	);

	let stderr = node_cmd.stderr.take().unwrap();
	let (ws_url, _) = find_ws_url_from_output(stderr);
	(node_cmd, ws_url)
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
		for line in BufReader::new(stdout).lines().flatten() {
			println!("OK: {}", line);
			let _ = tx2.send(line);
		}
	});

	std::thread::spawn(move || {
		for line in BufReader::new(stderr).lines().flatten() {
			println!("ERR: {}", line);
			let _ = tx.send(line);
		}
	});
}

pub enum Target {
	Node(Chain),
	StakingMinerPlayground,
}

pub async fn test_submit_solution(target: Target) {
	let (_drop, ws_url) = match target {
		Target::Node(chain) => run_polkadot_node(chain),
		Target::StakingMinerPlayground => run_staking_miner_playground(),
	};

	let mut miner = KillChildOnDrop(
		process::Command::new(cargo_bin(env!("CARGO_PKG_NAME")))
			.stdout(process::Stdio::piped())
			.stderr(process::Stdio::piped())
			.args(["--uri", &ws_url, "monitor", "--seed-or-path", "//Alice", "seq-phragmen"])
			.spawn()
			.unwrap(),
	);

	let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
	spawn_cli_output_threads(miner.stdout.take().unwrap(), miner.stderr.take().unwrap(), tx);

	tokio::spawn(async move {
		let r = rx.recv().await.unwrap();
		log::info!("{}", r);
	});

	let ready_solution = wait_for_mined_solution(&ws_url).await.unwrap();
	assert!(ready_solution.compute == ElectionCompute::Signed);
}

/// Wait until a solution is ready on chain
///
/// Timeout's after 6 minutes then it's regarded as an error.
pub async fn wait_for_mined_solution(ws_url: &str) -> anyhow::Result<SolutionStored> {
	let api = ChainClient::from_url(&ws_url).await?;
	let now = Instant::now();

	let mut blocks_sub = api.blocks().subscribe_finalized().await?;

	while let Some(block) = blocks_sub.next().await {
		if now.elapsed() > MAX_DURATION_FOR_SUBMIT_SOLUTION {
			break;
		}

		let block = block?;
		let events = block.events().await?;

		for ev in events.iter() {
			let ev = ev?;

			if let Some(solution_ev) = ev.as_event::<SolutionStored>()? {
				return Ok(solution_ev);
			}
		}
	}

	Err(anyhow::anyhow!(
		"ReadySolution not found in {}s regarded as error",
		MAX_DURATION_FOR_SUBMIT_SOLUTION.as_secs()
	))
}
