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

use std::sync::Arc;

use futures::channel::oneshot;
use hyper::{
	header::CONTENT_TYPE,
	service::{make_service_fn, service_fn},
	Body, Method, Request, Response,
};
use opentelemetry::{
	global,
	metrics::{BoundCounter, BoundValueRecorder},
	KeyValue,
};
use opentelemetry_prometheus::PrometheusExporter;
use prometheus::{Encoder, TextEncoder};

lazy_static::lazy_static! {
	static ref HANDLER_ALL: [KeyValue; 1] = [KeyValue::new("handler", "all")];
}

pub struct AppState {
	exporter: PrometheusExporter,
	pub submissions: BoundCounter<u64>,
	pub mined_solutions: BoundValueRecorder<u64>,
	pub balance: BoundValueRecorder<f64>,
}

pub type MetricsState = Arc<AppState>;

async fn serve_req(
	req: Request<Body>,
	state: Arc<AppState>,
) -> Result<Response<Body>, hyper::Error> {
	let response = match (req.method(), req.uri().path()) {
		(&Method::GET, "/metrics") => {
			let mut buffer = vec![];
			let encoder = TextEncoder::new();
			let metric_families = state.exporter.registry().gather();
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

pub fn run() -> (MetricsState, oneshot::Sender<()>) {
	let (tx, rx) = oneshot::channel();

	let exporter = opentelemetry_prometheus::exporter().init();

	let meter = global::meter("staking-miner/metrics");
	let state = Arc::new(AppState {
		exporter,
		submissions: meter
			.u64_counter("staking_miner.submissions")
			.with_description("Total number of submissions made")
			.init()
			.bind(HANDLER_ALL.as_ref()),
		mined_solutions: meter
			.u64_value_recorder("staking_miner.mined_solutions")
			.with_description("The time in us to mine a solution")
			.init()
			.bind(HANDLER_ALL.as_ref()),
		balance: meter
			.f64_value_recorder("staking_miner.balance")
			.with_description("Balance of the staking miner account")
			.init()
			.bind(HANDLER_ALL.as_ref()),
	});

	let state2 = state.clone();
	// For every connection, we must make a `Service` to handle all
	// incoming HTTP requests on said connection.
	let make_svc = make_service_fn(move |_conn| {
		let state = state2.clone();
		// This is the `Service` that will handle the connection.
		// `service_fn` is a helper to convert a function that
		// returns a Response into a `Service`.
		async move {
			Ok::<_, std::convert::Infallible>(service_fn(move |req| serve_req(req, state.clone())))
		}
	});

	let addr = ([127, 0, 0, 1], 3000).into();
	let server = hyper::Server::bind(&addr).serve(make_svc);

	log::info!("Started prometheus endpoint on http://{}", addr);

	let graceful = server.with_graceful_shutdown(async {
		rx.await.ok();
	});

	tokio::spawn(async move {
		if let Err(e) = graceful.await {
			log::warn!("server error: {}", e);
		}
	});

	(state, tx)
}
