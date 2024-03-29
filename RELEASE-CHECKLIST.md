# Release Checklist

These steps assume that you've checked out the staking-miner repository and are in the root directory of it.

We also assume that ongoing work done is being merged directly to the `main` branch.

1.  Ensure that everything you'd like to see released is on the `main` branch.

2.  Create a release branch off `main`, for example `chore-release-v0.15.0`. Decide how far the version needs to be bumped based
    on the changes to date. If unsure what to bump the version to (e.g. is it a major, minor or patch release), check with the
    Parity Tools team.

3.  Bump the crate version in `Cargo.toml` to whatever was decided in step 2. The easiest approach is to search and replace, checking
    that you didn't replace any other crate versions along the way.

4.  Update `CHANGELOG.md` to reflect the difference between this release and the last. If you're unsure of
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

5.  Commit any of the above changes to the release branch and open a PR in GitHub with a base of `main`. Name the branch something
    like `chore(release): v0.15.0`.

6.  Once the branch has been reviewed and passes CI, merge it.

7.  Run tests against the latest polkadot release:

    ```
    git clone https://github.com/paritytech/polkadot && cd polkadot
    git checkout v0.9.33 # use the release you want to test against
    cargo build --features fast-runtime
    cp ./target/debug/polkadot /usr/local/bin/polkadot # have the polkadot binary in your path requires by the tests.
    cd .. # assumes you were in the staking-miner repo
    cargo test --features slow-tests -- --nocapture
    ```

8.  Now, we're ready to publish the release to crates.io. Run `cargo publish` to do that.

9.  If the release was successful, tag the commit that we released in the `main` branch with the
    version that we just released, for example:

    ```
    git tag -s v0.15.0 # use the version number you've just published to crates.io, not this
    git push --tags
    ```

    Once this is pushed, go along to [the releases page on GitHub](https://github.com/paritytech/staking-miner-v2/releases)
    and draft a new release which points to the tag you just pushed to `main` above. Copy the changelog comments
    for the current release into the release description.
