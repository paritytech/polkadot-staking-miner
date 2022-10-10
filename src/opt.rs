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

use crate::{error::Error, prelude::*};

use clap::*;
use sp_npos_elections::BalancingConfig;
use sp_runtime::Perbill;
use std::{fmt, str::FromStr};

#[derive(Debug, Clone, Parser)]
#[cfg_attr(test, derive(PartialEq))]
pub enum Command {
	/// Monitor for the phase being signed, then compute.
	Monitor(MonitorConfig),
	/// Just compute a solution now, and don't submit it.
	DryRun(DryRunConfig),
	/// Provide a solution that can be submitted to the chain as an emergency response.
	EmergencySolution(EmergencySolutionConfig),
}

#[derive(Debug, Clone, Parser)]
#[cfg_attr(test, derive(PartialEq))]
pub enum Solver {
	SeqPhragmen {
		#[clap(long, default_value = "10")]
		iterations: usize,
	},
	PhragMMS {
		#[clap(long, default_value = "10")]
		iterations: usize,
	},
}

#[macro_export]
macro_rules! any_runtime {
	($chain:tt, $($code:tt)*) => {
		match $chain {
			Chain::Polkadot => {
				#[allow(unused)]
				use $crate::static_types::polkadot::MinerConfig;
				$($code)*
			},
			Chain::Kusama => {
				#[allow(unused)]
				use $crate::static_types::kusama::MinerConfig;
				$($code)*
			},
			Chain::Westend => {
				#[allow(unused)]
				use $crate::static_types::westend::MinerConfig;
				$($code)*
			},
		}
	};
}

/// Submission strategy to use.
#[derive(Debug, Copy, Clone)]
#[cfg_attr(test, derive(PartialEq))]
pub enum SubmissionStrategy {
	// Only submit if at the time, we are the best.
	IfLeading,
	// Always submit.
	Always,
	// Submit if we are leading, or if the solution that's leading is more that the given `Perbill`
	// better than us. This helps detect obviously fake solutions and still combat them.
	ClaimBetterThan(Perbill),
}

/// Custom `impl` to parse `SubmissionStrategy` from CLI.
///
/// Possible options:
/// * --submission-strategy if-leading: only submit if leading
/// * --submission-strategy always: always submit
/// * --submission-strategy "percent-better <percent>": submit if submission is `n` percent better.
///
impl FromStr for SubmissionStrategy {
	type Err = String;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		let s = s.trim();

		let res = if s == "if-leading" {
			Self::IfLeading
		} else if s == "always" {
			Self::Always
		} else if let Some(percent) = s.strip_prefix("percent-better ") {
			let percent: u32 = percent.parse().map_err(|e| format!("{:?}", e))?;
			Self::ClaimBetterThan(Perbill::from_percent(percent))
		} else {
			return Err(s.into())
		};
		Ok(res)
	}
}

frame_support::parameter_types! {
	/// Number of balancing iterations for a solution algorithm. Set based on the [`Solvers`] CLI
	/// config.
	pub static BalanceIterations: usize = 10;
	pub static Balancing: Option<BalancingConfig> = Some( BalancingConfig { iterations: BalanceIterations::get(), tolerance: 0 } );
}

/// The chain being used.
#[derive(Debug, Copy, Clone)]
pub enum Chain {
	Westend,
	Kusama,
	Polkadot,
}

/// The type of event to listen to.
///
///
/// Typically, finalized is safer and there is no chance of anything going wrong, but it can be
/// slower. It is recommended to use finalized, if the duration of the signed phase is longer
/// than the the finality delay.
#[cfg_attr(test, derive(PartialEq))]
#[derive(clap::ValueEnum, Debug, Copy, Clone)]
pub enum Listen {
	/// Latest finalized head of the canonical chain.
	Finalized,
	/// Latest head of the canonical chain.
	Head,
}

impl fmt::Display for Chain {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let chain = match self {
			Self::Polkadot => "polkadot",
			Self::Kusama => "kusama",
			Self::Westend => "westend",
		};
		write!(f, "{}", chain)
	}
}

impl std::str::FromStr for Chain {
	type Err = Error;

