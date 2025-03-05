use crate::{opt::Solver, prelude::Hash};
use polkadot_sdk::sp_runtime::Perbill;

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

/// Submission strategy to use.
#[derive(Debug, Copy, Clone)]
#[cfg_attr(test, derive(PartialEq))]
pub enum SubmissionStrategy {
    /// Always submit.
    Always,
    // Submit if we are leading, or if the solution that's leading is more that the given `Perbill`
    // better than us. This helps detect obviously fake solutions and still combat them.
    /// Only submit if at the time, we are the best (or equal to it).
    IfLeading,
    /// Submit if we are no worse than `Perbill` worse than the best.
    ClaimNoWorseThan(Perbill),
    /// Submit if we are leading, or if the solution that's leading is more that the given `Perbill`
    /// better than us. This helps detect obviously fake solutions and still combat them.
    ClaimBetterThan(Perbill),
}

/// Custom `impl` to parse `SubmissionStrategy` from CLI.
///
/// Possible options:
/// * --submission-strategy if-leading: only submit if leading
/// * --submission-strategy always: always submit
/// * --submission-strategy "percent-better <percent>": submit if submission is `n` percent better.
///
impl std::str::FromStr for SubmissionStrategy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();

        let res = if s == "if-leading" {
            Self::IfLeading
        } else if s == "always" {
            Self::Always
        } else if let Some(percent) = s.strip_prefix("no-worse-than ") {
            let percent: u32 = percent.parse().map_err(|e| format!("{:?}", e))?;
            Self::ClaimNoWorseThan(Perbill::from_percent(percent))
        } else if let Some(percent) = s.strip_prefix("percent-better ") {
            let percent: u32 = percent.parse().map_err(|e| format!("{:?}", e))?;
            Self::ClaimBetterThan(Perbill::from_percent(percent))
        } else {
            return Err(s.into());
        };
        Ok(res)
    }
}
