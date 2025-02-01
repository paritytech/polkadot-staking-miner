use crate::{opt::Solver, prelude::Hash};

#[derive(Debug, Clone, clap::Parser)]
#[cfg_attr(test, derive(PartialEq))]
pub struct EmergencySolutionConfig {
	/// The block hash at which scraping happens. If none is provided, the latest head is used.
	#[clap(long)]
	pub at: Option<Hash>,

	/// The solver algorithm to use.
	#[clap(subcommand)]
	pub solver: Solver,

	/// The number of top backed winners to take instead. All are taken, if not provided.
	pub force_winner_count: Option<u32>,
}

#[derive(Debug, Clone, clap::Parser)]
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

	/// The number of winners to take, instead of the `desired_targets` in snapshot.
	// Doing this would cause the dry-run to typically fail, but that's fine, the program should
	// still print out some score, and that should be it.
	#[clap(long)]
	pub force_winner_count: Option<u32>,

	/// The path to a file containing the seed of the account. If the file is not found, the seed is
	/// used as-is. If this is not provided, we won't attempt to submit anything.
	///
	/// Can also be provided via the `SEED` environment variable.
	///
	/// WARNING: Don't use an account with a large stash for this. Based on how the bot is
	/// configured, it might re-try and lose funds through transaction fees/deposits.
	#[clap(long, short, env = "SEED")]
	pub seed_or_path: Option<String>,
}

/// The type of event to listen to.
///
///
/// Typically, finalized is safer and there is no chance of anything going wrong, but it can be
/// slower. It is recommended to use finalized, if the duration of the signed phase is longer
/// than the the finality delay.
#[derive(clap::ValueEnum, Debug, Copy, Clone, PartialEq)]
pub enum Listen {
	/// Latest finalized head of the canonical chain.
	Finalized,
	/// Latest head of the canonical chain.
	Head,
}
