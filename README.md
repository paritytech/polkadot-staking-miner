# Polkadot staking miner

[![Daily compatibility check against latest polkadot](https://github.com/paritytech/polkadot-staking-miner/actions/workflows/nightly.yml/badge.svg)](https://github.com/paritytech/polkadot-staking-miner/actions/workflows/nightly.yml)

This is a re-write of the [staking miner](https://github.com/paritytech/polkadot/tree/master/utils/staking-miner) using [subxt](https://github.com/paritytech/subxt) to avoid hard dependency to each runtime version.

The binary itself embeds [static metadata](./artifacts/metadata.scale) to
generate a rust codegen at compile-time that [subxt provides](https://github.com/paritytech/subxt).

Runtime upgrades are handled by the polkadot-staking-miner by upgrading storage constants
and that will work unless there is a breaking change in the pallets `pallet-election-provider-multi-phase`
or `frame_system`.

Because detecting breaking changes when connecting to a RPC node when using
`polkadot-staking-miner` is hard, this repo performs daily integration tests
against `polkadot master` and in most cases [updating the metadata](#update-metadata)
and fixing the compile errors are sufficient.

It's also possible to use the `info` command to check whether the metadata
embedded in the binary is compatible with a remote node, [see the info the command for further information](#info-command)

Each release will specify which runtime version it was tested against but
it's not possible to know in advance which runtimes it will work with.

Thus, it's important to subscribe to releases to this repo or
add some logic that triggers an alert once the polkadot-staking-miner crashes.

## Usage

You can check the help with:

```bash
$ polkadot-staking-miner --help
```

### Monitor

To "mine solutions" and earn rewards, use a command like the following.
Please try this on a dev-chain before using real funds because it's
possible to lose money.

```bash
$ cargo run --release -- --uri ws://localhost:9944 monitor --seed-or-path //Alice --dry-run seq-phragmen
```

This is a starting point that will try compute new solutions to the
validator set that also validates that transaction before submitting.

For further information regarding the different options run:

```bash
$ cargo run --release -- monitor --help
```

### Dry run

It's possible to mine a solution locally without submitting anything to the
chain but it works only on blocks with a snapshot
(when the event Phase::Signed → Phase::Off is emitted).

```bash
$ cargo run --release -- --uri ws://localhost:9944 dry-run --at 0xba86a0ba663df496743eeb077d004ef86bd767716e0d8cb935ab90d3ae174e85 seq-phragmen
```

### Emergency solution

Mine a solution that can be submitted as an emergency solution.

```bash
$ cargo run --release -- --uri ws://localhost:9944 emergency-solution --at 0xba86a0ba663df496743eeb077d004ef86bd767716e0d8cb935ab90d3ae174e85 seq-phragmen
```

### Info command

Check if the polkadot-staking-miner's metadata is compatible with a remote node.

```bash
$ cargo run --release -- --uri wss://rpc.polkadot.io info
Remote_node:
{
  "spec_name": "polkadot",
  "impl_name": "parity-polkadot",
  "spec_version": 9431,
  "impl_version": 0,
  "authoring_version": 0,
  "transaction_version": 24,
  "state_version": 0
}
Compatible: YES
```

### Prepare your SEED

While you could pass your seed directly to the cli or Docker, this is highly **NOT** recommended. Instead, you should use an ENV variable.

You can set it manually using:
```bash
# The following line starts with an extra space on purpose, make sure to include it:
 SEED=0x1234...
```

Alternatively, for development, you may store your seed in a `.env` file that
can be loaded to make your seed available:

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
    -e URI=wss://your-node:9944 \
    polkadot-staking-miner dry-run
```

## Update metadata

The static metadata file is stored at [artifacts/metadata.scale](artifacts/metadata.scale)

To update the metadata you need to connect to a polkadot, kusama or westend node.

```bash
# Install subxt-cli
$ cargo install --locked subxt-cli
# Download the metadata from a local node and replace the current metadata
# See `https://github.com/paritytech/subxt/tree/master/cli` for further documentation of the `subxt-cli` tool.
$ subxt metadata --pallets "ElectionProviderMultiPhase,System" -f bytes > artifacts/metadata.scale
# Inspect the generated code
$ subxt codegen --file artifacts/metadata.scale | rustfmt > code.rs
```

## Test locally

Because elections occurs quite rarely and if you want to test it locally
it's possible build the polkadot binary with `feature --fast-runtime`
to ensure that elections occurs more often (in order of minutes rather hours/days).

```bash
$ cargo run --release --package polkadot --features fast-runtime -- --chain westend-dev --tmp --alice --execution Native -lruntime=debug --offchain-worker=Never --ws-port 9999
# open another terminal and run
$ cargo run --release -- --uri ws://localhost:9999 monitor --seed-or-path //Alice seq-phragmen
```

## Prometheus metrics

The staking-miner starts a prometheus server on port 9999 and that metrics can
be fetched by:

```bash
$ curl localhost:9999/metrics
```


```bash
# HELP staking_miner_balance The balance of the staking miner account
# TYPE staking_miner_balance gauge
staking_miner_balance 88756574897390270
# HELP staking_miner_mining_duration_ms The mined solution time in milliseconds.
# TYPE staking_miner_mining_duration_ms gauge
staking_miner_mining_duration_ms 50
# HELP staking_miner_score_minimal_stake The minimal winner, in terms of total backing stake
# TYPE staking_miner_score_minimal_stake gauge
staking_miner_score_minimal_stake 24426059484170936
# HELP staking_miner_score_sum_stake The sum of the total backing of all winners
# TYPE staking_miner_score_sum_stake gauge
staking_miner_score_sum_stake 2891461667266507300
# HELP staking_miner_score_sum_stake_squared The sum squared of the total backing of all winners, aka. the variance.
# TYPE staking_miner_score_sum_stake_squared gauge
staking_miner_score_sum_stake_squared 83801161022319280000000000000000000
# HELP staking_miner_solution_length_bytes Number of bytes in the solution submitted
# TYPE staking_miner_solution_length_bytes gauge
staking_miner_solution_length_bytes 2947
# HELP staking_miner_solution_weight Weight of the solution submitted
# TYPE staking_miner_solution_weight gauge
staking_miner_solution_weight 8285574626
# HELP staking_miner_submissions_started Number of submissions started
# TYPE staking_miner_submissions_started counter
staking_miner_submissions_started 2
# HELP staking_miner_submissions_success Number of submissions finished successfully
# TYPE staking_miner_submissions_success counter
staking_miner_submissions_success 2
# HELP staking_miner_submit_and_watch_duration_ms The time in milliseconds it took to submit the solution to chain and to be included in block
# TYPE staking_miner_submit_and_watch_duration_ms gauge
staking_miner_submit_and_watch_duration_ms 17283
```

## Related projects

- [substrate-etl](https://github.com/gpestana/substrate-etl) - a tool fetch state from substrate-based chains.
