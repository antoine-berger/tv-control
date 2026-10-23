# Qualification status

Implemented behavior and deployment qualification are separate. Use current
[CI results](https://github.com/antoine-berger/tv-control/actions) for automated
checks and record the tested source commit for any hardware result.

| Area | Evidence and remaining work |
| --- | --- |
| Controller core | Typed intents, deadlines, postconditions and bounded volatile coordination; contract tests cover these behaviors |
| CLI and MCP | Local CLI and bounded cancellable stdio server; loopback integration fixtures |
| ADB | Explicit-target executable adapter with version preflight; dedicated daemon required and check/use race remains |
| Jellyfin | Exact session/media selection, version gate and position evidence; mock and loopback contracts |
| Native builds | Linux/macOS CI; see current run status rather than assuming a past run covers a new commit |
| ARM64 packaging | musl cross-build and static ELF inspection; installed-router execution remains untested |
| Host performance | Historical macOS snapshot for the initial build; no measurements of later corrections |
| Hardware | Initial read-only preflight is historical evidence only; qualification of corrected builds requires separately recorded results |
| Hosted integration | Native bridge implemented; real TV/tunnel/account/plugin journey remains unqualified. Local MCP operation does not establish hosted access |
| Complete onboarding pipeline | Native ADB/MCP/tunnel composition, private progress/logs and cooperative verification implemented; simulated terminal/installer contracts cover the journey; real TV and hosted account qualification remain outstanding; see ADR 011 |
| End-to-end latency | Ten-second user-submission-to-physical-effect and displayed-confirmation target remains unproven |
| Router resources | Firmware ABI, smoke test and complete controller/ADB/curl/tunnel resource costs remain unqualified |

The [review notes](docs/REVIEW.md) identify corrected regressions and the remaining
ADB limitation. The [acceptance protocol](docs/ACCEPTANCE.md) defines evidence for
hardware and latency claims. [Router packaging](docs/ROUTER.md) and the
[benchmark scope](benchmarks/README.md) describe what their existing artifacts
can establish. No installation or hardware qualification follows automatically
from publication or a successful fixture.
