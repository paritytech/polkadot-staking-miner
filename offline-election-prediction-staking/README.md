# Offline Election Prediction Tool

A powerful Rust-based tool for accurately predicting the results of on-chain elections for Substrate-based blockchains using the same Phragmén algorithm used by the chain itself.

## Overview

This tool enables you to predict validator election outcomes for Substrate-based chains (like Polkadot, Kusama, and their parachains) without running a full node. It fetches the necessary staking data from the chain and runs the same election algorithm that the chain uses to determine validator sets.

## Features

- **Accurate Predictions**: Uses the same Phragmén algorithm as Substrate chains
- **Multiple Chain Support**: Works with Polkadot, Kusama, and other Substrate-based chains
- **Flexible Data Sources**: Can fetch data from chain or use cached data
- **Detailed Analysis**: Provides comprehensive statistics and validator information
- **JSON Output**: Structured results for further analysis
- **Command-Line Interface**: Easy-to-use CLI with multiple configuration options

## Installation

### Prerequisites

- Rust 1.70+ (with 2024 edition support)
- Internet connection for fetching chain data (optional if using cached data)

### Build from Source

````bash
git clone <repository-url>
cd polkadot-staking-miner
cargo build --release


## Usage

### Basic Commands

```bash
# Show help
 ./target/release/polkadot-staking-miner predict --help

# Show predict command help
 ./target/release/polkadot-staking-miner predict --help
````

### Command Options

- `--chain-uri <CHAIN_URI>`: Chain WebSocket endpoint URI (default: wss://westend-asset-hub-rpc.polkadot.io)
- `--desired-validators <DESIRED_VALIDATORS>`: Desired number of validators for the prediction
- `--custom-nominators-validators <CUSTOM_NOMINATORS_VALIDATORS>`: Path to custom nominators and validators JSON file
- `--output <OUTPUT>`: Output file for prediction results (default: election_prediction.json)
- `--use-cached-data`: Use cached data instead of fetching from chain
- `--cache-dir <CACHE_DIR>`: Path to cached data directory (default: ./cache)

## Examples

### Polkadot Mainnet

```bash
 ./target/release/polkadot-staking-miner --uri wss://rpc.polkadot.io predict --desired-validators 19
```

### Assset-hub

```bash
 ./target/release/polkadot-staking-miner --uri wss://westend-asset-hub-rpc.polkadot.io predict --desired-validators 19
```

### Kusama

```bash
 ./target/release/polkadot-staking-miner --uri wss://kusama.api.onfinality.io/public-ws predict --desired-validators 19
```

### Using Cached Data

```bash
./target/release/polkadot-staking-miner --uri wss://westend-asset-hub-rpc.polkadot.io predict --use-cached-data --desired-validators 19
```

### Custom Output

```bash
 ./target/release/polkadot-staking-miner predict --output results/polkadot_prediction.json
```

### Typical Usage

```bash
 ./target/release/polkadot-staking-miner --uri wss://westend-asset-hub-rpc.polkadot.io predict --desired-validators 19 --output my_prediction.json
```

## Output Format

The tool generates a JSON file with the following structure:

```json
{
  "metadata": {
    "timestamp": "2024-01-01T00:00:00Z",
    "desired_validators": 19
  },
  "results": {
    "active_validators": [
      {
        "account": "5ENXqYmc5m6VLMm5i1mun832xAv2Qm9t3M4PWAFvvyCJLNoR",
        "total_stake": 165143380563409994,
        "self_stake": 74060125441473510
      }
    ],
    "statistics": {
      "minimum_stake": 61838713796955041,
      "average_stake": 119978106139616045,
      "total_staked": 2279584036652704860
    }
  }
}
```

## How It Works

1. **Data Fetching**: Connects to the specified chain endpoint and fetches:

   - Current active validators
   - Validator candidates and their stakes
   - Nominator information and their targets
   - Chain parameters (desired validators, runners-up)

2. **Election Simulation**: Runs the Phragmén algorithm to determine:

   - Which validators would be elected
   - Stake distribution among validators
   - Runners-up for backup validators

3. **Analysis**: Provides detailed statistics including:
   - Total stake and voter information
   - Validator support metrics
   - Stake distribution analysis

## Supported Chains

This tool works with any Substrate-based chain that implements the Staking pallet, including:

- **Polkadot**: Main parachain network
- **Kusama**: Canary network
- **Westend**: Test network
- **Asset Hub**: Polkadot's asset parachain
- **Other Parachains**: Any Substrate-based parachain

## Development

### Project Structure

```
src/
├── main.rs          # CLI entry point
├── lib.rs           # Library exports
├── cli.rs           # CLI configuration and parsing
├── predict.rs       # Election prediction logic
├── data_fetcher.rs  # Chain data fetching
├── types.rs         # Type definitions
└── bin/
    └── hex2ss58.rs  # Utility for address conversion
```

### Running Tests

```bash
cargo test
```

### Building for Release

```bash
cargo build --release
```

## Contributing

1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Add tests if applicable
5. Submit a pull request

## License

This project is licensed under the MIT License - see the LICENSE file for details.

## Acknowledgments

- Built using the Substrate ecosystem tools
- Uses the same Phragmén algorithm as Substrate chains
- Inspired by the need for accurate election predictions in the Polkadot ecosystem

## Troubleshooting

### Common Issues

1. **Connection Errors**: Ensure the chain endpoint is accessible and correct
2. **Data Fetching Issues**: Try using `--use-cached-data` if chain access is problematic
3. **Build Errors**: Ensure you have Rust 1.70+ with 2024 edition support

### Getting Help

- Check the help commands: `cargo run --bin polkadot-staking-miner --help`
- Review the examples in this README
- Open an issue for bugs or feature requests

## Output

The tool will:

1. Fetch or load election data based on your configuration
2. Run the election prediction algorithm
3. Display results in the console
4. Save detailed results to the specified output file (JSON format)
