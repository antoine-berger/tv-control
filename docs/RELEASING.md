# Explicit cloud prereleases

Normal `main` and pull-request CI never publishes. The only publication job is
`release-prerelease`, after both existing CI matrices succeed. It uses the
GitHub-hosted runners already used by CI; no developer computer or stored token
is required. Only that job gets `contents: write` and `actions: read`.

## Prepare and trigger

1. Review and merge the release workflow and all final source fixes onto `main`.
   Wait for a successful **main push CI run for that exact full commit SHA**.
   A successful run for an earlier source commit does not cover workflow changes.
2. Read `[workspace.package].version` from `Cargo.toml`, currently `0.3.0`.
   Confirm there is no `v0.3.0` tag, release, or draft release. The publisher will
   refuse to overwrite or resume an existing one.
3. Let `C` be the exact tested main commit. Create branch
   `release/v0.3.0-C`, substituting the full 40-character SHA, initially at `C`'s
   first parent. Then fast-forward it to `C` with an expected-old-SHA lease.
   This produces a real push update without adding an empty commit or changing
   the tested SHA. Verify the resulting run has event `push`, head SHA `C`, and
   the exact selected release branch. Creating a branch at an already-existing
   commit alone is not relied on to produce a push workflow run.
4. Keep `main` at `C` through publication. The guard requires `main` HEAD and the
   checkout, push payload, run metadata, and release branch's encoded SHA to
   agree. A changed head, forced push, missing main CI success, or mismatch fails
   closed before any tag or release is created.

The branch name is an explicit release request by a repository writer. It does
not let a PR or fork publish. Do not create it until publication is authorized.
Do not weaken the guards, force a tag, or switch to `workflow_run` or
`pull_request_target` to work around a failed release.

## Asset provenance and publication

The publishing job downloads only the current run's immutable Actions artifacts
with a SHA-pinned `actions/download-artifact` and digest mismatches set to fail.
The three expected artifact directories are kept separate:

- `tv-control-macos-ARM64`
- `tv-control-aarch64-linux-musl`
- `tv-control-x86_64-linux-musl`

Exactly eight public files are permitted: the three qualified `.tar.gz` bundles,
their three `.sha256` files, `install.sh`, and `install.sh.sha256`. The verifier
checks archive members, checksums, architecture, source-file equality to the
selected checkout, and identical installers across all packages. It never runs
downloaded binaries or installers with the writer token. The original matrix
jobs perform the supported offline smoke tests before artifact upload.

The publisher creates the tag at the exact tested commit, creates a draft
prerelease with `make_latest: "false"`, uploads the eight files, and requires the
API-reported size, state and SHA-256 digest for every asset to match. It verifies
the full draft asset set and tag again before publishing with `prerelease: true`
and `make_latest: "false"`. The release body records the source SHA, main and
package CI run links, and all asset hashes.

No write is automatically retried. If a request fails or its response is lost,
inspect GitHub first: a tag or draft may remain. This workflow never deletes,
retags, overwrites, or resumes a partial publication automatically. Report that
state and obtain any needed approval for recovery.

## Verify publicly before updating the distribution branch

The repository's `latest` **branch** supplies the one-command installer at
`https://raw.githubusercontent.com/antoine-berger/tv-control/latest/install.sh`.
It is independent of GitHub's latest-release API. Keep this release a prerelease;
never PATCH it to stable or change `make_latest` for this workflow.

After publication finishes, use the read-only command
`python3 scripts/verify_public_release.py plan RELEASE_ID FULL_SOURCE_SHA`.
It checks the public release and original successful CI, then prints a branch:
`verify/vVERSION-FULL_SOURCE_SHA-rRELEASE_ID-FULL_MANIFEST_SHA256`.
The manifest digest is SHA-256 of compact, key-sorted JSON containing
`release_id`, `tag`, `source_sha`, and the complete `asset_sha256` mapping. It
binds the specific release ID and all eight original asset hashes, so a changed
asset or different release cannot pass under the same branch name.

Create that exact branch at the source commit's first parent, then leased
fast-forward it to the source commit, as with the publication trigger. Both
`verify-public-release` matrix jobs must finish successfully at the exact SHA:

1. Linux x86-64 and macOS ARM64 anonymously read public release metadata and
   download all eight files through the public release URLs. They require the
   original successful main/publication CI, exact tag SHA, exact asset inventory,
   API digest and recorded publication hashes.
2. Each job validates all three bundles against the selected source checkout,
   then runs native offline package install/discover/uninstall smoke tests.
   ARM64 Linux gets structural verification, not native execution.
3. Each job also executes the downloaded public installer with explicit
   `--version v0.3.0` in an isolated home/prefix, downloads from the real public
   endpoint, compares the installed executable with the verified package,
   performs fixture discovery, and uninstalls it.
4. Each job re-reads release metadata after execution and fails if it changed.
   Download or smoke failures fail the run. There is no write-capable dependent
   job, and verification branches skip the rebuild matrices entirely.

Only after both verification jobs pass, check the existing `latest` branch and
update it to the exact verified source commit using an expected-old-SHA lease.
If another change moved `latest`, stop and inspect it instead of overwriting it.
If the existing distribution branch is not an ancestor of the new source, any
force-with-lease update requires explicit authorization. The workflow does not
move branches itself. Re-read `latest` after the update and fetch its public raw
installer to verify it equals the tested source and defaults to the manifest
version. Never substitute a different commit or rely on only one successful
matrix job.

## Local guard tests

Run `python3 -W error::ResourceWarning scripts/test_release.py` and
`python3 -W error::ResourceWarning scripts/test_verify_public_release.py`. Tests use local
fixtures and a fake GitHub API; they never publish or make network requests.
The ordinary CI check matrix also runs them. `scripts/release.py` does not write
to GitHub without `--publish`, and still requires the complete GitHub context,
matching artifacts and all remote provenance guards when that flag is present.

## References

- [GitHub workflow events](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows)
- [GitHub token permissions](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax#permissions)
- [Artifact download isolation and digest checking](https://github.com/actions/download-artifact)
- [Release API and latest/prerelease behavior](https://docs.github.com/en/rest/releases/releases)
- [Git reference API](https://docs.github.com/en/rest/git/refs)
