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
        }
        (&Method::GET, "/") => Response::builder()
            .status(200)
            .body(Body::from(""))
            .unwrap(),
        _ => Response::builder()
            .status(404)
            .body(Body::from(""))
            .unwrap(),
    };

    Ok(response)
}

pub async fn run(port: u16) -> Result<(), String> {
    // Create the address to bind to
    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    // Bind the TCP listener
    let listener = TcpListener::bind(&addr)
        .await
        .map_err(|e| format!("Failed to bind socket on port {}: {:?}", port, e))?;

    log::info!(target: LOG_TARGET, "Started prometheus endpoint on http://{}", addr);

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
                    let conn = server
                        .serve_connection_with_upgrades(io, service)
                        .into_owned();

                    let conn = graceful.watch(conn);

                    tokio::spawn(async move {
                        if let Err(err) = conn.await {
                            log::debug!(target: LOG_TARGET, "connection error: {:?}", err);
                        }
                    });
                }
                Err(e) => {
                    log::debug!(target: LOG_TARGET, "Error accepting connection: {:?}", e);
                    continue;
                }
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
        register_gauge!(opts!(
            "staking_miner_balance",
            "The balance of the staking miner account"
        ))
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
}
