use staking_miner::prelude::Chain;
use std::{
	io::{BufRead, BufReader, Read},
	ops::{Deref, DerefMut},
	process::{self, Child},
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
		.find_map(|line| {
			let line =
				line.expect("failed to obtain next line from stdout for WS address discovery");
			log::info!("{}", line);

			data.push_str(&line);

			// does the line contain our port (we expect this specific output from substrate).
			let sock_addr = match line.split_once("Running JSON-RPC WS server: addr=") {
				None => return None,
				Some((_, after)) => after.split_once(",").unwrap().0,
			};

			Some(format!("ws://{}", sock_addr))
		})
		.expect("We should get a WebSocket address");

	(ws_url, data)
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
