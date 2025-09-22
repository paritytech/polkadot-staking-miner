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
pub use hidden::*;
use http_body_util::Full;
use hyper::{Method, Request, Response, body::Bytes, header::CONTENT_TYPE, service::service_fn};
use hyper_util::{
	rt::{TokioExecutor, TokioIo},
	server::{conn::auto::Builder, graceful::GracefulShutdown},
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

pub async fn run(port: u16) -> Result<(), String> {
	// Create the address to bind to
	let addr = SocketAddr::from(([0, 0, 0, 0], port));

	// Bind the TCP listener
	let listener = TcpListener::bind(&addr)
		.await
		.map_err(|e| format!("Failed to bind socket on port {port}: {e:?}"))?;

	log::info!(target: LOG_TARGET, "Started prometheus endpoint on http://{addr}");

	// Create a graceful shutdown handler
	let graceful = GracefulShutdown::new();

	// Spawn the server task
	tokio::spawn(async move {
		let executor = TokioExecutor::new();
		let server = Builder::new(executor);

		loop {
			match listener.accept().await {
				Ok((stream, _)) => {
					let io = TokioIo::new(stream);

					// Create a service for this connection
					let service = service_fn(serve_req);

					// Serve the connection with graceful shutdown
					let conn = server.serve_connection_with_upgrades(io, service).into_owned();

					let conn = graceful.watch(conn);

					tokio::spawn(async move {
						if let Err(err) = conn.await {
							log::debug!(target: LOG_TARGET, "connection error: {err:?}");
						}
					});
				},
				Err(e) => {
					log::debug!(target: LOG_TARGET, "Error accepting connection: {e:?}");
					continue;
				},
			}
		}
	});

	Ok(())
}

mod hidden {

	use once_cell::sync::Lazy;
	use polkadot_sdk::sp_npos_elections;
	use prometheus::{Counter, Gauge, opts, register_counter, register_gauge};

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

	static CLEAR_OLD_ROUNDS_CLEANUP_SUCCESS: Lazy<Counter> = Lazy::new(|| {
		register_counter!(opts!(
			"staking_miner_clear_old_rounds_cleanup_success_total",
			"Total number of successful clear old rounds cleanup operations"
		))
		.unwrap()
	});

	static CLEAR_OLD_ROUNDS_CLEANUP_FAILURES: Lazy<Counter> = Lazy::new(|| {
		register_counter!(opts!(
			"staking_miner_clear_old_rounds_cleanup_failures_total",
			"Total number of failed clear old rounds cleanup operations"
		))
		.unwrap()
	});

	static CLEAR_OLD_ROUNDS_OLD_SUBMISSIONS_FOUND: Lazy<Gauge> = Lazy::new(|| {
		register_gauge!(opts!(
			"staking_miner_clear_old_rounds_old_submissions_found",
			"Number of old submissions found during last clear old rounds run"
		))
		.unwrap()
	});

	static CLEAR_OLD_ROUNDS_OLD_SUBMISSIONS_CLEARED: Lazy<Gauge> = Lazy::new(|| {
		register_gauge!(opts!(
			"staking_miner_clear_old_rounds_old_submissions_cleared",
			"Number of old submissions successfully cleared during last clear old rounds run"
		))
		.unwrap()
	});

	static CLEAR_OLD_ROUNDS_CLEANUP_DURATION: Lazy<Gauge> = Lazy::new(|| {
		register_gauge!(
			"staking_miner_clear_old_rounds_cleanup_duration_ms",
			"The time in milliseconds it takes to complete clear old rounds cleanup"
		)
		.unwrap()
	});

	static ERA_PRUNING_SUBMISSIONS_SUCCESS: Lazy<Counter> = Lazy::new(|| {
		register_counter!(opts!(
			"staking_miner_era_pruning_submissions_success_total",
			"Total number of successful prune_era_step submissions"
		))
		.unwrap()
	});

	static ERA_PRUNING_SUBMISSIONS_FAILURES: Lazy<Counter> = Lazy::new(|| {
		register_counter!(opts!(
			"staking_miner_era_pruning_submissions_failures_total",
			"Total number of failed prune_era_step submissions"
		))
		.unwrap()
	});

	static ERA_PRUNING_STORAGE_MAP_SIZE: Lazy<Gauge> = Lazy::new(|| {
		register_gauge!(opts!(
			"staking_miner_era_pruning_storage_map_size",
			"Current size of the era pruning storage map"
		))
		.unwrap()
	});

	static LISTENER_SUBSCRIPTION_STALLS: Lazy<Counter> = Lazy::new(|| {
		register_counter!(opts!(
			"staking_miner_listener_subscription_stalls_total",
			"Total number of times the listener subscription was detected as stalled and recreated"
		))
		.unwrap()
	});

	static UPDATER_SUBSCRIPTION_STALLS: Lazy<Counter> = Lazy::new(|| {
		register_counter!(opts!(
			"staking_miner_updater_subscription_stalls_total",
			"Total number of times the updater subscription was detected as stalled and recreated"
		))
		.unwrap()
	});

	static STORAGE_QUERY_DURATION: Lazy<Gauge> = Lazy::new(|| {
		register_gauge!(
			"staking_miner_storage_query_duration_ms",
			"Duration of storage queries in milliseconds"
		)
		.unwrap()
	});

	static BLOCK_STATE_DURATION: Lazy<Gauge> = Lazy::new(|| {
		register_gauge!(
			"staking_miner_block_state_duration_ms",
			"Duration of get_block_state() calls in milliseconds"
		)
		.unwrap()
	});

	static BLOCK_DETAILS_DURATION: Lazy<Gauge> = Lazy::new(|| {
		register_gauge!(
			"staking_miner_block_details_duration_ms",
			"Duration of BlockDetails::new() calls in milliseconds"
		)
		.unwrap()
	});
	static CHECK_EXISTING_SUBMISSION_DURATION: Lazy<Gauge> = Lazy::new(|| {
		register_gauge!(
			"staking_miner_check_existing_submission_duration_ms",
			"Duration of checking and handling existing submissions in milliseconds"
		)
		.unwrap()
	});
	static BAIL_DURATION: Lazy<Gauge> = Lazy::new(|| {
		register_gauge!(
			"staking_miner_bail_duration_ms",
			"Duration of bail operations in milliseconds"
		)
		.unwrap()
	});
	static BALANCE_FETCH_DURATION: Lazy<Gauge> = Lazy::new(|| {
		register_gauge!(
			"staking_miner_balance_fetch_duration_ms",
			"Duration of balance fetch operations in milliseconds"
		)
		.unwrap()
	});
	static PHASE_CHECK_DURATION: Lazy<Gauge> = Lazy::new(|| {
		register_gauge!(
			"staking_miner_phase_check_duration_ms",
			"Duration of phase check operations in milliseconds"
		)
		.unwrap()
	});
	static SCORE_CHECK_DURATION: Lazy<Gauge> = Lazy::new(|| {
		register_gauge!(
			"staking_miner_score_check_duration_ms",
			"Duration of score check operations in milliseconds"
		)
		.unwrap()
	});
	static MISSING_PAGES_DURATION: Lazy<Gauge> = Lazy::new(|| {
		register_gauge!(
			"staking_miner_missing_pages_duration_ms",
			"Duration of missing pages submission in milliseconds"
		)
		.unwrap()
	});

	static LAST_BLOCK_PROCESSING_TIME: Lazy<Gauge> = Lazy::new(|| {
		register_gauge!(opts!(
			"staking_miner_last_block_processing_timestamp",
			"Unix timestamp of when the last block was successfully processed by listener"
		))
		.unwrap()
	});

	static BLOCK_PROCESSING_STALLS: Lazy<Counter> = Lazy::new(|| {
		register_counter!(opts!(
			"staking_miner_block_processing_stalls_total",
			"Total number of times block processing was detected as stalled (different from subscription stalls)"
		))
		.unwrap()
	});
	static MINING_TIMEOUTS: Lazy<Counter> = Lazy::new(|| {
		register_counter!(opts!(
			"staking_miner_mining_timeouts_total",
			"Total number of solution mining timeouts"
		))
		.unwrap()
	});
	static CHECK_EXISTING_SUBMISSION_TIMEOUTS: Lazy<Counter> = Lazy::new(|| {
		register_counter!(opts!(
			"staking_miner_check_existing_submission_timeouts_total",
			"Total number of check existing submission timeouts"
		))
		.unwrap()
	});
	static BAIL_TIMEOUTS: Lazy<Counter> = Lazy::new(|| {
		register_counter!(opts!(
			"staking_miner_bail_timeouts_total",
			"Total number of bail operation timeouts"
		))
		.unwrap()
	});
	static SUBMIT_TIMEOUTS: Lazy<Counter> = Lazy::new(|| {
		register_counter!(opts!(
			"staking_miner_submit_timeouts_total",
			"Total number of solution submission timeouts"
		))
		.unwrap()
	});
	static BALANCE_FETCH_TIMEOUTS: Lazy<Counter> = Lazy::new(|| {
		register_counter!(opts!(
			"staking_miner_balance_fetch_timeouts_total",
			"Total number of balance fetch timeouts"
		))
		.unwrap()
	});
	static PHASE_CHECK_TIMEOUTS: Lazy<Counter> = Lazy::new(|| {
		register_counter!(opts!(
			"staking_miner_phase_check_timeouts_total",
			"Total number of phase check timeouts"
		))
		.unwrap()
	});
	static SCORE_CHECK_TIMEOUTS: Lazy<Counter> = Lazy::new(|| {
		register_counter!(opts!(
			"staking_miner_score_check_timeouts_total",
			"Total number of score check timeouts"
		))
		.unwrap()
	});
	static MISSING_PAGES_TIMEOUTS: Lazy<Counter> = Lazy::new(|| {
		register_counter!(opts!(
			"staking_miner_missing_pages_timeouts_total",
			"Total number of missing pages submission timeouts"
		))
		.unwrap()
	});

	static ERA_PRUNING_TIMEOUTS: Lazy<Counter> = Lazy::new(|| {
		register_counter!(opts!(
			"staking_miner_era_pruning_timeouts_total",
			"Total number of era pruning storage query timeouts"
		))
		.unwrap()
	});

	pub fn on_runtime_upgrade() {
		RUNTIME_UPGRADES.inc();
	}

	pub fn set_balance(balance: f64) {
		BALANCE.set(balance);
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

	pub fn on_submission_started() {
		SUBMISSIONS_STARTED.inc();
	}

	pub fn on_submission_success() {
		SUBMISSIONS_SUCCESS.inc();
	}

	pub fn on_clear_old_rounds_cleanup_success(cleared_count: u32) {
		CLEAR_OLD_ROUNDS_CLEANUP_SUCCESS.inc();
		CLEAR_OLD_ROUNDS_OLD_SUBMISSIONS_CLEARED.set(cleared_count as f64);
	}

	pub fn on_clear_old_rounds_cleanup_failure() {
		CLEAR_OLD_ROUNDS_CLEANUP_FAILURES.inc();
	}

	pub fn set_clear_old_rounds_old_submissions_found(count: u32) {
		CLEAR_OLD_ROUNDS_OLD_SUBMISSIONS_FOUND.set(count as f64);
	}

	pub fn observe_clear_old_rounds_cleanup_duration(time: f64) {
		CLEAR_OLD_ROUNDS_CLEANUP_DURATION.set(time);
	}

	pub fn on_era_pruning_submission_success() {
		ERA_PRUNING_SUBMISSIONS_SUCCESS.inc();
	}

	pub fn on_era_pruning_submission_failure() {
		ERA_PRUNING_SUBMISSIONS_FAILURES.inc();
	}

	pub fn set_era_pruning_storage_map_size(size: u32) {
		ERA_PRUNING_STORAGE_MAP_SIZE.set(size as f64);
	}

	pub fn on_listener_subscription_stall() {
		LISTENER_SUBSCRIPTION_STALLS.inc();
	}

	pub fn on_updater_subscription_stall() {
		UPDATER_SUBSCRIPTION_STALLS.inc();
	}

	pub fn observe_storage_query_duration(duration_ms: f64) {
		STORAGE_QUERY_DURATION.set(duration_ms);
	}

	pub fn observe_block_state_duration(duration_ms: f64) {
		BLOCK_STATE_DURATION.set(duration_ms);
	}

	pub fn observe_block_details_duration(duration_ms: f64) {
		BLOCK_DETAILS_DURATION.set(duration_ms);
	}
	pub fn observe_check_existing_submission_duration(duration_ms: f64) {
		CHECK_EXISTING_SUBMISSION_DURATION.set(duration_ms);
	}
	pub fn observe_bail_duration(duration_ms: f64) {
		BAIL_DURATION.set(duration_ms);
	}

	pub fn set_last_block_processing_time() {
		use std::time::{SystemTime, UNIX_EPOCH};
		let timestamp =
			SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as f64;
		LAST_BLOCK_PROCESSING_TIME.set(timestamp);
	}

	pub fn on_block_processing_stall() {
		BLOCK_PROCESSING_STALLS.inc();
	}
	pub fn on_mining_timeout() {
		MINING_TIMEOUTS.inc();
	}
	pub fn on_check_existing_submission_timeout() {
		CHECK_EXISTING_SUBMISSION_TIMEOUTS.inc();
	}
	pub fn on_bail_timeout() {
		BAIL_TIMEOUTS.inc();
	}
	pub fn on_submit_timeout() {
		SUBMIT_TIMEOUTS.inc();
	}
	pub fn observe_balance_fetch_duration(duration_ms: f64) {
		BALANCE_FETCH_DURATION.set(duration_ms);
	}
	pub fn on_balance_fetch_timeout() {
		BALANCE_FETCH_TIMEOUTS.inc();
	}
	pub fn observe_phase_check_duration(duration_ms: f64) {
		PHASE_CHECK_DURATION.set(duration_ms);
	}
	pub fn on_phase_check_timeout() {
		PHASE_CHECK_TIMEOUTS.inc();
	}
	pub fn observe_score_check_duration(duration_ms: f64) {
		SCORE_CHECK_DURATION.set(duration_ms);
	}
	pub fn on_score_check_timeout() {
		SCORE_CHECK_TIMEOUTS.inc();
	}
	pub fn observe_missing_pages_duration(duration_ms: f64) {
		MISSING_PAGES_DURATION.set(duration_ms);
	}
	pub fn on_missing_pages_timeout() {
		MISSING_PAGES_TIMEOUTS.inc();
	}

	pub fn on_era_pruning_timeout() {
		ERA_PRUNING_TIMEOUTS.inc();
	}
}
