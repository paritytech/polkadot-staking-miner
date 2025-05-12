# Release Checklist

These steps assume that you've checked out the staking-miner repository and are in the root directory of it.

We also assume that ongoing work done is being merged directly to the `main` branch.

1.  Ensure that everything you'd like to see released is on the `main` branch.

2.  Create a release branch off `main`, for example `chore-release-v0.15.0`. Decide how far the version needs to be bumped based
    on the changes to date. If unsure what to bump the version to (e.g. is it a major, minor or patch release), check with the
    Parity Tools team.

3.  Bump the crate version in `Cargo.toml` to whatever was decided in step 2. The easiest approach is to search and replace, checking
    that you didn't replace any other crate versions along the way.

4.  Update chainspecs if needed for `kusama` and `polkadot`:

```bash
git clone https://github.com/polkadot-fellows/runtimes && cd runtimes
git checkout v1.5.0 # use the release you want to test against
cargo build --release -p chain-spec-generator --no-default-features --features fast-runtime,kusama,polkadot
# generate RAW specs and copy to chainspecs folder within staking-miner repo
./target/release/chain-spec-generator kusama-dev --raw > <staking miner repo root>/chainspecs/kusama-dev-raw-spec.json
./target/release/chain-spec-generator polkadot-dev --raw > <staking miner repo root>/chainspecs/polkadot-dev-raw-spec.json
```

5.  Update `CHANGELOG.md` to reflect the difference between this release and the last. If you're unsure of
    what to add, check with the Tools team. See the `CHANGELOG.md` file for details of the format it follows.

    First, if there have been any significant changes, add a description of those changes to the top of the
    changelog entry for this release. This will help people to understand the impact of the change and what they need to do
    to adopt it.

    Next, you can use the following script to generate the merged PRs between releases:

    ```
    ./scripts/generate_changelog.sh
    ```

    Ensure that the script picked the latest published release tag (e.g. if releasing `v0.15.0`, the script should
    provide something like `[+] Latest release tag: v0.14.0` ). Then group the PRs into "Fixed", "Added" and "Changed" sections,
    and make any other adjustments that you feel are necessary for clarity.

6.  Commit any of the above changes to the release branch and open a PR in GitHub with a base of `main`. Name the branch something
    like `chore(release): v0.15.0`.

7.  Once the branch has been reviewed and passes CI, merge it.

8.  Run tests against the latest polkadot-sdk release:

    ```
    git clone https://github.com/paritytech/polkadot-sdk && cd polkadot-sdk
    git checkout polkadot-v1.18.1 # use the release you want to test against
    cargo build --features fast-runtime
    cp ./target/debug/polkadot /usr/local/bin/polkadot # have the polkadot binary in your path requires by the tests.
    cd .. # assumes you were in the staking-miner repo
    cargo test --workspace --all-features -- --nocapture
    ```

9.  Now, we're ready to publish the release to crates.io. Run `cargo publish` to do that.

10. If the release was successful, tag the commit that we released in the `main` branch with the
    version that we just released, for example:

    ```
    git tag -s v0.15.0 # use the version number you've just published to crates.io, not this
    git push --tags
    ```

    Once this is pushed, go along to [the releases page on GitHub](https://github.com/paritytech/staking-miner-v2/releases)
    and draft a new release which points to the tag you just pushed to `main` above. Copy the changelog comments
    for the current release into the release description.
