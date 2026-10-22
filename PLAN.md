# Delivery checklist

Goals tooling is not exposed in the task's execution interface. No `/goal`
activation is claimed. This checklist records the implementation scope.

- [x] Independent domain and application crates with outbound ports.
- [x] Typed, bounded configuration with secret references and no personal defaults.
- [x] CLI and narrow cancellable MCP stdio transport.
- [x] Explicit maintained ADB executable adapter; no setup or discovery side effects.
- [x] Jellyfin adapter and mock contracts, exact identity and patched baseline checks.
- [x] Deadline, postcondition, uncertain-result and coordination tests.
- [x] Final format/lint/test and architecture checks after hardening.
- [x] ARM64 musl crossbuild and static ELF inspection.
- [x] Host binary/RSS/CPU/latency measurements, scope explicitly labelled.
- [ ] Tracked-content privacy audit and initial public repository publication.
- [ ] Independent review and CI success on the pushed commit.
- [ ] Separately authorized hardware preflight and bounded reversible tests.
- [ ] Hosted main-chat integration and user-to-TV latency acceptance.
- [ ] Installed router ABI inspection, smoke test and resource qualification.

The last three items are independent qualification steps. Publishing a build or
passing fixture tests does not complete them.
