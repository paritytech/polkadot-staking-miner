use staking_miner::opt::Chain;
use std::{
	io::{BufRead, BufReader, Read},
	net::SocketAddr,
	ops::{Deref, DerefMut},
	process::{self, Child, ChildStderr, ChildStdout},
};
use tracing_subscriber::EnvFilter;

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
		.take(50)
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
			let addr_str = line_end.split_once(",").unwrap().0;

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
			.args(&["--dev"])
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
	let chain_str = format!("{}-dev", chain.to_string());

	let mut node_cmd = KillChildOnDrop(
		process::Command::new("polkadot")
			.stdout(process::Stdio::piped())
			.stderr(process::Stdio::piped())
			.args(&[
				"--chain",
				&chain_str,
				"--tmp",
				"--alice",
				"--execution",
				"Native",
				"--offchain-worker=Never",
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
		for line in BufReader::new(stdout).lines() {
			if let Ok(line) = line {
				println!("OK: {}", line);
				let _ = tx2.send(line);
			}
		}
	});

	std::thread::spawn(move || {
		for line in BufReader::new(stderr).lines() {
			if let Ok(line) = line {
				println!("ERR: {}", line);
				let _ = tx.send(line);
			}
		}
	});
}
