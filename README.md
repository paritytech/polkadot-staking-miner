# Polkadot staking miner

[![Daily compatibility check against latest polkadot](https://github.com/paritytech/polkadot-staking-miner/actions/workflows/nightly.yml/badge.svg)](https://github.com/paritytech/polkadot-staking-miner/actions/workflows/nightly.yml)

## Why do we need a staking miner?

Polkadot uses a **Nominated Proof-of-Stake (NPoS)** consensus mechanism where validators are
selected through a complex election process. This election determines the active validators in the
next era and how stake is distributed among them to maximize security and decentralization.

The nomination process in a nutshell:

```text
Nominators → Stake DOT → Nominate Validators → Validator Set Selection
```

where

- Nominators choose validators they trust with their stake
- _Phragmén_ algorithm tries to optimize three main objectives:
  - **Proportional Representation**: The algorithm ensures that the voting power is distributed proportionally among validators based on the stake backing them. This means that validators with more nominated stake should have a proportionally higher chance of being selected, maintaining fairness in representation.
  - **Security Maximization**: The algorithm aims to maximize the total stake backing the elected validator set. By selecting validators with the highest combined nominated stake, it enhances the economic security of the network, as it becomes more expensive for malicious actors to attack the system.
  - **Balanced Distribution**: The algorithm tries to minimize variance, distributing nominators' stakes as evenly as possible across the elected validators. This prevents scenarios where some validators have significantly more stake than others, which could create imbalances in the network's security and decentralization.

This multi-objective optimization is a NP-complete problem and cannot _yet_ be solved on-chain due to block time
and resource constraints. Instead, the blockchain runs a **multi-phase election**:

1. **Snapshot Phase**: Collect all staking data (validators, nominators, stakes)
2. **Signed Phase**: Off-chain actors (miners) compute optimal solutions and submit them
3. **Unsigned Phase**: Validators can submit solutions if no good ones were found
4. **Export Phase**: The best solution is chosen and applied

**Staking miners** are essential participants who:

- Compute high-quality election solutions off-chain
- Submit solutions during the signed phase to earn rewards
- Compete to provide the most optimal validator selection
- Ensure the election completes successfully even under high load

Without miners, elections could fail or produce suboptimal results, compromising network security
and decentralization.

More details about staking on Polkadot can be found [here](https://wiki.polkadot.network/learn/learn-staking/).

## Overview

### New miner vs legacy one

This is a re-write of the
[staking miner](https://github.com/paritytech/polkadot/tree/master/utils/staking-miner) using
[subxt](https://github.com/paritytech/subxt) to avoid hard dependency to each runtime version.

A key difference is that the miner only interacts with AssetHub nodes that support multi-page async
staking and in particular the new multi-phase, multi-block election provider pallet (`EPMB`), while
the old legacy miner is designed to work with the previous multi-phase, single-page election
provider pallet.

### Responsibilities

Whereas the main responsibility of the miner via the `monitor` command is to mine and submit a solution "at the right time" (i.e. during `Signed` phase), it is currently used also for some ancillary tasks to preserve the correct functionality of the staking system:

- an automatic deposit recovery system that cleans up old discarded submissions to reclaim locked deposits, ensuring optimal economic efficiency for operators.
- a mechanism to lazy prune old staking era data from the chain storage to keep the state size manageable.

This may be seen as an abuse of staking-miner responsibilities, and while it is a compromise, having a single 24/7 offchain worker for all staking tasks, including some janitorial ones 😅, is quite convenient. This setup allows us to avoid deploying and maintaining multiple bots, each assigned to a specific task (such as mining a solution, cleaning old rounds, or pruning old eras).

## Architecture

The polkadot-staking-miner uses a multi-task architecture to efficiently handle different
responsibilities:

### Task Overview

The miner consists of **four independent tasks** that communicate via bounded channels:

1. **Listener Task**: Monitors blockchain for phase changes and new (`finalized` only) blocks.
2. **Miner Task**: Handles solution mining and submission operations
3. **Clear Old Round Task**: Manages automatic deposit recovery from old submissions
4. **Era Pruning Task**: Handles lazy era pruning operations, only during Off phase

### Task Communication

```
                      (finalized blocks)
┌──────────────────────────────────────────────────────────────────────────┐
│   ┌─────────────┐                      ┌─────────────┐              ┌─────────────┐
└──▶│ Listener    │                      │   Miner     │              │ Blockchain  │
    │             │  Snapshot/Signed     │             │              │             │
    │ ┌─────────┐ │ ────────────────────▶│ ┌─────────┐ │ (solutions)  │             │
    │ │ Stream  │ │  (mining work)       │ │ Mining  │ │───────────▶  │             │
    │ └─────────┘ │                      │ └─────────┘ │              │             │
    │      │      │  Round++             │ ┌─────────┐ │              │             │
    │      ▼      │ ────────────────────▶│ │ Clear   │ │              │             │
    │ ┌─────────┐ │                      │ │ Snapshot│ │              │             │
    │ │ Phase   │ │                      │ └─────────┘ │              │             │
    │ │ Check   │ │  Round++             └─────────────┘              │             │
    │ └─────────┘ │ ────────────────────▶┌───────────────┐            │             │
    │     │       │  (deposit cleanup)   │ClearOldRounds │ (cleanup)  │             │
    │     │       │                      │ ┌─────────┐   │───────────▶│             │
    │     │       │                      │ │ Cleanup │   │            │             │
    │     │       │                      │ └─────────┘   │            │             │
    │     │       │                      └───────────────┘            │             │
    │     │       │                                                   │             │
    │     │       │Entering/leaving Off ┌───────────────┐(prune_era_step)           │
    │     └───────────────────────────▶ │ Era pruning   │───────────▶ │             │
    │             │   New block in Off  └───────────────┘             │             │
    │             │                                                   │             │
    └─────────────┘                                                   └─────────────┘
```

### Key Design Principles

- **Separation of Concerns**: Each task has a single, well-defined responsibility
- **Non-Blocking Operations**: Tasks operate independently without blocking each other
- **Backpressure Handling**: Bounded channels prevent memory bloat and ensure fresh work
- **Fault Isolation**: Task failures are isolated and handled appropriately
- **Performance Optimization**: Mining operations are never delayed by cleanup tasks

### RPC Endpoint Pool

The miner supports multiple RPC endpoints for high availability and automatic failover:

```bash
polkadot-staking-miner --uri "wss://rpc1.example.com,wss://rpc2.example.com" monitor --seed-or-path //Alice
```

**Key Features:**

- **Round-Robin Failover**: When a connection fails, the miner automatically tries the next endpoint
  in the pool, cycling through all available endpoints
- **Fast Failover for Pools**: When multiple endpoints are configured, connection retries are
  reduced to 1 per endpoint (vs 3 for single endpoint) to fail fast and try alternatives
- **Zombie Node Detection**: The miner task monitors for consecutive RPC failures (e.g., transaction
  rejections, timeouts) and triggers failover even when block streaming appears healthy
- **Unified Client Wrapper**: All tasks (listener, miner, runtime upgrade, etc.) share the same
  connection pool and benefit from automatic failover

**Prometheus Metrics for Monitoring:**

- `staking_miner_endpoint_switches_total`: Total number of times the client switched endpoints
- `staking_miner_miner_triggered_failovers_total`: Failovers triggered by consecutive miner failures
- `staking_miner_listener_subscription_stalls_total`: Listener-detected stalls causing failover
- `staking_miner_connection_timeouts_total`: Initial connection attempt timeouts

## Usage

You can check the help with:

```bash
polkadot-staking-miner --help
```

### Info command

Check remote node's metadata.

```bash
$ cargo run --release -- --uri wss://westend-asset-hub-rpc.polkadot.io info
Remote_node:
{
  "spec_name": "westmint",
  "impl_name": "westmint",
  "spec_version": 1018011,
  "impl_version": 0,
  "authoring_version": 1,
  "transaction_version": 16,
  "state_version": 1
}
```

### Monitor

To mine a solution and earn rewards, use a command like the following:

```bash
polkadot-staking-miner --uri ws://127.0.0.1:9946 monitor --seed-or-path //Alice
```

Here are some notable options you can use with the command:

| Option                               | Description                                                                                                                      | Default Value   |
| :----------------------------------- | :------------------------------------------------------------------------------------------------------------------------------- | :-------------- |
| `--chunk-size <number>`              | Controls how many solution pages are submitted concurrently.                                                                     | 0 (all at once) |
| `--do-reduce`                        | Enables solution reduction to make submissions more efficient.                                                                   | Off             |
| `--min-signed-phase-blocks <number>` | Minimum number of blocks required in the signed phase before submitting a solution.                                              | 10              |
| `--balancing-iterations <number>`    | Number of balancing iterations for the sequential phragmen algorithm. Higher values may produce better balanced solutions at the | 10              |
|                                      | cost of more computation time.                                                                                                   |                 |
| `--algorithm <algorithm>`            | Election algorithm to use for mining solutions. Supported: `seq-phragmen`, `phragmms`.                                           | `seq-phragmen`  |

Refer to `--help` for the full list of options.

NOTE: the miner only listens to finalized blocks. Listening to the best blocks is not offered as an
option.

### Prepare your SEED

While you could pass your seed directly to the cli or Docker, this is highly **NOT** recommended.
Instead, you should use an ENV variable.

You can set it manually using:

```bash
# The following line starts with an extra space on purpose, make sure to include it:
 SEED=0x1234...
```

Alternatively, for development, you may store your seed in a `.env` file that can be loaded to make
your seed available:

`.env`:

```bash
SEED=0x1234...
RUST_LOG=polkadot-staking-miner=debug
```

You can load it using `source .env`.

### Docker

A Docker container, especially one holding one of your `SEED` should be kept as secure as possible.
While it won't prevent a malicious actor to read your `SEED` if they gain access to your container,
it is nonetheless recommended running this container in `read-only` mode:

```bash
docker run --rm -it \
    --name polkadot-staking-miner \
    --read-only \
    -e RUST_LOG=info \
    -e SEED \
    -e URI=wss://your-node:9946 \
    polkadot-staking-miner monitor
```

### Important Warning

**Do not run multiple instances of the miner with the same account.**

The miner uses the on-chain nonce for a given user to submit solutions, which can lead to nonce
collisions if multiple miners are running with the same account. This can cause transaction failures
and potentially result in lost rewards or other issues.

## Predict

The `predict` command allows you to predict validator election outcomes for Substrate-based chains
without running a full node. It fetches the necessary staking data from the chain and runs the same
Phragmén algorithm that the chain uses to determine validator sets.

### Basic Usage

```bash
cargo run -- --uri wss://westend-asset-hub-rpc.polkadot.io predict --do-reduce
```

### Command Options

| Option                          | Description                                                                                                                   | Default Value      |
| :------------------------------ | :---------------------------------------------------------------------------------------------------------------------------- | :----------------- |
| `--desired-validators <number>` | Desired number of validators for the prediction                                                                               | Fetched from chain |
| `--overrides <path>`            | Path to election overrides JSON file (see format below)                                                                       | None               |
| `--output-dir <path>`           | Output directory for prediction results                                                                                       | `results`          |
| `--balancing-iterations <number>`| Number of balancing iterations for the sequential phragmen algorithm. Higher values may produce better balanced solutions at the cost of more computation time. | 10                 |
| `--do-reduce`            | Reduce the solution to prevent further trimming.                                                                              | `false`             |
| `--algorithm <algorithm>`       | Election algorithm to use. Supported: `seq-phragmen`, `phragmms`.                                                            | `seq-phragmen`     |
| `--block-number <number>`       | Block number at which to run the prediction. If not specified, uses the latest block.                                         | Latest block       |

### Examples

#### Basic Prediction

```bash
cargo run -- --uri wss://westend-asset-hub-rpc.polkadot.io predict --do-reduce
```

#### With Desired Validators

```bash
cargo run -- --uri wss://westend-asset-hub-rpc.polkadot.io predict --desired-validators 50 --do-reduce
```

#### Using Election Overrides File

```bash
cargo run -- --uri wss://westend-asset-hub-rpc.polkadot.io predict --overrides overrides.json
```

#### Prediction at a Specific Block

```bash
cargo run -- --uri wss://westend-asset-hub-rpc.polkadot.io predict --block-number 13196110 --do-reduce
```

#### Run Prediction with reduction

```bash
cargo run -- --uri wss://westend-asset-hub-rpc.polkadot.io predict --do-reduce
```

#### Run Prediction with PhragMMS algorithm

```bash
cargo run -- --uri wss://westend-asset-hub-rpc.polkadot.io predict --algorithm phragmms
```

### Output Files

The tool generates the following JSON files in the specified output directory:

1. **`validators_prediction.json`**: Contains elected validators with their stake information
2. **`nominators_prediction.json`**: Contains nominator allocations and validator support

#### Validators Prediction Format

```json
{
  "metadata": {
    "timestamp": "1765799538",
    "desired_validators": 600,
    "round": 40,
    "block_number": 10803423,
    "solution_score": {
      "minimal_stake": 11797523289886283,
      "sum_stake": 8372189060111758480,
      "sum_stake_squared": 117584540059969491964159919300216042
    },
    "data_source": "snapshot"
  },
  "results": [
    {
      "account": "15roBmbe5NmRXb4imfmhKxSjH8k9J5xtHSrvYJKpmmCLoPqD",
      "total_stake": "2372626.3933261476 DOT",
      "self_stake": "0 DOT",
      "nominator_count": 2,
      "nominators": [
        {
          "address": "121GCLDNk9ErAkCovjjuF3npDB3veo3i3myY6a5v2yNEgrZw",
          "allocated_stake": "769476 DOT"
        },
        {
          "address": "14mtWxmkUHsWqJLxMiRR8qrHTHyck712E5yjWpnxPBEh8Acb",
          "allocated_stake": "135680 DOT"
        },
      ]
    }
  ]
}
```

#### Nominators Prediction Format

```json
{
  "nominators": [
    {
      "address": "15VArSaLFf3r9MzyQjcNTexjPoRDJuVVkqUmqtuUuBcPCYrX",
      "stake": "447.2323363908 DOT",
      "active_validators": [
        {
          "validator": "15ZvLonEseaWZNy8LDkXXj3Y8bmAjxCjwvpy4pXWSL4nGSBs",
          "allocated_stake": "447.2323363908 DOT"
        }
      ],
      "inactive_validators": [
        "1627VVB5gtHiseCV8ZdffF7P3bWrLMkU92Q6u3LsG8tGuB63"
      ],
      "waiting_validators": [
        "13K6QTYBPMUFTbhZzqToKcfCiWbt4wDPHr3rUPyUessiPR61",
        "15rb4HVycC1KLHsdaSdV1x2TJAmUkD7PhubmhL3PnGv7RiGY"
      ]
    }
  ]
}
```

### Election Overrides File Format

When using `--overrides`, the file should have the following JSON structure:

```json
{
  "candidates_include": ["15S7YtETM31QxYYqubAwRJKRSM4v4Ua6WGFYnx1VuFBnWqdG"],
  "candidates_exclude": [],
  "voters_include": [
    ["15S7YtETM31QxYYqubAwRJKRSM4v4Ua6WGFYnx1VuFBnWqdG", 1000000, ["15S7YtETM31QxYYqubAwRJKRSM4v4Ua6WGFYnx1VuFBnWqdG"]]
  ],
  "voters_exclude": []
}
```

**Note:** Override file paths can be nested (e.g., `data/elections/overrides.json`). The tool will
automatically resolve relative paths from the current working directory.

### How Predict Command Works

1. **Data Source**: The tool first tries to fetch data from the chain's snapshot (if available),
   then falls back to the staking pallet. 

2. **Overrides Application**: If `--overrides` is provided, the tool applies the specified modifications to the fetched candidates and voters:
   - (1) Add candidates that may not exist on-chain.
   - (2) Remove specific candidates from the election.
   - (3) Add or override voters with custom stake amounts.
   - (4) Remove specific voters from the election.

3. **Election Algorithm**: Runs the same Phragmén algorithm (`seq_phragmen`) used by Substrate chains
   to determine:
   - Which validators would be elected
   - Stake distribution among validators
   - Nominator allocations to validators

4. **Output Generation**: Creates detailed JSON files with predictions, including validator and
   nominator perspectives.

## Server

The `server` command starts a REST API server that provides endpoints for running election simulations and fetching raw election data.

### Basic Usage

```bash
cargo run -- --uri wss://polkadot-asset-hub-rpc.polkadot.io server --port 8080
```

### Command Options

| Option   | Description                                 | Default Value |
| :------- | :------------------------------------------ | :------------ |
| `--port` | The port to listen on for REST API requests. | `8080`        |

---

### API Endpoints

#### 1. `POST /simulate`

Runs a full election simulation and returns the predicted validator set and nominator allocations.

**Query Parameters:**
- `block` (optional): Block number at which to run the simulation. Overrides `block_number` in JSON body.

**Request Body (JSON):**

```json
{
  "desired_validators": 600,
  "block_number": 11465562,
  "balancing_iterations": 10,
  "do_reduce": true,
  "algorithm": "SeqPhragmen",
  "overrides": {
    "candidates_include": ["15S7YtETM31QxYYqubAwRJKRSM4v4Ua6WGFYnx1VuFBnWqdG"],
    "candidates_exclude": [],
    "voters_include": [
      ["15S7YtETM31QxYYqubAwRJKRSM4v4Ua6WGFYnx1VuFBnWqdG", 1000000, ["15S7YtETM31QxYYqubAwRJKRSM4v4Ua6WGFYnx1VuFBnWqdG"]]
    ],
    "voters_exclude": []
  }
}
```

| Parameter | Type | Description | Default |
| :--- | :--- | :--- | :--- |
| `desired_validators` | `number` | Number of validators for the prediction | Fetched from chain |
| `balancing_iterations` | `number` | Number of balancing iterations | `10` |
| `do_reduce` | `boolean` | Reduce the solution to prevent further trimming | `false` |
| `algorithm` | `string` | Election algorithm: `SeqPhragmen` or `Phragmms` | `SeqPhragmen` |
| `overrides` | `string` | Overrides object | `null` |
| `block_number` | `number` | Block number at which to run the prediction | Latest block |

**Example Request:**
```bash
curl -X POST "http://localhost:8080/simulate?block=11465562" \
     -H "Content-Type: application/json" \
     -d '{"do_reduce": true}'
```

**Response Format:**
```json
{
  "result": {
    "run_parameters": {
      "block_number": 11465562,
      "desired_validators": 600,
      "balancing_iterations": 10,
      "do_reduce": true,
      "algorithm": "SeqPhragmen"
    },
    "validators": { ... },  // similar to predict's validators_output.json
    "nominators": { ... }   // similar to predict's nominators_output.json
  }
}
```

#### 2. `GET /snapshot`

Fetches the raw election snapshot data (validators and nominators) directly from the chain (or synthesizes it from staking storage).

**Query Parameters:**
- `block` (optional): Block number to fetch the snapshot from.

**Example Request:**
```bash
curl "http://localhost:8080/snapshot?block=12928418"
```

**Response Format:**
```json
{
  "result": {
    "validators": ["5Grwva...", "5FHneW..."], // target snapshot
    "nominators": [                           // voter snapshot
      {
        "account": "5FHneW...",
        "stake": "1000000000000",
        "targets": ["5Grwva..."]
      }
    ],
    "config": {
      "block_number": 12928418,
      "round": 469,
      "data_source": "snapshot"
    }
  }
}
```

---

## Update metadata

The binary itself embeds [multi-block static metadata](./artifacts/multi_block.scale) to generate a
rust codegen at compile-time that [subxt provides](https://github.com/paritytech/subxt).

To update the metadata you need to connect to a polkadot, kusama or westend compatible node.

```bash
# Install subxt-cli
$ cargo install --locked subxt-cli
# Download the metadata from a local node running a compatible runtime and replace the current metadata.
# Specify --url ws://... if needed e.g. --url ws://localhost:9946)
$ subxt metadata  > artifacts/multi_block.scale
# Inspect the generated code.
# See `https://github.com/paritytech/subxt/tree/master/cli` for further documentation of the `subxt-cli` tool.
$ subxt codegen --file artifacts/multi_block.scale | rustfmt > code.rs
```

## Runtime upgrades

Runtime upgrades are handled by the polkadot-staking-miner by upgrading storage constants and that
will work unless there is a breaking change in the pallets `pallet-election-provider-multi-block` or
`frame_system`.

Because detecting breaking changes when connecting to a RPC node when using `polkadot-staking-miner`
is hard, this repo performs daily integration tests against `polkadot master` and in most cases
[updating the metadata](#update-metadata) and fixing the compile errors are sufficient.

It's also possible to use the `info` command to check whether the metadata embedded in the binary is
compatible with a remote node, [see the info the command for further information](#info-command)

Each release will specify which runtime version it was tested against but it's not possible to know
in advance which runtimes it will work with.

Thus, it's important to subscribe to releases to this repo or add some logic that triggers an alert
once the polkadot-staking-miner crashes.

## Test locally

Because elections occurs quite rarely and if you want to test it locally it's possible build the
polkadot binary with `feature --fast-runtime` to ensure that elections occurs more often (in order
of minutes rather hours/days).

```bash
# from the root of polkadot-sdk branch
$ cargo build --release -p polkadot -p polkadot-parachain-bin --features fast-runtime
# Be sure that the generated binaries are in your PATH variable!
$ cd substrate/frame/staking-async/runtimes/papi-tests
# setup the environment just the 1st time
$ just setup
# NOTE: choose your favorite preset runtime (e.g. development, polkadot, kusama with different pages and number of validators and nominators).
# It relies on [Zombienet](https://github.com/paritytech/zombienet) to spawn RC nodes and a parachain collator supporting the new staking-async machinery. The miner will run against the parachain collator. See substrate/frame/staking-async/runtimes/papi-tests for more details
$ just run fake-dev
```

## Prometheus metrics

The staking-miner starts a prometheus server on port 9999 and that metrics can be fetched by:

```bash
curl localhost:9999/metrics
```

```bash
# Core Mining Metrics
# HELP staking_miner_balance The balance of the staking miner account
# TYPE staking_miner_balance gauge
staking_miner_balance 88756574897390270
# HELP staking_miner_mining_duration_ms The mined solution time in milliseconds.
# TYPE staking_miner_mining_duration_ms gauge
staking_miner_mining_duration_ms 50
# HELP staking_miner_submit_and_watch_duration_ms The time in milliseconds it took to submit the solution to chain and to be included in block
# TYPE staking_miner_submit_and_watch_duration_ms gauge
staking_miner_submit_and_watch_duration_ms 17283

# Submission Tracking
# HELP staking_miner_submissions_started Number of submissions started
# TYPE staking_miner_submissions_started counter
staking_miner_submissions_started 12
# HELP staking_miner_submissions_success Number of submissions finished successfully
# TYPE staking_miner_submissions_success counter
staking_miner_submissions_success 10

# Solution Quality Metrics
# HELP staking_miner_score_minimal_stake The minimal winner, in terms of total backing stake
# TYPE staking_miner_score_minimal_stake gauge
staking_miner_score_minimal_stake 24426059484170936
# HELP staking_miner_score_sum_stake The sum of the total backing of all winners
# TYPE staking_miner_score_sum_stake gauge
staking_miner_score_sum_stake 2891461667266507300
# HELP staking_miner_score_sum_stake_squared The sum squared of the total backing of all winners, aka. the variance.
# TYPE staking_miner_score_sum_stake_squared gauge
staking_miner_score_sum_stake_squared 83801161022319280000000000000000000

# Runtime Upgrades
# HELP staking_miner_runtime_upgrades Number of runtime upgrades performed
# TYPE staking_miner_runtime_upgrades counter
staking_miner_runtime_upgrades 2

# Clear Old Rounds (Deposit Recovery) Metrics
# HELP staking_miner_clear_old_rounds_cleanup_success_total Total number of successful clear old rounds cleanup operations
# TYPE staking_miner_clear_old_rounds_cleanup_success_total counter
staking_miner_clear_old_rounds_cleanup_success_total 3
# HELP staking_miner_clear_old_rounds_cleanup_failures_total Total number of failed clear old rounds cleanup operations
# TYPE staking_miner_clear_old_rounds_cleanup_failures_total counter
staking_miner_clear_old_rounds_cleanup_failures_total 0
# HELP staking_miner_clear_old_rounds_cleanup_duration_ms The time in milliseconds it takes to complete clear old rounds cleanup
# TYPE staking_miner_clear_old_rounds_cleanup_duration_ms gauge
staking_miner_clear_old_rounds_cleanup_duration_ms 1250
# HELP staking_miner_clear_old_rounds_old_submissions_found Number of old submissions found during last clear old rounds run
# TYPE staking_miner_clear_old_rounds_old_submissions_found gauge
staking_miner_clear_old_rounds_old_submissions_found 2
# HELP staking_miner_clear_old_rounds_old_submissions_cleared Number of old submissions successfully cleared during last clear old rounds run
# TYPE staking_miner_clear_old_rounds_old_submissions_cleared gauge
staking_miner_clear_old_rounds_old_submissions_cleared 2

# Era Pruning Metrics
# HELP staking_miner_era_pruning_submissions_success_total Total number of successful prune_era_step submissions
# TYPE staking_miner_era_pruning_submissions_success_total counter
staking_miner_era_pruning_submissions_success_total 15
# HELP staking_miner_era_pruning_submissions_failures_total Total number of failed prune_era_step submissions
# TYPE staking_miner_era_pruning_submissions_failures_total counter
staking_miner_era_pruning_submissions_failures_total 1
# HELP staking_miner_era_pruning_storage_map_size Current size of the era pruning storage map
# TYPE staking_miner_era_pruning_storage_map_size gauge
staking_miner_era_pruning_storage_map_size 3
# HELP staking_miner_era_pruning_timeouts_total Total number of era pruning storage query timeouts
# TYPE staking_miner_era_pruning_timeouts_total counter
staking_miner_era_pruning_timeouts_total 0

# Subscription Health Metrics
# HELP staking_miner_listener_subscription_stalls_total Total number of times the listener subscription was detected as stalled and recreated
# TYPE staking_miner_listener_subscription_stalls_total counter
staking_miner_listener_subscription_stalls_total 1
# HELP staking_miner_updater_subscription_stalls_total Total number of times the updater subscription was detected as stalled and recreated
# TYPE staking_miner_updater_subscription_stalls_total counter
staking_miner_updater_subscription_stalls_total 0
# HELP staking_miner_block_processing_stalls_total Total number of times block processing was detected as stalled
# TYPE staking_miner_block_processing_stalls_total counter
staking_miner_block_processing_stalls_total 2

# RPC Endpoint Pool Metrics
# HELP staking_miner_endpoint_switches_total Total number of times the client switched from one RPC endpoint to another
# TYPE staking_miner_endpoint_switches_total counter
staking_miner_endpoint_switches_total 2
# HELP staking_miner_miner_triggered_failovers_total Total number of times the miner task triggered a failover due to consecutive RPC failures
# TYPE staking_miner_miner_triggered_failovers_total counter
staking_miner_miner_triggered_failovers_total 1
# HELP staking_miner_connection_timeouts_total Total number of initial connection attempt timeouts
# TYPE staking_miner_connection_timeouts_total counter
staking_miner_connection_timeouts_total 0
# HELP staking_miner_mining_timeouts_total Total number of solution mining timeouts
# TYPE staking_miner_mining_timeouts_total counter
staking_miner_mining_timeouts_total 0
# HELP staking_miner_check_existing_submission_timeouts_total Total number of check existing submission timeouts
# TYPE staking_miner_check_existing_submission_timeouts_total counter
staking_miner_check_existing_submission_timeouts_total 0
# HELP staking_miner_bail_timeouts_total Total number of bail operation timeouts
# TYPE staking_miner_bail_timeouts_total counter
staking_miner_bail_timeouts_total 0
# HELP staking_miner_submit_timeouts_total Total number of solution submission timeouts
# TYPE staking_miner_submit_timeouts_total counter
staking_miner_submit_timeouts_total 0
# HELP staking_miner_phase_check_timeouts_total Total number of phase check timeouts
# TYPE staking_miner_phase_check_timeouts_total counter
staking_miner_phase_check_timeouts_total 0
# HELP staking_miner_score_check_timeouts_total Total number of score check timeouts
# TYPE staking_miner_score_check_timeouts_total counter
staking_miner_score_check_timeouts_total 0
# HELP staking_miner_missing_pages_timeouts_total Total number of missing pages submission timeouts
# TYPE staking_miner_missing_pages_timeouts_total counter
staking_miner_missing_pages_timeouts_total 0

# Performance Duration Metrics (for successful operations)
# HELP staking_miner_check_existing_submission_duration_ms Duration of checking existing submissions in milliseconds
# TYPE staking_miner_check_existing_submission_duration_ms gauge
staking_miner_check_existing_submission_duration_ms 125
# HELP staking_miner_bail_duration_ms Duration of bail operations in milliseconds
# TYPE staking_miner_bail_duration_ms gauge
staking_miner_bail_duration_ms 2340
# HELP staking_miner_phase_check_duration_ms Duration of phase check operations in milliseconds
# TYPE staking_miner_phase_check_duration_ms gauge
staking_miner_phase_check_duration_ms 85
# HELP staking_miner_score_check_duration_ms Duration of score check operations in milliseconds
# TYPE staking_miner_score_check_duration_ms gauge
staking_miner_score_check_duration_ms 120
# HELP staking_miner_missing_pages_duration_ms Duration of missing pages submission in milliseconds
# TYPE staking_miner_missing_pages_duration_ms gauge
staking_miner_missing_pages_duration_ms 8500
```
