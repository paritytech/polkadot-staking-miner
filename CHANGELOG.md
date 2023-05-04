# Changelog

The format is based on [Keep a Changelog].

[Keep a Changelog]: http://keepachangelog.com/en/1.0.0/

## [v0.1.4] - 2023-05-04

This is release to support new runtime changes in polkadot v0.9.42

## [Added]
- Allow configuring winner count in dry-run  ([#539](https://github.com/paritytech/staking-miner-v2/pull/539))
- feat: add `feasibility_check` on mined solution  ([#483](https://github.com/paritytech/staking-miner-v2/pull/483))

## [Changed]
- `commands` module to expose each sub-command, and put options next to them  ([#540](https://github.com/paritytech/staking-miner-v2/pull/540))

## [Fixed]
- fix: update Cargo.lock for v0.1.3  ([#499](https://github.com/paritytech/staking-miner-v2/pull/499))

### Compatibility
Tested against:
- Polkadot v9420
- Kusama v9420
- Westend v9420

## [v0.1.3] - 2022-03-21

This is a patch release that fixes a bug which takes the `Weight::proof_size` into account.

### Fixed
- fix: use `Weight::proof_size`  ([#481](https://github.com/paritytech/staking-miner-v2/pull/481))

### Changed
- chore: upgrade subxt v0.27  ([#464](https://github.com/paritytech/staking-miner-v2/pull/464))
- chore(deps): bump once_cell from 1.17.0 to 1.17.1  ([#463](https://github.com/paritytech/staking-miner-v2/pull/463))
- chore(deps): bump secp256k1 in /staking-miner-playground  ([#470](https://github.com/paritytech/staking-miner-v2/pull/470))
- chore(deps): bump tokio in /staking-miner-playground  ([#471](https://github.com/paritytech/staking-miner-v2/pull/471))
- chore: adjust ci staking-miner-playground  ([#469](https://github.com/paritytech/staking-miner-v2/pull/469))
- chore(deps): bump paste from 1.0.11 to 1.0.12  ([#477](https://github.com/paritytech/staking-miner-v2/pull/477))
- chore(deps): bump thiserror from 1.0.38 to 1.0.39  ([#478](https://github.com/paritytech/staking-miner-v2/pull/478))
- chore(deps): bump serde_json from 1.0.93 to 1.0.94  ([#479](https://github.com/paritytech/staking-miner-v2/pull/479))
- chore(deps): bump tokio from 1.25.0 to 1.26.0  ([#476](https://github.com/paritytech/staking-miner-v2/pull/476))
- chore(deps): bump assert_cmd from 2.0.9 to 2.0.10  ([#493](https://github.com/paritytech/staking-miner-v2/pull/493))
- chore(deps): bump assert_cmd from 2.0.8 to 2.0.9  ([#492](https://github.com/paritytech/staking-miner-v2/pull/492))
- chore(deps): bump serde from 1.0.155 to 1.0.156  ([#490](https://github.com/paritytech/staking-miner-v2/pull/490))
- chore(deps): bump serde from 1.0.154 to 1.0.155  ([#487](https://github.com/paritytech/staking-miner-v2/pull/487))
- chore(deps): bump hyper from 0.14.24 to 0.14.25  ([#488](https://github.com/paritytech/staking-miner-v2/pull/488))
- chore(deps): bump futures from 0.3.26 to 0.3.27  ([#489](https://github.com/paritytech/staking-miner-v2/pull/489))
- chore(deps): bump serde from 1.0.153 to 1.0.154  ([#486](https://github.com/paritytech/staking-miner-v2/pull/486))
- chore: remove unused dependencies  ([#485](https://github.com/paritytech/staking-miner-v2/pull/485))
- chore(deps): bump wasmtime from 5.0.0 to 5.0.1  ([#484](https://github.com/paritytech/staking-miner-v2/pull/484))
- chore(deps): bump serde from 1.0.152 to 1.0.153  ([#482](https://github.com/paritytech/staking-miner-v2/pull/482))
- Bump hyper from 0.14.23 to 0.14.24  ([#457](https://github.com/paritytech/staking-miner-v2/pull/457))
- Bump serde_json from 1.0.91 to 1.0.93  ([#460](https://github.com/paritytech/staking-miner-v2/pull/460))
- Bump futures from 0.3.25 to 0.3.26  ([#456](https://github.com/paritytech/staking-miner-v2/pull/456))
- Bump tokio from 1.24.2 to 1.25.0  ([#455](https://github.com/paritytech/staking-miner-v2/pull/455))
- Bump bumpalo from 3.10.0 to 3.12.0  ([#453](https://github.com/paritytech/staking-miner-v2/pull/453))
- Bump tokio from 1.24.1 to 1.24.2  ([#452](https://github.com/paritytech/staking-miner-v2/pull/452))

### Compatibility
Tested against:
- Polkadot v9390
- Kusama v9390
- Westend v9390

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
