# Changelog

The format is based on [Keep a Changelog].

[Keep a Changelog]: http://keepachangelog.com/en/1.0.0/

## [v0.1.2] - 2022-01-11

This is a release that adds a couple of new features/CLI options, a useful one is `--dry-run` which will check the validity of
the mined solution which for example can spot if an account has too low balance but it requires that the RPC
endpoint exposes `unsafe RPC methods` to work.

In addition, this release fixes another multiple solution bug.

### Added
- feat: add delay and log CLI args  ([#442](https://github.com/paritytech/staking-miner-v2/pull/442))

### Fixed
- fix: `dryRun check` + `fix multiple solutions bug`  ([#443](https://github.com/paritytech/staking-miner-v2/pull/443))

### Changed
- Bump tokio from 1.23.1 to 1.24.1  ([#448](https://github.com/paritytech/staking-miner-v2/pull/448))
- Bump assert_cmd from 2.0.7 to 2.0.8  ([#449](https://github.com/paritytech/staking-miner-v2/pull/449))
- Bump tokio from 1.23.0 to 1.23.1  ([#447](https://github.com/paritytech/staking-miner-v2/pull/447))
- Bump serde from 1.0.151 to 1.0.152  ([#444](https://github.com/paritytech/staking-miner-v2/pull/444))
- Bump once_cell from 1.16.0 to 1.17.0  ([#446](https://github.com/paritytech/staking-miner-v2/pull/446))
- Bump paste from 1.0.9 to 1.0.11  ([#438](https://github.com/paritytech/staking-miner-v2/pull/438))
- Bump serde from 1.0.149 to 1.0.151  ([#439](https://github.com/paritytech/staking-miner-v2/pull/439))
- Bump serde_json from 1.0.89 to 1.0.91  ([#440](https://github.com/paritytech/staking-miner-v2/pull/440))
- Bump thiserror from 1.0.37 to 1.0.38  ([#441](https://github.com/paritytech/staking-miner-v2/pull/441))
- Bump secp256k1 from 0.24.0 to 0.24.2  ([#432](https://github.com/paritytech/staking-miner-v2/pull/432))
- Bump scale-info from 2.3.0 to 2.3.1  ([#434](https://github.com/paritytech/staking-miner-v2/pull/434))

### Compatibility
Tested against:
- Polkadot v9360
- Kusama v9360
- Westend v9360

## [v0.1.1] - 2022-12-06

This is a patch release that fixes a bug in `--listen finalized` before it was possible to submit multiple solutions because the solution was regarded as finalized when it was included in block.

### Fixed

- fix: `--listen finalized` wait for finalized xt  ([#425](https://github.com/paritytech/staking-miner-v2/pull/425))

### Changed

- Bump jsonrpsee from 0.16.1 to 0.16.2  ([#427](https://github.com/paritytech/staking-miner-v2/pull/427))
- Bump assert_cmd from 2.0.6 to 2.0.7  ([#428](https://github.com/paritytech/staking-miner-v2/pull/428))
- Bump serde from 1.0.147 to 1.0.148  ([#424](https://github.com/paritytech/staking-miner-v2/pull/424))
- Bump serde_json from 1.0.88 to 1.0.89  ([#423](https://github.com/paritytech/staking-miner-v2/pull/423))

### Compatibility

Tested against:
- Polkadot v9330
- Kusama v9330
- Westend v9330

## [v0.1.0] - 2022-11-23

The first release of staking miner v2.

### Compatibility

Tested against:
- Polkadot v9330
- Kusama v9330
- Westend v9330
