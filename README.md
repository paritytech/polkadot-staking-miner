# Polkadot staking miner

[![Daily compatibility check against latest polkadot](https://github.com/paritytech/polkadot-staking-miner/actions/workflows/nightly.yml/badge.svg)](https://github.com/paritytech/polkadot-staking-miner/actions/workflows/nightly.yml)

This is a re-write of the [staking miner](https://github.com/paritytech/polkadot/tree/master/utils/staking-miner) using [subxt](https://github.com/paritytech/subxt) to avoid hard dependency to each runtime version.

A key difference is that the miner only interacts with AssetHub nodes that support multi-page async staking and in particular the new multi-phase, multi-block election provider pallet (`EPMB`), while the old legacy miner is designed to work with the previous multi-phase, single-page election provider pallet.

The binary itself embeds [multi-block static metadata](./artifacts/multi_block.scale) to
generate a rust codegen at compile-time that [subxt provides](https://github.com/paritytech/subxt).

Runtime upgrades are handled by the polkadot-staking-miner by upgrading storage constants
and that will work unless there is a breaking change in the pallets `pallet-election-provider-multi-block` or `frame_system`.

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

## Important Warning

**Do not run multiple instances of the miner with the same account.**

The miner uses the on-chain nonce for a given user to submit solutions, which can lead to nonce
collisions if multiple miners are running with the same account. This can cause transaction
failures and potentially result in lost rewards or other issues.

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

The `--chunk-size` option controls how many solution pages are submitted concurrently. When set to 0 (default), all pages are submitted at once. When set to a positive number, pages are submitted in chunks of that size, waiting for each chunk to be included in a block before submitting the next chunk. This can help manage network load and improve reliability on congested networks or if the pages per solution increases.

```bash
polkadot-staking-miner --uri ws://127.0.0.1:9946 monitor --seed-or-path //Alice --chunk-size 4
```

The `--do-reduce` option (off by default) enables solution reduction, to prevent further trimming, making submission more efficient.

```bash
polkadot-staking-miner --uri ws://127.0.0.1:9966 monitor --seed-or-path //Alice --do-reduce
```

Other notable options are:

- `--listen` (default: finalized, possible values: finalized or head)
- `--submission-strategy` (default: `if-leading`)

Refer to `--help` for the full list of options.

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
    -e URI=wss://your-node:9946 \
    polkadot-staking-miner monitor
```

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

Because elections occurs quite rarely and if you want to test it locally
it's possible build the polkadot binary with `feature --fast-runtime`
to ensure that elections occurs more often (in order of minutes rather hours/days).

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
# HELP staking_miner_submit_and_watch_duration_ms The time in milliseconds it took to submit the solution to chain and to be included in block
# TYPE staking_miner_submit_and_watch_duration_ms gauge
staking_miner_submit_and_watch_duration_ms 17283
```

## Related projects

- [substrate-etl](https://github.com/gpestana/substrate-etl) - a tool fetch state from substrate-based chains.
