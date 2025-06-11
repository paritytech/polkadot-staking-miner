# Changelog

The format is based on [Keep a Changelog].

[Keep a Changelog]: http://keepachangelog.com/en/1.0.0/

## [v1.8.0] - 2025-05-26

This release brings several important changes:

- Updated to work with `election-provider-multi-block` pallet and fully support multi-block staking with experimental monitoring capabilities via a new monitor command `experimental-monitor-multi-block`
- Added support for chunked submission strategy for multi-block solutions
- Fixed snapshot functionality after metadata update, to handle storage items keyed by a round index
- Bumped Rust to 1.85 / edition 2024
- Bumped subxt to 0.41.0 and updated hyper to v1.6.0
- Update chainspecs for Polkadot and Kusama to be compatible with runtime `v1.5.1`

### [Added]

- feat: `experimental-monitor-multi-block` support ([#955](https://github.com/paritytech/staking-miner-v2/pull/955))
- Add chunked submission strategy for multi-block solutions ([#1016](https://github.com/paritytech/staking-miner-v2/pull/1016))
- multi block miner: do_reduce configurable via CLI ([#1008](https://github.com/paritytech/staking-miner-v2/pull/1008))

### [Fixed]

- Fix snapshot fn after metadata update ([#1037](https://github.com/paritytech/staking-miner-v2/pull/1037))

### [Changed]

- chore(deps): bump hyper-util from 0.1.11 to 0.1.12 ([#1062](https://github.com/paritytech/staking-miner-v2/pull/1062))
- chore(deps): bump tokio from 1.45.0 to 1.45.1 ([#1061](https://github.com/paritytech/staking-miner-v2/pull/1061))
- Bump Rust to 1.85 / edition 2024 and update docker image for CI/CD accordingly ([#1047](https://github.com/paritytech/staking-miner-v2/pull/1047))
- tests: add e2e tests for multi-block miner ([#1040](https://github.com/paritytech/staking-miner-v2/pull/1040))
- chore(deps): bump tokio from 1.44.2 to 1.45.0 ([#1043](https://github.com/paritytech/staking-miner-v2/pull/1043))
- chore(deps): bump clap from 4.5.37 to 4.5.38 ([#1042](https://github.com/paritytech/staking-miner-v2/pull/1042))
- chore(deps): bump actions/download-artifact from 4.2.1 to 4.3.0 ([#1039](https://github.com/paritytech/staking-miner-v2/pull/1039))
- chore(deps): bump actions/download-artifact from 4.2.1 to 4.3.0 ([#1039](https://github.com/paritytech/staking-miner-v2/pull/1039))
- Update multiblock metadata (tracking SDK rev 8da42e8fae) ([#1036](https://github.com/paritytech/staking-miner-v2/pull/1036))
- update polkadot-sdk ([#1034](https://github.com/paritytech/staking-miner-v2/pull/1034))
- Update hyper to v1.6.0 ([#1022](https://github.com/paritytech/staking-miner-v2/pull/1022))
- doc: no multiple instances of the miner with the same account ([#1021](https://github.com/paritytech/staking-miner-v2/pull/1021))
- Bump subxt to `0.41.0` ([#1015](https://github.com/paritytech/staking-miner-v2/pull/1015))
- Update miner to properly target multi-block staking ([#1012](https://github.com/paritytech/staking-miner-v2/pull/1012))
- chore(deps): bump prometheus from 0.13.4 to 0.14.0 ([#1005](https://github.com/paritytech/staking-miner-v2/pull/1005))
- chore(deps): bump jsonrpsee from 0.24.7 to 0.24.9 ([#964](https://github.com/paritytech/staking-miner-v2/pull/964), [#1003](https://github.com/paritytech/staking-miner-v2/pull/1003))
- chore(deps): bump polkadot-sdk from 0.9.0 to 0.12.1 ([#970](https://github.com/paritytech/staking-miner-v2/pull/970), [#975](https://github.com/paritytech/staking-miner-v2/pull/975))
- chore(deps): bump tokio from 1.43.0 to 1.44.2 ([#992](https://github.com/paritytech/staking-miner-v2/pull/992), [#999](https://github.com/paritytech/staking-miner-v2/pull/999), [#1009](https://github.com/paritytech/staking-miner-v2/pull/1009))
- cleanup todos with tracking issues ([#996](https://github.com/paritytech/staking-miner-v2/pull/996))
- chore: use rustfmt stable ([#972](https://github.com/paritytech/staking-miner-v2/pull/972))

### Compatibility

Multi-phase election tested against:

- Westend v1,018,001
- Kusama v1,018,001
- Polkadot v1,018,001

Multi-block election was tested locally using `zombienet-cli` vs `polkadot-sdk` `v1.18.2`, waiting for the actual support for multi-block to be deployed on Westend network.

## [v1.7.0] - 2025-01-21

This release comes with the following changes:

- The polkadot-staking-miner now disables the feature `substrate-compat` in subxt to avoid duplicate versions of polkadot-sdk dependencies
- Support substrate-node which is treated as westend node.
- Move to the polkadot-sdk umbrella crate.

### [Changed]

- chore(deps): bump log from 0.4.22 to 0.4.25 ([#961](https://github.com/paritytech/staking-miner-v2/pull/961))
- chore(deps): bump serde_json from 1.0.135 to 1.0.137 ([#962](https://github.com/paritytech/staking-miner-v2/pull/962))
- allow substrate node equal to westend ([#960](https://github.com/paritytech/staking-miner-v2/pull/960))
- chore(deps): bump serde_json from 1.0.134 to 1.0.135 ([#956](https://github.com/paritytech/staking-miner-v2/pull/956))
- chore(deps): bump clap from 4.5.23 to 4.5.26 ([#957](https://github.com/paritytech/staking-miner-v2/pull/957))
- chore(deps): bump thiserror from 2.0.9 to 2.0.11 ([#958](https://github.com/paritytech/staking-miner-v2/pull/958))
- chore(deps): bump tokio from 1.42.0 to 1.43.0 ([#959](https://github.com/paritytech/staking-miner-v2/pull/959))
- chore(deps): bump pin-project-lite from 0.2.15 to 0.2.16 ([#954](https://github.com/paritytech/staking-miner-v2/pull/954))
- chore(deps): bump polkadot-sdk from 0.7.0 to 0.9.0 ([#953](https://github.com/paritytech/staking-miner-v2/pull/953))
- chore(deps): bump anyhow from 1.0.94 to 1.0.95 ([#952](https://github.com/paritytech/staking-miner-v2/pull/952))
- chore(deps): bump serde_json from 1.0.133 to 1.0.134 ([#949](https://github.com/paritytech/staking-miner-v2/pull/949))
- chore(deps): bump thiserror from 2.0.6 to 2.0.9 ([#950](https://github.com/paritytech/staking-miner-v2/pull/950))
- chore(deps): bump serde from 1.0.215 to 1.0.217 ([#951](https://github.com/paritytech/staking-miner-v2/pull/951))
- chore(deps): bump hyper from 0.14.31 to 0.14.32 ([#948](https://github.com/paritytech/staking-miner-v2/pull/948))
- chore(deps): bump thiserror from 2.0.3 to 2.0.6 ([#943](https://github.com/paritytech/staking-miner-v2/pull/943))
- chore(deps): bump clap from 4.5.21 to 4.5.23 ([#942](https://github.com/paritytech/staking-miner-v2/pull/942))
- chore(deps): bump tokio from 1.41.1 to 1.42.0 ([#944](https://github.com/paritytech/staking-miner-v2/pull/944))
- chore(deps): bump anyhow from 1.0.93 to 1.0.94 ([#945](https://github.com/paritytech/staking-miner-v2/pull/945))
- chore(deps): bump tracing-subscriber from 0.3.18 to 0.3.19 ([#940](https://github.com/paritytech/staking-miner-v2/pull/940))
- chore(deps): bump scale-info from 2.11.5 to 2.11.6 ([#939](https://github.com/paritytech/staking-miner-v2/pull/939))
- chore(deps): bump clap from 4.5.20 to 4.5.21 ([#938](https://github.com/paritytech/staking-miner-v2/pull/938))
- chore(deps): bump serde_json from 1.0.132 to 1.0.133 ([#936](https://github.com/paritytech/staking-miner-v2/pull/936))
- chore(deps): bump serde from 1.0.214 to 1.0.215 ([#937](https://github.com/paritytech/staking-miner-v2/pull/937))
- chore(deps): bump thiserror from 1.0.67 to 2.0.3 ([#933](https://github.com/paritytech/staking-miner-v2/pull/933))
- chore(deps): bump anyhow from 1.0.92 to 1.0.93 ([#932](https://github.com/paritytech/staking-miner-v2/pull/932))
- chore(deps): bump tokio from 1.41.0 to 1.41.1 ([#931](https://github.com/paritytech/staking-miner-v2/pull/931))
- chore(deps): bump serde from 1.0.213 to 1.0.214 ([#930](https://github.com/paritytech/staking-miner-v2/pull/930))
- chore(deps): bump thiserror from 1.0.65 to 1.0.67 ([#929](https://github.com/paritytech/staking-miner-v2/pull/929))
- chore(deps): bump anyhow from 1.0.91 to 1.0.92 ([#928](https://github.com/paritytech/staking-miner-v2/pull/928))
- remove `subxt substrate-compat` feature ([#927](https://github.com/paritytech/staking-miner-v2/pull/927))
- chore(deps): move to polkadot-sdk umbrella crate ([#926](https://github.com/paritytech/staking-miner-v2/pull/926))
- chore(deps): bump serde from 1.0.210 to 1.0.213 ([#922](https://github.com/paritytech/staking-miner-v2/pull/922))
- chore(deps): bump anyhow from 1.0.90 to 1.0.91 ([#923](https://github.com/paritytech/staking-miner-v2/pull/923))
- chore(deps): bump regex from 1.11.0 to 1.11.1 ([#924](https://github.com/paritytech/staking-miner-v2/pull/924))
- chore(deps): bump pin-project-lite from 0.2.14 to 0.2.15 ([#925](https://github.com/paritytech/staking-miner-v2/pull/925))
- chore(deps): bump thiserror from 1.0.64 to 1.0.65 ([#921](https://github.com/paritytech/staking-miner-v2/pull/921))

### Compatibility

Tested against:

- Westend v1,017,001
- Kusama v1,002,005
- Polkadot v1,003,004

## [v1.6.0] - 2024-10-25

This release primarily updates the staking-miner to be able to decode v5 extrinsics
which may be required for future releases, see https://github.com/paritytech/polkadot-sdk/pull/3685 for more information.

### [Changed]

- chore(deps): bump subxt to 0.38 and related deps ([#918](https://github.com/paritytech/staking-miner-v2/pull/918))

### Compatibility

Tested against:

- Westend v1,016,001
- Kusama v1,003,003
- Polkadot v1,003,003

## [v1.5.1] - 2024-07-03

This is a bug-fix release that changes internal trimming data structure from
BTreeMap to MinHeap to support voters with the same weight.

### [Fixed]

- fix: feasibility check when voters have the same staked amount ([#856](https://github.com/paritytech/staking-miner-v2/pull/856))

### [Changed]

- chore(deps): bump tokio from 1.37.0 to 1.38.0 ([#845](https://github.com/paritytech/staking-miner-v2/pull/845))
- chore(deps): bump regex from 1.10.4 to 1.10.5 ([#844](https://github.com/paritytech/staking-miner-v2/pull/844))
- chore(deps): bump anyhow from 1.0.82 to 1.0.86 ([#842](https://github.com/paritytech/staking-miner-v2/pull/842))
- chore(deps): bump hyper from 0.14.28 to 0.14.29 ([#846](https://github.com/paritytech/staking-miner-v2/pull/846))
- chore(deps): bump serde_json from 1.0.117 to 1.0.119 ([#857](https://github.com/paritytech/staking-miner-v2/pull/857))
- chore(deps): bump frame-support from 34.0.0 to 35.0.0 ([#853](https://github.com/paritytech/staking-miner-v2/pull/853))
- chore(deps): bump sp-runtime from 37.0.0 to 38.0.0 ([#855](https://github.com/paritytech/staking-miner-v2/pull/855))
- chore(deps): bump clap from 4.5.4 to 4.5.7 ([#850](https://github.com/paritytech/staking-miner-v2/pull/850))
- chore: update polkadot-sdk deps ([#861](https://github.com/paritytech/staking-miner-v2/pull/861))

### Compatibility

Tested against:

- Westend v1,014,000
- Kusama v1,002,006
- Polkadot v1,002,006

## [v1.5.0] - 2024-06-07

This release updates subxt to support the signed extension [CheckMetadataHash](https://github.com/paritytech/polkadot-sdk/pull/4274)

### [Changed]

- chore(deps): update polkadot-sdk dependencies ([#840](https://github.com/paritytech/staking-miner-v2/pull/840))
- chore(deps): bump subxt to v0.37.0 ([#836](https://github.com/paritytech/staking-miner-v2/pull/836))
- chore(deps): bump prometheus from 0.13.3 to 0.13.4 ([#825](https://github.com/paritytech/staking-miner-v2/pull/825))
- chore(deps): bump thiserror from 1.0.58 to 1.0.59 ([#822](https://github.com/paritytech/staking-miner-v2/pull/822))
- chore(deps): bump h2 from 0.3.24 to 0.3.26 in /staking-miner-playground ([#804](https://github.com/paritytech/staking-miner-v2/pull/804))
- chore(deps): bump sp-storage from 20.0.0 to 21.0.0 ([#812](https://github.com/paritytech/staking-miner-v2/pull/812))
- chore(deps): bump scale-info from 2.11.1 to 2.11.2 ([#814](https://github.com/paritytech/staking-miner-v2/pull/814))
- chore(deps): bump serde from 1.0.197 to 1.0.198 ([#818](https://github.com/paritytech/staking-miner-v2/pull/818))
- chore(deps): bump rustls from 0.21.10 to 0.21.11 ([#817](https://github.com/paritytech/staking-miner-v2/pull/817))
- chore(deps): bump anyhow from 1.0.81 to 1.0.82 ([#811](https://github.com/paritytech/staking-miner-v2/pull/811))

### Compatibility

Tested against:

- Westend v1010 and Westend master (rev d783ca9d9bf)

## [v1.4.0]

This is release to support that the SignedPhase and UnsignedPhase has been removed from the
metadata. See ([#803](https://github.com/paritytech/staking-miner-v2/pull/803)) for further information.

### Compatibility

Tested against:

- Westend v1010

## [v1.3.1] - 2023-12-27

The main changes of this release are as follows:

- Change the binary name to `polkadot-staking-miner` to publish on crates.io.
- Bump rust MSRV to 1.74
- Change `submit_signed_solution` extrinsic to be mortal
- A runtime upgrade bug was fixed in this release.

### Runtime upgrade bug fixed.

Recently, we noticed that it can be possible that the runtime upgrades won't
upgrade the metadata because the actual runtime upgrade is applied to the block
after `state_subscribeRuntimeVersion` emits an event.

For that reason, the polkadot-staking-miner now subscribes to `system().last_runtime_upgrade()` instead to fix that.

### [Changed]

- refactor: make solution extrinsic mortal ([#728](https://github.com/paritytech/staking-miner-v2/pull/728))
- rename project to polkadot-staking-miner ([#717](https://github.com/paritytech/staking-miner-v2/pull/717))

## [v1.3.0] - 2023-12-15 [YANKED]

The change to subxt-signer broke previous behaviour.

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

- add `info command` ([#596](https://github.com/paritytech/staking-miner-v2/pull/596))

## Changed

- strip metadata ([#595](https://github.com/paritytech/staking-miner-v2/pull/595))
- chore(deps): bump clap from 4.3.3 to 4.3.4 ([#602](https://github.com/paritytech/staking-miner-v2/pull/602))
- chore(deps): bump clap from 4.3.2 to 4.3.3 ([#601](https://github.com/paritytech/staking-miner-v2/pull/601))
- chore(deps): bump log from 0.4.18 to 0.4.19 ([#600](https://github.com/paritytech/staking-miner-v2/pull/600))
- chore: fix clippy warnings ([#599](https://github.com/paritytech/staking-miner-v2/pull/599))
- chore(deps): clap v4 ([#598](https://github.com/paritytech/staking-miner-v2/pull/598))
- chore(deps): bump serde from 1.0.163 to 1.0.164 ([#597](https://github.com/paritytech/staking-miner-v2/pull/597))
- chore(deps): bump futures from 0.3.27 to 0.3.28 ([#590](https://github.com/paritytech/staking-miner-v2/pull/590))
- chore(deps): bump once_cell from 1.17.2 to 1.18.0 ([#592](https://github.com/paritytech/staking-miner-v2/pull/592))
- chore(deps): bump regex from 1.8.3 to 1.8.4 ([#593](https://github.com/paritytech/staking-miner-v2/pull/593))

## [v1.0.0] - 2023-06-02

This is the first release staking-miner-v2 which makes it production ready
and the most noteable changes are:

- Add support for `emergency solutions`
- Change `submission strategy if-leading` to only submit if the score is better.
- Listen to `finalized heads` by default.

## Added

- add trimming tests ([#538](https://github.com/paritytech/staking-miner-v2/pull/538))
- Implements emergency solution command ([#557](https://github.com/paritytech/staking-miner-v2/pull/557))

## Changed

- update subxt ([#571](https://github.com/paritytech/staking-miner-v2/pull/571))
- chore(deps): bump once_cell from 1.17.1 to 1.17.2 ([#582](https://github.com/paritytech/staking-miner-v2/pull/582))
- chore(deps): bump log from 0.4.17 to 0.4.18 ([#580](https://github.com/paritytech/staking-miner-v2/pull/580))
- chore(deps): bump tokio from 1.28.1 to 1.28.2 ([#581](https://github.com/paritytech/staking-miner-v2/pull/581))
- chore(deps): bump regex from 1.8.2 to 1.8.3 ([#577](https://github.com/paritytech/staking-miner-v2/pull/577))
- chore: remove unused deps and features ([#575](https://github.com/paritytech/staking-miner-v2/pull/575))
- chore(deps): bump regex from 1.8.1 to 1.8.2 ([#574](https://github.com/paritytech/staking-miner-v2/pull/574))
- chore(deps): bump anyhow from 1.0.69 to 1.0.71 ([#570](https://github.com/paritytech/staking-miner-v2/pull/570))
- chore(deps): bump scale-info from 2.6.0 to 2.7.0 ([#569](https://github.com/paritytech/staking-miner-v2/pull/569))
- chore(deps): bump serde from 1.0.162 to 1.0.163 ([#565](https://github.com/paritytech/staking-miner-v2/pull/565))
- chore(deps): bump tokio from 1.28.0 to 1.28.1 ([#560](https://github.com/paritytech/staking-miner-v2/pull/560))
- chore(deps): bump serde from 1.0.160 to 1.0.162 ([#554](https://github.com/paritytech/staking-miner-v2/pull/554))
- improve README ([#587](https://github.com/paritytech/staking-miner-v2/pull/587))

## Fixed

- tests: read at most 1024 lines of logs before rpc server output ([#556](https://github.com/paritytech/staking-miner-v2/pull/556))
- helpers: parse rpc server addr ([#552](https://github.com/paritytech/staking-miner-v2/pull/552))
- change `submission strategy == if leading` to not submit equal score ([#589](https://github.com/paritytech/staking-miner-v2/pull/589))

### Compatibility

Tested against:

- Polkadot v9420
- Kusama v9420
- Westend v9420

## [v0.1.4] - 2023-05-04

This is a release to support new runtime changes in polkadot v0.9.42.

## [Added]

- Allow configuring winner count in dry-run ([#539](https://github.com/paritytech/staking-miner-v2/pull/539))
- add `feasibility_check` on mined solution ([#483](https://github.com/paritytech/staking-miner-v2/pull/483))

## [Changed]

- `commands` module to expose each sub-command, and put options next to them ([#540](https://github.com/paritytech/staking-miner-v2/pull/540))

## [Fixed]

- fix: update Cargo.lock for v0.1.3 ([#499](https://github.com/paritytech/staking-miner-v2/pull/499))

### Compatibility

Tested against:

- Polkadot v9420
- Kusama v9420
- Westend v9420

## [v0.1.3] - 2022-03-21

This is a patch release that fixes a bug which takes the `Weight::proof_size` into account.

### Fixed

- fix: use `Weight::proof_size` ([#481](https://github.com/paritytech/staking-miner-v2/pull/481))

### Changed

- chore: upgrade subxt v0.27 ([#464](https://github.com/paritytech/staking-miner-v2/pull/464))
- chore(deps): bump once_cell from 1.17.0 to 1.17.1 ([#463](https://github.com/paritytech/staking-miner-v2/pull/463))
- chore(deps): bump secp256k1 in /staking-miner-playground ([#470](https://github.com/paritytech/staking-miner-v2/pull/470))
- chore(deps): bump tokio in /staking-miner-playground ([#471](https://github.com/paritytech/staking-miner-v2/pull/471))
- chore: adjust ci staking-miner-playground ([#469](https://github.com/paritytech/staking-miner-v2/pull/469))
- chore(deps): bump paste from 1.0.11 to 1.0.12 ([#477](https://github.com/paritytech/staking-miner-v2/pull/477))
- chore(deps): bump thiserror from 1.0.38 to 1.0.39 ([#478](https://github.com/paritytech/staking-miner-v2/pull/478))
- chore(deps): bump serde_json from 1.0.93 to 1.0.94 ([#479](https://github.com/paritytech/staking-miner-v2/pull/479))
- chore(deps): bump tokio from 1.25.0 to 1.26.0 ([#476](https://github.com/paritytech/staking-miner-v2/pull/476))
- chore(deps): bump assert_cmd from 2.0.9 to 2.0.10 ([#493](https://github.com/paritytech/staking-miner-v2/pull/493))
- chore(deps): bump assert_cmd from 2.0.8 to 2.0.9 ([#492](https://github.com/paritytech/staking-miner-v2/pull/492))
- chore(deps): bump serde from 1.0.155 to 1.0.156 ([#490](https://github.com/paritytech/staking-miner-v2/pull/490))
- chore(deps): bump serde from 1.0.154 to 1.0.155 ([#487](https://github.com/paritytech/staking-miner-v2/pull/487))
- chore(deps): bump hyper from 0.14.24 to 0.14.25 ([#488](https://github.com/paritytech/staking-miner-v2/pull/488))
- chore(deps): bump futures from 0.3.26 to 0.3.27 ([#489](https://github.com/paritytech/staking-miner-v2/pull/489))
- chore(deps): bump serde from 1.0.153 to 1.0.154 ([#486](https://github.com/paritytech/staking-miner-v2/pull/486))
- chore: remove unused dependencies ([#485](https://github.com/paritytech/staking-miner-v2/pull/485))
- chore(deps): bump wasmtime from 5.0.0 to 5.0.1 ([#484](https://github.com/paritytech/staking-miner-v2/pull/484))
- chore(deps): bump serde from 1.0.152 to 1.0.153 ([#482](https://github.com/paritytech/staking-miner-v2/pull/482))
- Bump hyper from 0.14.23 to 0.14.24 ([#457](https://github.com/paritytech/staking-miner-v2/pull/457))
- Bump serde_json from 1.0.91 to 1.0.93 ([#460](https://github.com/paritytech/staking-miner-v2/pull/460))
- Bump futures from 0.3.25 to 0.3.26 ([#456](https://github.com/paritytech/staking-miner-v2/pull/456))
- Bump tokio from 1.24.2 to 1.25.0 ([#455](https://github.com/paritytech/staking-miner-v2/pull/455))
- Bump bumpalo from 3.10.0 to 3.12.0 ([#453](https://github.com/paritytech/staking-miner-v2/pull/453))
- Bump tokio from 1.24.1 to 1.24.2 ([#452](https://github.com/paritytech/staking-miner-v2/pull/452))

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

- feat: add delay and log CLI args ([#442](https://github.com/paritytech/staking-miner-v2/pull/442))

### Fixed

- fix: `dryRun check` + `fix multiple solutions bug` ([#443](https://github.com/paritytech/staking-miner-v2/pull/443))

### Changed

- Bump tokio from 1.23.1 to 1.24.1 ([#448](https://github.com/paritytech/staking-miner-v2/pull/448))
- Bump assert_cmd from 2.0.7 to 2.0.8 ([#449](https://github.com/paritytech/staking-miner-v2/pull/449))
- Bump tokio from 1.23.0 to 1.23.1 ([#447](https://github.com/paritytech/staking-miner-v2/pull/447))
- Bump serde from 1.0.151 to 1.0.152 ([#444](https://github.com/paritytech/staking-miner-v2/pull/444))
- Bump once_cell from 1.16.0 to 1.17.0 ([#446](https://github.com/paritytech/staking-miner-v2/pull/446))
- Bump paste from 1.0.9 to 1.0.11 ([#438](https://github.com/paritytech/staking-miner-v2/pull/438))
- Bump serde from 1.0.149 to 1.0.151 ([#439](https://github.com/paritytech/staking-miner-v2/pull/439))
- Bump serde_json from 1.0.89 to 1.0.91 ([#440](https://github.com/paritytech/staking-miner-v2/pull/440))
- Bump thiserror from 1.0.37 to 1.0.38 ([#441](https://github.com/paritytech/staking-miner-v2/pull/441))
- Bump secp256k1 from 0.24.0 to 0.24.2 ([#432](https://github.com/paritytech/staking-miner-v2/pull/432))
- Bump scale-info from 2.3.0 to 2.3.1 ([#434](https://github.com/paritytech/staking-miner-v2/pull/434))

### Compatibility

Tested against:

- Polkadot v9360
- Kusama v9360
- Westend v9360

## [v0.1.1] - 2022-12-06

This is a patch release that fixes a bug in `--listen finalized` before it was possible to submit multiple solutions because the solution was regarded as finalized when it was included in block.

### Fixed

- fix: `--listen finalized` wait for finalized xt ([#425](https://github.com/paritytech/staking-miner-v2/pull/425))

### Changed

- Bump jsonrpsee from 0.16.1 to 0.16.2 ([#427](https://github.com/paritytech/staking-miner-v2/pull/427))
- Bump assert_cmd from 2.0.6 to 2.0.7 ([#428](https://github.com/paritytech/staking-miner-v2/pull/428))
- Bump serde from 1.0.147 to 1.0.148 ([#424](https://github.com/paritytech/staking-miner-v2/pull/424))
- Bump serde_json from 1.0.88 to 1.0.89 ([#423](https://github.com/paritytech/staking-miner-v2/pull/423))

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
