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

This is a re-write of the
[staking miner](https://github.com/paritytech/polkadot/tree/master/utils/staking-miner) using
[subxt](https://github.com/paritytech/subxt) to avoid hard dependency to each runtime version.

A key difference is that the miner only interacts with AssetHub nodes that support multi-page async
staking and in particular the new multi-phase, multi-block election provider pallet (`EPMB`), while
the old legacy miner is designed to work with the previous multi-phase, single-page election
provider pallet.

The miner includes an automatic deposit recovery system that cleans up old discarded submissions to
reclaim locked deposits, ensuring optimal economic efficiency for operators.

The binary itself embeds [multi-block static metadata](./artifacts/multi_block.scale) to generate a
rust codegen at compile-time that [subxt provides](https://github.com/paritytech/subxt).

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

## Usage

You can check the help with:

```bash
$ polkadot-staking-miner --help
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

| Option                               | Description                                                                         | Default Value   |
| :----------------------------------- | :---------------------------------------------------------------------------------- | :-------------- |
| `--chunk-size <number>`              | Controls how many solution pages are submitted concurrently.                        | 0 (all at once) |
| `--do-reduce`                        | Enables solution reduction to make submissions more efficient.                      | Off             |
| `--min-signed-phase-blocks <number>` | Minimum number of blocks required in the signed phase before submitting a solution. | 10              |

Refer to `--help` for the full list of options.

NOTE: the miner only listens to finalized blocks. Listening to the best blocks is not offered as an
option.

### Automatic Deposit Recovery

The miner automatically handles deposit recovery for discarded submissions:

- **Automatic Cleanup**: Scans for old submissions from previous rounds that were not selected as
  winners
- **Deposit Recovery**: Calls `clear_old_round_data()` to reclaim locked deposits from discarded
  solutions
- **Triggered on Round Increments**: Activates when the election round number increments

This ensures that deposits from unsuccessful submissions are automatically recovered, maintaining
the economic viability of long-term mining operations.

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

## Update metadata

The static metadata files are stored at [artifacts/multi_block.scale](artifacts/multi_block.scale).

To update the metadata you need to connect to a polkadot, kusama or westend compatible node.

```bash
# Install subxt-cli
$ cargo install --locked subxt-cli
# Download the metadata from a local node running a compatible runtime and replace the current metadata.
# Specify --url ws://... if needed e.g. --url ws://localhost:9946)
$ subxt metadata  > artifacts/multi_block.scale
# Inspect the generated code.
# See `https://github.com/paritytech/subxt/tree/master/cli` for further documentation of the `subxt-cli` tool.
$ subxt codegen --file artifacts/metadata.scale | rustfmt > code.rs
```

## Test locally

Because elections occurs quite rarely and if you want to test it locally it's possible build the
polkadot binary with `feature --fast-runtime` to ensure that elections occurs more often (in order
of minutes rather hours/days).

```bash
# from the root of polkadot-sdk branch
$ cargo build --release -p polkadot -p polkadot-parachain-bin --features fast-runtime
# Be sure that the generated binaries are in your PATH variable!
$ cd substrate/frame/staking-async/runtimes/parachain
# NOTE: Customize the bash script to run a specific preset runtime (e.g. development, polkadot, kusama with different pages and number of validators and nominators).
# The scripts relies on [Zombienet](https://github.com/paritytech/zombienet) to spawn RC nodes and a parachain collator supporting the new staking-async machinery. The miner will run against the parachain collator. See the script for more details.
$ ./build-and-run-zn.sh
```

## Prometheus metrics

The staking-miner starts a prometheus server on port 9999 and that metrics can be fetched by:

```bash
$ curl localhost:9999/metrics
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

# Janitor (Deposit Recovery) Metrics
# HELP staking_miner_janitor_cleanup_success_total Total number of successful janitor cleanup operations
# TYPE staking_miner_janitor_cleanup_success_total counter
staking_miner_janitor_cleanup_success_total 3
# HELP staking_miner_janitor_cleanup_failures_total Total number of failed janitor cleanup operations
# TYPE staking_miner_janitor_cleanup_failures_total counter
staking_miner_janitor_cleanup_failures_total 0
# HELP staking_miner_janitor_cleanup_duration_ms The time in milliseconds it takes to complete janitor cleanup
# TYPE staking_miner_janitor_cleanup_duration_ms gauge
staking_miner_janitor_cleanup_duration_ms 1250
# HELP staking_miner_janitor_old_submissions_found Number of old submissions found during last janitor run
# TYPE staking_miner_janitor_old_submissions_found gauge
staking_miner_janitor_old_submissions_found 2
# HELP staking_miner_janitor_old_submissions_cleared Number of old submissions successfully cleared during last janitor run
# TYPE staking_miner_janitor_old_submissions_cleared gauge
staking_miner_janitor_old_submissions_cleared 2

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
# HELP staking_miner_tx_finalization_timeouts_total Total number of transaction finalization timeouts
# TYPE staking_miner_tx_finalization_timeouts_total counter
staking_miner_tx_finalization_timeouts_total 0
```

## Architecture

The polkadot-staking-miner uses a multi-task architecture to efficiently handle different
responsibilities:

### Task Overview

The miner consists of **three independent tasks** that communicate via bounded channels:

1. **Listener Task**: Monitors blockchain for phase changes and new (`finalized` only) blocks.
2. **Miner Task**: Handles solution mining and submission operations
3. **Janitor Task**: Manages automatic deposit recovery from old submissions

### Task Communication

```
                      (finalized blocks)
┌──────────────────────────────────────────────────────────────────────────┐
│   ┌─────────────┐                      ┌─────────────┐            ┌─────────────┐
└──▶│ Listener    │                      │   Miner     │            │ Blockchain  │
    │             │  Snapshot/Signed     │             │            │             │
    │ ┌─────────┐ │ ────────────────────▶│ ┌─────────┐ │ (solutions)│             │
    │ │ Stream  │ │  (mining work)       │ │ Mining  │ │───────────▶│             │
    │ └─────────┘ │                      │ └─────────┘ │            │             │
    │      │      │  Round++             │ ┌─────────┐ │            │             │
    │      ▼      │ ────────────────────▶│ │ Clear   │ │            │             │
    │ ┌─────────┐ │                      │ │ Snapshot│ │            │             │
    │ │ Phase   │ │                      │ └─────────┘ │            │             │
    │ │ Check   │ │  Round++             └─────────────┘            │             │
    │ └─────────┘ │ ────────────────────▶┌─────────────┐            │             │
    │             │  (deposit cleanup)   │  Janitor    │ (cleanup)  │             │
    │             │                      │ ┌─────────┐ │───────────▶│             │
    │             │                      │ │ Cleanup │ │            │             │
    │             │                      │ └─────────┘ │            │             │
    └─────────────┘                      └─────────────┘            └─────────────┘
```

### Key Design Principles

- **Separation of Concerns**: Each task has a single, well-defined responsibility
- **Non-Blocking Operations**: Tasks operate independently without blocking each other
- **Backpressure Handling**: Bounded channels prevent memory bloat and ensure fresh work
- **Fault Isolation**: Task failures are isolated and handled appropriately
- **Performance Optimization**: Mining operations are never delayed by cleanup tasks

### Benefits

- **Reliable Mining**: Core mining functionality is never blocked by ancillary operations
- **Automatic Maintenance**: Deposit recovery runs seamlessly in the background
- **Scalable Architecture**: Tasks can be optimized independently for their workloads
