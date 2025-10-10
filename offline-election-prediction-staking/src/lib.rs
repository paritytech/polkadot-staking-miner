//! Offline Election Prediction Tool
//!
//! This library provides tools to accurately predict the results of on-chain elections
//! for Substrate-based chains using the same Phragmén algorithm used by the chain itself.

pub mod types;
pub mod predict;
pub mod data_fetcher;
pub mod cli;

pub use types::*;
pub use predict::ElectionPredictor;
pub use data_fetcher::DataFetcher;
pub use cli::{Cli, Commands, PredictCliConfig, save_prediction_results};
