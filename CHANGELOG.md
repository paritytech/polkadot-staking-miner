# Changelog

The format is based on [Keep a Changelog].

[Keep a Changelog]: http://keepachangelog.com/en/1.0.0/

## [v1.3.0] - 2023-12-14

The main changes of this release are as follows:
- Change the binary name to `polkadot-staking-miner` to publish on crates.io.
- Temporarly disable runtime upgrades more information below.
- Bump rust MSRV to 1.74
- Change `submit_signed_solution` extrinsic to be mortal

### Runtime upgrades are disabled

Recently, we noticed that it may be possible that the runtime upgrades won't 
upgrade the metadata because the actual runtime upgrade is applied to the block 
after 'state_subscribeRuntimeVersion' emits an event. 
For that reason, runtime upgrades are temporarily disabled, 
and the polkadot-staking-miner is terminated if a runtime upgrade occurs.

To deal with runtime upgrades, it should be sufficient to restart the client and wait a 
few seconds until another block is finalized to fetch the latest metadata or verify 
that the runtime upgrade has been applied successfully.

This is just a temporary fix and will be fixed in the next release.

### [Changed]
- refactor: make solution extrinsic mortal ([#728](https://github.com/paritytech/staking-miner-v2/pull/728))
- chore(deps): bump subxt, subxt-signer, scale-value ([#726](https://github.com/paritytech/staking-miner-v2/pull/726))
- chore(deps): bump clap from 4.4.10 to 4.4.11 ([#721](https://github.com/paritytech/staking-miner-v2/pull/721))
- chore(deps): bump tokio from 1.34.0 to 1.35.0 ([#724](https://github.com/paritytech/staking-miner-v2/pull/724))
- chore(deps): bump once_cell from 1.18.0 to 1.19.0 ([#722](https://github.com/paritytech/staking-miner-v2/pull/722))
- refactor: use `subxt-signer` to reduce the number of deps ([#720](https://github.com/paritytech/staking-miner-v2/pull/720))
- chore(deps): bump clap from 4.4.8 to 4.4.10 ([#719](https://github.com/paritytech/staking-miner-v2/pull/719))
- rename project to polkadot-staking-miner ([#717](https://github.com/paritytech/staking-miner-v2/pull/717))

## [v1.2.0] - 2023-11-23

The major changes of this release:
- Trimming has been reworked such that the staking-miner "pre-trims" the solution instead relying on the remote node to perform the trimming.
- The default port number in the URI can now be omitted.
- All dependencies are fetched from crates.io and the staking-miner can now be published on crates.io
- New prometheus metrics "staking_miner_trim_started" and "staking_miner_trim_success" have been added to monitor the trimming status.

### [Changed]
- chore: update deps to publish on crates.io ([#715](https://github.com/paritytech/staking-miner-v2/pull/715))
- refactor(trimming): implement pre-trimming ([#687](https://github.com/paritytech/staking-miner-v2/pull/687))

## [v1.1.0] - 2023-06-15

This release adds a subcommand `info` to detect whether the metadata 
of the staking-miner is compatible with a remote node.

```bash
$ staking-miner --uri wss://rpc.polkadot.io:443 info
Remote_node:
{
  "spec_name": "polkadot",
  "impl_name": "parity-polkadot",
  "spec_version": 9370,
  "impl_version": 0,
  "authoring_version": 0,
  "transaction_version": 20,
  "state_version": 0
}
Compatible: NO
```

## Added
- add `info command`  ([#596](https://github.com/paritytech/staking-miner-v2/pull/596))

## Changed
- strip metadata  ([#595](https://github.com/paritytech/staking-miner-v2/pull/595))
- chore(deps): bump clap from 4.3.3 to 4.3.4  ([#602](https://github.com/paritytech/staking-miner-v2/pull/602))
- chore(deps): bump clap from 4.3.2 to 4.3.3  ([#601](https://github.com/paritytech/staking-miner-v2/pull/601))
- chore(deps): bump log from 0.4.18 to 0.4.19  ([#600](https://github.com/paritytech/staking-miner-v2/pull/600))
- chore: fix clippy warnings  ([#599](https://github.com/paritytech/staking-miner-v2/pull/599))
- chore(deps): clap v4  ([#598](https://github.com/paritytech/staking-miner-v2/pull/598))
- chore(deps): bump serde from 1.0.163 to 1.0.164  ([#597](https://github.com/paritytech/staking-miner-v2/pull/597))
- chore(deps): bump futures from 0.3.27 to 0.3.28  ([#590](https://github.com/paritytech/staking-miner-v2/pull/590))
- chore(deps): bump once_cell from 1.17.2 to 1.18.0  ([#592](https://github.com/paritytech/staking-miner-v2/pull/592))
- chore(deps): bump regex from 1.8.3 to 1.8.4  ([#593](https://github.com/paritytech/staking-miner-v2/pull/593))

## [v1.0.0] - 2023-06-02

This is the first release staking-miner-v2 which makes it production ready
and the most noteable changes are:

 - Add support for `emergency solutions`
 - Change `submission strategy if-leading` to only submit if the score is better.
 - Listen to `finalized heads` by default.

## Added
- add trimming tests  ([#538](https://github.com/paritytech/staking-miner-v2/pull/538))
- Implements emergency solution command  ([#557](https://github.com/paritytech/staking-miner-v2/pull/557))

## Changed
- update subxt  ([#571](https://github.com/paritytech/staking-miner-v2/pull/571))
- chore(deps): bump once_cell from 1.17.1 to 1.17.2  ([#582](https://github.com/paritytech/staking-miner-v2/pull/582))
- chore(deps): bump log from 0.4.17 to 0.4.18  ([#580](https://github.com/paritytech/staking-miner-v2/pull/580))
- chore(deps): bump tokio from 1.28.1 to 1.28.2  ([#581](https://github.com/paritytech/staking-miner-v2/pull/581))
- chore(deps): bump regex from 1.8.2 to 1.8.3  ([#577](https://github.com/paritytech/staking-miner-v2/pull/577))
- chore: remove unused deps and features  ([#575](https://github.com/paritytech/staking-miner-v2/pull/575))
- chore(deps): bump regex from 1.8.1 to 1.8.2  ([#574](https://github.com/paritytech/staking-miner-v2/pull/574))
- chore(deps): bump anyhow from 1.0.69 to 1.0.71  ([#570](https://github.com/paritytech/staking-miner-v2/pull/570))
- chore(deps): bump scale-info from 2.6.0 to 2.7.0  ([#569](https://github.com/paritytech/staking-miner-v2/pull/569))
- chore(deps): bump serde from 1.0.162 to 1.0.163  ([#565](https://github.com/paritytech/staking-miner-v2/pull/565))
- chore(deps): bump tokio from 1.28.0 to 1.28.1  ([#560](https://github.com/paritytech/staking-miner-v2/pull/560))
- chore(deps): bump serde from 1.0.160 to 1.0.162  ([#554](https://github.com/paritytech/staking-miner-v2/pull/554))
- improve README  ([#587](https://github.com/paritytech/staking-miner-v2/pull/587))

## Fixed
- tests: read at most 1024 lines of logs before rpc server output  ([#556](https://github.com/paritytech/staking-miner-v2/pull/556))
- helpers: parse rpc server addr  ([#552](https://github.com/paritytech/staking-miner-v2/pull/552))
- change `submission strategy == if leading` to not submit equal score ([#589](https://github.com/paritytech/staking-miner-v2/pull/589))

### Compatibility

Tested against:
- Polkadot v9420
- Kusama v9420
- Westend v9420

## [v0.1.4] - 2023-05-04

This is a release to support new runtime changes in polkadot v0.9.42.

## [Added]
- Allow configuring winner count in dry-run  ([#539](https://github.com/paritytech/staking-miner-v2/pull/539))
- add `feasibility_check` on mined solution  ([#483](https://github.com/paritytech/staking-miner-v2/pull/483))

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
