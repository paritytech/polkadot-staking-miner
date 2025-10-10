use sp_arithmetic::Perbill;
use sp_npos_elections::{ElectionScore, seq_phragmen};
use std::{collections::{HashMap, HashSet}};

use crate::types::{
    AccountId, Balance, EraIndex, NominatorData, 
    PredictedElectionResult, DetailedPredictionResult, PredictionError,
    ValidatorSupport, StakeDistribution, ValidatorStake
};

/// Main election prediction engine
pub struct ElectionPredictor {
    /// Current active era
    pub active_era: EraIndex,
    /// Desired number of validators
    pub desired_members: u32,
    /// Desired number of runners-up
    pub desired_runners_up: u32,
    /// Voters - nominators
    pub voters: Vec<NominatorData>, // (voter, stake, targets)
    /// Candidate_stashes - all the validator candidates
    pub candidate_stashes: Vec<AccountId>,
}

impl ElectionPredictor {
    /// Create a new election predictor
    pub fn new(
        active_era: EraIndex,
        desired_members: u32,
        desired_runners_up: u32,
        voters: Vec<NominatorData>,
        candidate_stashes: Vec<AccountId>,
    ) -> Self {
        Self {
            active_era,
            desired_members,
            desired_runners_up,
            voters,
            candidate_stashes,
        }
    }

    /// Predict election results using the Phragmén algorithm
    /// This replicates the exact same algorithm used by Substrate's election provider
    pub fn predict_election(&self) -> Result<PredictedElectionResult, PredictionError> {
        if self.candidate_stashes.is_empty() {
            return Err(PredictionError::NoCandidates);
        }

        if self.voters.is_empty() {
            return Err(PredictionError::NoVoters);
        }

        // Convert to the format expected by sp-npos-elections
        let voter_count = self.voters.len();
        let candidate_count = self.candidate_stashes.len();

        let result = seq_phragmen::<AccountId, Perbill>(
            self.desired_members as usize + self.desired_runners_up as usize, // Request total winners including runners-up
            self.candidate_stashes.clone(),
            self.voters.clone(),
            None, // No balancing config
        )
        .map_err(|_| PredictionError::AlgorithmError)?;

        // Convert results back to our format
        let mut members = Vec::new();
        let mut runners_up = Vec::new();

        // Process winners (members)
        for (winner, stake) in result.winners.iter().take(self.desired_members as usize) {
            members.push((winner.clone(), *stake));
        }

        // Process runners-up
        for (winner, stake) in result
            .winners
            .iter()
            .skip(self.desired_members as usize)
            .take(self.desired_runners_up as usize)
        {
            runners_up.push((winner.clone(), *stake));
        }

        // Create a score for ElectionResult
        let result_score = ElectionScore {
            minimal_stake: 0,
            sum_stake: 0,
            sum_stake_squared: 0,
        };

        Ok(PredictedElectionResult {
            members,
            runners_up,
            score: result_score,
            total_voters: voter_count as u32,
            total_candidates: candidate_count as u32,
            active_era: self.active_era,
        })
    }

    /// Predict election with detailed analysis
    pub fn predict_with_analysis(&self) -> Result<DetailedPredictionResult, PredictionError> {
        let prediction = self.predict_election()?;

        // Calculate additional metrics
        let total_stake: Balance = self.voters.iter().map(|(_, stake, _)| *stake as Balance).sum();
        let total_voters = self.voters.len();
        let total_candidates = self.candidate_stashes.len();

        // Calculate stake distribution
        let mut stake_distribution: Vec<StakeDistribution> = Vec::new();
        for (voter, stake, _) in &self.voters {
            stake_distribution.push((
                voter.clone(),
                *stake,
                *stake as f64 / total_stake as f64 * 100.0,
            ));
        }
        stake_distribution.sort_by(|a, b| b.1.cmp(&a.1));

        // Calculate validator support
        let mut validator_support: ValidatorSupport = HashMap::new();
        for (_voter, stake, targets) in &self.voters {
            let stake_per_target = if targets.is_empty() {
                0
            } else {
                *stake as Balance / targets.len() as Balance
            };
            for target in targets {
                *validator_support
                    .entry(target.to_string().clone())
                    .or_insert(0) += stake_per_target;
            }
        }

        Ok(DetailedPredictionResult {
            prediction: prediction,
            total_stake,
            total_voters,
            total_candidates,
            stake_distribution,
            validator_support,
        }) 
    }
}
