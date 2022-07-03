# Staking Miner v2

WARNING this library is under active development DO NOT USE IN PRODUCTION.

The library is a re-write of [polkadot staking miner](https://github.com/paritytech/polkadot/tree/master/utils/staking-miner) using [subxt](https://github.com/paritytech/subxt)
to avoid hard dependency to each runtime version.

The intention is that this library will "only break" once the [pallet-election-provider-multi-phase](https://crates.parity.io/pallet_election_provider_multi_phase/index.html) breaks i.e, not on every runtime upgrade.

You can check the help with:
```
staking-miner --help
```


## Building

```
cargo build --release --locked
```

## Docker

TODO.

### Running

A Docker container, especially one holding one of your `SEED` should be kept as secure as possible.
While it won't prevent a malicious actor to read your `SEED` if they gain access to your container, it is nonetheless recommended running this container in `read-only` mode:

```
# The following line starts with an extra space on purpose:
 SEED=0x1234...

docker run --rm -it \
    --name staking-miner \
    --read-only \
    -e RUST_LOG=info \
    -e SEED=$SEED \
    -e URI=wss://your-node:9944 \
    staking-miner dry-run
```

### Test locally

1. $ cargo build --package polkadot --features fast-runtime
2. $ polkadot --chain polkadot-dev --tmp --alice --execution Native -lruntime=debug --offchain-worker=Always --ws-port 9999
3. $ staking-miner --uri ws://localhost:9999 --seed-or-path //Alice monitor phrag-mms
