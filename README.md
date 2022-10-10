# Staking Miner v2

[![Daily compatibility check against latest polkadot](https://github.com/paritytech/staking-miner-v2/actions/workflows/nightly.yml/badge.svg)](https://github.com/paritytech/staking-miner-v2/actions/workflows/nightly.yml)

WARNING this library is under active development DO NOT USE IN PRODUCTION.

The library is a re-write of [polkadot staking miner](https://github.com/paritytech/polkadot/tree/master/utils/staking-miner) using [subxt](https://github.com/paritytech/subxt)
to avoid hard dependency to each runtime version. It's using a static metadata (metadata.scale) file to generate rust types, instructions how to update metadata can be found [here](#update-metadata)

The intention is that this library will "only break" once the [pallet-election-provider-multi-phase](https://crates.parity.io/pallet_election_provider_multi_phase/index.html) breaks i.e, not on every runtime upgrade.



You can check the help with:
```
staking-miner --help
```

## Update metadata

The static metadata file is stored at [artifacts/metadata.scale](artifacts/metadata.scale)

To update the metadata you need to connect to a polkadot, kusama or westend node

```bash
# Install subxt-cli
$ cargo install --locked subxt-cli
# Download metadata from local node and replace the current metadata
# See `https://github.com/paritytech/subxt/tree/master/cli` for further documentation of the `subxt-cli` tool.
$ subxt metadata -f bytes > artifacts/metadata.scale
# Inspect the generated code
$ subxt codegen --file artifacts/metadata.scale | rustfmt +nightly > code.rs
```


## Building

```
cargo build --release --locked
```

## Prepare your SEED

While you could pass your seed directly to the cli or Docker, this is highly **NOT** recommended. Instead, you should use an ENV variable.

You can set it manually using:
```
# The following line starts with an extra space on purpose, make sure to include it:
 SEED=0x1234...
```

Alternatively, for development, you may store your seed in a `.env` file that can be loaded to make your seed available:

`.env`:
```
SEED=0x1234...
RUST_LOG=staking-miner=debug
```
You can load it using `source .env`.

## Running

### Docker

A Docker container, especially one holding one of your `SEED` should be kept as secure as possible.
While it won't prevent a malicious actor to read your `SEED` if they gain access to your container, it is nonetheless recommended running this container in `read-only` mode:

```
docker run --rm -it \
    --name staking-miner \
    --read-only \
    -e RUST_LOG=info \
    -e SEED \
    -e URI=wss://your-node:9944 \
    staking-miner dry-run
```

## Test locally

1. $ `cargo build --package polkadot --features fast-runtime`
2. $ `polkadot --chain polkadot-dev --tmp --alice --execution Native -lruntime=debug --offchain-worker=Always --ws-port 9999`
3. $ `staking-miner --uri ws://localhost:9999 --seed-or-path //Alice monitor phrag-mms`
