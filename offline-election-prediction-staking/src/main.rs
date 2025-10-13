use anyhow::Result;
use clap::Parser;

use offline_election::{
    DetailedPredictionResult, 
    ElectionPredictor, 
    Cli, Commands,
    save_prediction_results
};


/// Main function to fetch data and run election prediction
#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    env_logger::init();

    // Parse CLI arguments
    let cli = Cli::parse();
    
    match cli.command {
        Commands::Predict(config) => {
            run_prediction(config).await?;
        }
    }

    Ok(())
}

/// Run the election prediction with the given configuration
async fn run_prediction(config: offline_election::PredictCliConfig) -> Result<()> {
    println!("Starting election prediction tool...");
    // println!("Configuration: {:?}", config);

    // Create data fetcher
    let mut fetcher = config.create_data_fetcher().await?;

    // Fetch or load election data based on CLI options
    let election_data = config.load_election_data(&mut fetcher).await?;
    
    // Clone election_data to avoid borrow issues
    let election_data_clone = election_data.clone();
    
    let mut candidate_stashes = vec![];
    let mut voters = Vec::from(election_data.nominators);

    for (stash, stake) in election_data.candidates {
        voters.push((stash.clone(), stake as u64, vec![stash.clone()]));
        candidate_stashes.push(stash);
    }

    // Use CLI desired_validators if provided, otherwise use chain data
    let desired_validators = config.get_desired_validators(election_data.desired_validators);

    // Create election predictor
    let predictor = ElectionPredictor::new(
        election_data.active_era,
        desired_validators,
        election_data.desired_runners_up,
        voters,
        candidate_stashes,
    );

    println!("Running election prediction...");
    println!("Desired validators: {}", desired_validators);

    // Run prediction with detailed analysis
    match predictor.predict_with_analysis() {
        Ok(detailed_result) => {
            println!("Election prediction completed successfully!");
            print_prediction_results(&detailed_result);
            
            // Save results to output file
            save_prediction_results(&detailed_result, desired_validators, &election_data_clone, &config.output)?;
            println!("Results saved to: {}", config.output);
        }
        Err(e) => {
            eprintln!("Election prediction failed: {}", e);
            return Err(e.into());
        }
    }

    Ok(())
}

/// Print the prediction results in a formatted way
fn print_prediction_results(result: &DetailedPredictionResult) {
    println!("\n=== ELECTION PREDICTION RESULTS ===");
    println!("Active Era: {}", result.prediction.active_era);
    println!("Total Stake: {} units", result.total_stake);
    println!("Total Voters: {}", result.total_voters);
    println!("Total Candidates: {}", result.total_candidates);

    println!("\n--- ELECTED VALIDATORS ---");
    for (i, (validator, stake)) in result.prediction.members.iter().enumerate() {
        println!(
            "{}. {} (Stake: {} units)",
            i + 1,
            validator.to_string(),
            *stake
        );
    }

    if !result.prediction.runners_up.is_empty() {
        println!("\n--- RUNNERS-UP ---");
        for (i, (validator, stake)) in result.prediction.runners_up.iter().enumerate() {
            println!(
                "{}. {} (Stake: {} units)",
                i + 1,
                validator.to_string(),
                *stake
            );
        }
    }

    
    println!("\n=== END OF PREDICTION RESULTS ===");
}