	fn from_str(s: &str) -> Result<Self, Error> {
		match s {
			"polkadot" => Ok(Self::Polkadot),
			"kusama" => Ok(Self::Kusama),
			"westend" => Ok(Self::Westend),
			chain => Err(Error::InvalidChain(chain.to_string())),
		}
	}
}

impl TryFrom<subxt::rpc::RuntimeVersion> for Chain {
	type Error = Error;

	fn try_from(rv: subxt::rpc::RuntimeVersion) -> Result<Self, Error> {
		let json = rv
			.other
			.get("specName")
			.expect("RuntimeVersion must have specName; qed")
			.clone();
		let mut chain =
			serde_json::from_value::<String>(json).expect("specName must be String; qed");
		chain.make_ascii_lowercase();
		Chain::from_str(&chain)
	}
}

#[derive(Debug, Clone, Parser)]
#[cfg_attr(test, derive(PartialEq))]
pub struct MonitorConfig {
	/// They type of event to listen to.
	///
	/// Typically, finalized is safer and there is no chance of anything going wrong, but it can be
	/// slower. It is recommended to use finalized, if the duration of the signed phase is longer
	/// than the the finality delay.
	#[clap(long, value_enum, default_value_t = Listen::Head)]
	pub listen: Listen,

	/// The solver algorithm to use.
	#[clap(subcommand)]
	pub solver: Solver,

	/// Submission strategy to use.
	///
	/// Possible options:
	///
	/// `--submission-strategy if-leading`: only submit if leading.
	///
	/// `--submission-strategy always`: always submit.
	///
	/// `--submission-strategy "percent-better <percent>"`: submit if the submission is `n` percent better.
	#[clap(long, parse(try_from_str), default_value = "if-leading")]
	pub submission_strategy: SubmissionStrategy,

	/// The path to a file containing the seed of the account. If the file is not found, the seed is
	/// used as-is.
	///
	/// Can also be provided via the `SEED` environment variable.
	///
	/// WARNING: Don't use an account with a large stash for this. Based on how the bot is
	/// configured, it might re-try and lose funds through transaction fees/deposits.
	#[clap(long, short, env = "SEED")]
	pub seed_or_path: String,
}

#[derive(Debug, Clone, Parser)]
#[cfg_attr(test, derive(PartialEq))]
pub struct EmergencySolutionConfig {
	/// The block hash at which scraping happens. If none is provided, the latest head is used.
	#[clap(long)]
	pub at: Option<Hash>,

	/// The solver algorithm to use.
	#[clap(subcommand)]
	pub solver: Solver,

	/// The number of top backed winners to take. All are taken, if not provided.
	pub take: Option<usize>,
}

#[derive(Debug, Clone, Parser)]
#[cfg_attr(test, derive(PartialEq))]
pub struct DryRunConfig {
	/// The block hash at which scraping happens. If none is provided, the latest head is used.
	#[clap(long)]
	pub at: Option<Hash>,

	/// The solver algorithm to use.
	#[clap(subcommand)]
	pub solver: Solver,

	/// Force create a new snapshot, else expect one to exist onchain.
	#[clap(long)]
	pub force_snapshot: bool,

	/// The path to a file containing the seed of the account. If the file is not found, the seed is
	/// used as-is.
	///
	/// Can also be provided via the `SEED` environment variable.
	///
	/// WARNING: Don't use an account with a large stash for this. Based on how the bot is
	/// configured, it might re-try and lose funds through transaction fees/deposits.
	#[clap(long, short, env = "SEED")]
	pub seed_or_path: String,
}

#[derive(Debug, Clone, Parser)]
#[cfg_attr(test, derive(PartialEq))]
#[clap(author, version, about)]
pub struct Opt {
	/// The `ws` node to connect to.
	#[clap(long, short, default_value = DEFAULT_URI, env = "URI")]
	pub uri: String,

	#[clap(subcommand)]
	pub command: Command,

	/// The prometheus endpoint TCP port.
	#[clap(long, short, env = "PROMETHEUS_PORT")]
	pub prometheus_port: Option<u16>,
}
