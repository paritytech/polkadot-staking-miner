// Copyright 2021-2022 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

use crate::prelude::LOG_TARGET;
use http_body_util::Full;
use hyper::{Method, Request, Response, body::Bytes, header::CONTENT_TYPE, service::service_fn};
use hyper_util::{
	rt::{TokioExecutor, TokioIo},
	server::conn::auto::Builder,
};
use prometheus::{Encoder, TextEncoder};
use std::net::SocketAddr;
use tokio::net::TcpListener;

type Body = Full<Bytes>;

async fn serve_req(req: Request<hyper::body::Incoming>) -> Result<Response<Body>, hyper::Error> {
	let response = match (req.method(), req.uri().path()) {
		(&Method::GET, "/metrics") => {
			let mut buffer = vec![];
			let encoder = TextEncoder::new();
			let metric_families = prometheus::gather();
			encoder.encode(&metric_families, &mut buffer).unwrap();

			Response::builder()
				.status(200)
				.header(CONTENT_TYPE, encoder.format_type())
				.body(Body::from(buffer))
				.unwrap()
		},
		(&Method::GET, "/") => Response::builder().status(200).body(Body::from("")).unwrap(),
		_ => Response::builder().status(404).body(Body::from("")).unwrap(),
	};

	Ok(response)
}

pub async fn run(port: u16) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
	let addr = SocketAddr::from(([0, 0, 0, 0], port));
	run_server(addr).await
}

pub fn on_runtime_upgrade() {
	// No-op for dummy miner
}

pub fn on_updater_subscription_stall() {
	// No-op for dummy miner
}

pub async fn run_server(addr: SocketAddr) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
	let listener = TcpListener::bind(addr).await?;

	loop {
		let (stream, _) = listener.accept().await?;
		let io = TokioIo::new(stream);

		tokio::spawn(async move {
			let builder = Builder::new(TokioExecutor::new());
			let conn = builder.serve_connection(io, service_fn(serve_req));

			if let Err(err) = conn.await {
				log::error!(target: LOG_TARGET, "Error serving connection: {err:?}");
			}

			log::trace!(target: LOG_TARGET, "Connection dropped");
		});
	}
}
