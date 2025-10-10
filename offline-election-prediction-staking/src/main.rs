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


