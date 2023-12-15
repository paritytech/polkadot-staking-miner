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

use super::prelude::LOG_TARGET;
use futures::channel::oneshot;
pub use hidden::*;
use hyper::{
	header::CONTENT_TYPE,
	service::{make_service_fn, service_fn},
	Body, Method, Request, Response,
};
use prometheus::{Encoder, TextEncoder};

async fn serve_req(req: Request<Body>) -> Result<Response<Body>, hyper::Error> {
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

pub struct GracefulShutdown(Option<oneshot::Sender<()>>);

impl Drop for GracefulShutdown {
	fn drop(&mut self) {
		if let Some(handle) = self.0.take() {
			let _ = handle.send(());
		}
	}
}

pub fn run(port: u16) -> Result<GracefulShutdown, String> {
	let (tx, rx) = oneshot::channel();

	// For every connection, we must make a `Service` to handle all incoming HTTP requests on said
	// connection.
	let make_svc = make_service_fn(move |_conn| async move {
		Ok::<_, std::convert::Infallible>(service_fn(serve_req))
	});

	let addr = ([0, 0, 0, 0], port).into();
	let server = hyper::Server::try_bind(&addr)
		.map_err(|e| format!("Failed bind socket on port {} {:?}", port, e))?
		.serve(make_svc);

	log::info!(target: LOG_TARGET, "Started prometheus endpoint on http://{}", addr);

	let graceful = server.with_graceful_shutdown(async {
		rx.await.ok();
	});

	tokio::spawn(async move {
		if let Err(e) = graceful.await {
			log::warn!("Server error: {}", e);
		}
	});

	Ok(GracefulShutdown(Some(tx)))
}

mod hidden {
	use frame_election_provider_support::Weight;
	use once_cell::sync::Lazy;
	use prometheus::{opts, register_counter, register_gauge, Counter, Gauge};

	static TRIMMED_SOLUTION_STARTED: Lazy<Counter> = Lazy::new(|| {
		register_counter!(opts!(
			"staking_miner_trim_started",
			"Number of started trimmed solutions",
		))
		.unwrap()
	});

	static TRIMMED_SOLUTION_SUCCESS: Lazy<Counter> = Lazy::new(|| {
		register_counter!(opts!(
			"staking_miner_trim_success",
			"Number of successful trimmed solutions",
		))
		.unwrap()
	});

	static SUBMISSIONS_STARTED: Lazy<Counter> = Lazy::new(|| {
		register_counter!(opts!(
			"staking_miner_submissions_started",
			"Number of submissions started",
		))
		.unwrap()
	});

	static SUBMISSIONS_SUCCESS: Lazy<Counter> = Lazy::new(|| {
		register_counter!(opts!(
			"staking_miner_submissions_success",
			"Number of submissions finished successfully",
		))
		.unwrap()
	});
	static MINED_SOLUTION_DURATION: Lazy<Gauge> = Lazy::new(|| {
		register_gauge!(
			"staking_miner_mining_duration_ms",
			"The mined solution time in milliseconds."
		)
		.unwrap()
	});
	static SUBMIT_SOLUTION_AND_WATCH_DURATION: Lazy<Gauge> = Lazy::new(|| {
		register_gauge!(
			"staking_miner_submit_and_watch_duration_ms",
			"The time in milliseconds it took to submit the solution to chain and to be included in block",
		)
		.unwrap()
	});
	static BALANCE: Lazy<Gauge> = Lazy::new(|| {
		register_gauge!(opts!("staking_miner_balance", "The balance of the staking miner account"))
			.unwrap()
	});
	static SCORE_MINIMAL_STAKE: Lazy<Gauge> = Lazy::new(|| {
		register_gauge!(opts!(
			"staking_miner_score_minimal_stake",
			"The minimal winner, in terms of total backing stake"
		))
		.unwrap()
	});
	static SCORE_SUM_STAKE: Lazy<Gauge> = Lazy::new(|| {
		register_gauge!(opts!(
			"staking_miner_score_sum_stake",
			"The sum of the total backing of all winners",
		))
		.unwrap()
	});
	static SCORE_SUM_STAKE_SQUARED: Lazy<Gauge> = Lazy::new(|| {
		register_gauge!(opts!(
			"staking_miner_score_sum_stake_squared",
			"The sum squared of the total backing of all winners, aka. the variance.",
		))
		.unwrap()
	});
	static RUNTIME_UPGRADES: Lazy<Counter> = Lazy::new(|| {
		register_counter!(opts!(
			"staking_miner_runtime_upgrades",
			"Number of runtime upgrades performed",
		))
		.unwrap()
	});
	static SUBMISSION_LENGTH: Lazy<Gauge> = Lazy::new(|| {
		register_gauge!(opts!(
			"staking_miner_solution_length_bytes",
			"Number of bytes in the solution submitted",
		))
		.unwrap()
	});
	static SUBMISSION_WEIGHT: Lazy<Gauge> = Lazy::new(|| {
		register_gauge!(opts!("staking_miner_solution_weight", "Weight of the solution submitted"))
			.unwrap()
	});

	#[allow(unused)]
	pub fn on_runtime_upgrade() {
		RUNTIME_UPGRADES.inc();
	}

	pub fn on_submission_attempt() {
		SUBMISSIONS_STARTED.inc();
	}

	pub fn on_submission_success() {
		SUBMISSIONS_SUCCESS.inc();
	}

	pub fn on_trim_attempt() {
		TRIMMED_SOLUTION_STARTED.inc();
	}

	pub fn on_trim_success() {
		TRIMMED_SOLUTION_SUCCESS.inc();
	}

	pub fn set_balance(balance: f64) {
		BALANCE.set(balance);
	}

	pub fn set_length(len: usize) {
		SUBMISSION_LENGTH.set(len as f64);
	}

	pub fn set_weight(weight: Weight) {
		SUBMISSION_WEIGHT.set(weight.ref_time() as f64)
	}

	pub fn set_score(score: sp_npos_elections::ElectionScore) {
		SCORE_MINIMAL_STAKE.set(score.minimal_stake as f64);
		SCORE_SUM_STAKE.set(score.sum_stake as f64);
		SCORE_SUM_STAKE_SQUARED.set(score.sum_stake_squared as f64);
	}

	pub fn observe_submit_and_watch_duration(time: f64) {
		SUBMIT_SOLUTION_AND_WATCH_DURATION.set(time);
	}

	pub fn observe_mined_solution_duration(time: f64) {
		MINED_SOLUTION_DURATION.set(time);
	}
}
