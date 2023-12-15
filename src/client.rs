use crate::prelude::*;
use jsonrpsee::ws_client::WsClientBuilder;
use subxt::backend::rpc::RpcClient as RawRpcClient;

/// Wraps the subxt interfaces to make it easy to use for the staking-miner.
#[derive(Clone, Debug)]
pub struct Client {
	/// Access to typed rpc calls from subxt.
	rpc: RpcClient,
	/// Access to chain APIs such as storage, events etc.
	chain_api: ChainClient,
	/// Raw RPC client.
	raw_rpc: RawRpcClient,
}

impl Client {
	pub async fn new(uri: &str) -> Result<Self, subxt::Error> {
		log::debug!(target: LOG_TARGET, "attempting to connect to {:?}", uri);

		let rpc = loop {
			match WsClientBuilder::default()
				.max_request_size(u32::MAX)
				.max_response_size(u32::MAX)
				.request_timeout(std::time::Duration::from_secs(600))
				.build(&uri)
				.await
			{
				Ok(rpc) => break RawRpcClient::new(rpc),
				Err(e) => {
					log::warn!(
						target: LOG_TARGET,
						"failed to connect to client due to {:?}, retrying soon..",
						e,
					);
				},
			};
			tokio::time::sleep(std::time::Duration::from_millis(2_500)).await;
		};

		let chain_api = ChainClient::from_rpc_client(rpc.clone()).await?;
		Ok(Self { rpc: RpcClient::new(rpc.clone()), raw_rpc: rpc, chain_api })
	}

	/// Get a reference to the RPC interface exposed by subxt.
	pub fn rpc(&self) -> &RpcClient {
		&self.rpc
	}

	/// Get a reference to the chain API.
	pub fn chain_api(&self) -> &ChainClient {
		&self.chain_api
	}

	/// Get a reference to the raw rpc client API.
	pub fn raw_rpc(&self) -> &RawRpcClient {
		&self.raw_rpc
	}
}
