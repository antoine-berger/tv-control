# Delivery checklist

Goals tooling is not exposed in the task's execution interface. No `/goal`
activation is claimed. This checklist records the implementation scope.

- [x] Independent domain and application crates with outbound ports.
- [x] Typed, bounded configuration with secret references and no personal defaults.
- [x] CLI and narrow cancellable MCP stdio transport.
- [x] Explicit maintained ADB executable adapter; version guard with documented server-replacement limitation.
- [x] Jellyfin adapter and mock contracts, exact identity and patched baseline checks.
- [x] Deadline, postcondition, uncertain-result and coordination tests.
- [x] Final format/lint/test and architecture checks after hardening.
- [x] ARM64 musl crossbuild and static ELF inspection.
- [x] Host binary/RSS/CPU/latency measurements, scope explicitly labelled.
- [x] Tracked-content privacy audit and initial public repository publication.
- [ ] Independent review and CI success on the pushed commit.
- [x] Read-only hardware preflight (initial commit only; 430 ms local wall time).
- [ ] Bounded reversible hardware tests on the corrected build.
- [ ] Hosted main-chat integration and user-to-TV latency acceptance.
- [ ] Installed router ABI inspection, smoke test and resource qualification.

The remaining hardware and integration items are independent qualification steps. Publishing a build or
passing fixture tests does not complete them.
