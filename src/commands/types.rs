use polkadot_sdk::sp_runtime::Perbill;

/// Submission strategy to use.
#[derive(Debug, Copy, Clone)]
#[cfg_attr(test, derive(PartialEq))]
pub enum SubmissionStrategy {
	/// Always submit.
	Always,
	// Submit if we are leading, or if the solution that's leading is more that the given
	// `Perbill` better than us. This helps detect obviously fake solutions and still combat
	// them.
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

/// TODO: make `solver algorithm` configurable https://github.com/paritytech/polkadot-staking-miner/issues/989
#[derive(Debug, Clone, clap::Parser)]
#[cfg_attr(test, derive(PartialEq))]
pub struct MultiBlockMonitorConfig {
	#[clap(long, short, env = "SEED")]
	pub seed_or_path: String,

	#[clap(long, value_parser, default_value = "if-leading")]
	pub submission_strategy: SubmissionStrategy,

	/// Reduce the solution to prevent further trimming.
	#[clap(long, default_value_t = false)]
	pub do_reduce: bool,

	/// Chunk size for submitting solution pages. If not specified or equal to zero,
	/// all pages will be submitted concurrently. Otherwise, pages will be submitted in chunks
	/// of the specified size, waiting for each chunk to be included in a block before
	/// submitting the next chunk.
	#[clap(long, default_value_t = 0)]
	pub chunk_size: usize,

	/// Minimum number of blocks required in the signed phase before submitting a solution.
	/// If the signed phase has fewer blocks remaining, the miner will skip mining to avoid
	/// incomplete submissions and will bail any existing incomplete submissions.
	#[clap(long, default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..))]
	pub min_signed_phase_blocks: u32,

	/// Simulate malicious behavior by submitting max score with no solution pages.
	/// This creates invalid submissions to test the system's response to spam attacks.
	#[clap(long, default_value_t = false, hide = true)]
	pub shady: bool,
}
