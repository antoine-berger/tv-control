# GitHub automation

[`workflows/ci.yml`](workflows/ci.yml) owns native checks, Linux cross-builds,
package checks and the guarded release/public-verification jobs.

Ordinary main/PR builds are read-only. Publication requires an explicitly named,
version-and-source-bound release branch and successful prerequisite checks.
Public verification uses original release bytes, not a rebuild, and keeps its
controller commit separate from the release source commit.

Read [releasing](../docs/RELEASING.md) before changing triggers or permissions.
A passing fixture/build job does not qualify physical TV playback, hosted account
access or router runtime. Keep action revisions pinned and permissions scoped to
the job that needs them.
