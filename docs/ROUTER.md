# Router packaging

**Router execution is untested.** The candidate target is
`aarch64-unknown-linux-musl`: 64-bit little-endian ARMv8-A Linux. Target support
and a successful cross-build do not establish compatibility with installed
firmware, its kernel, userland or external executables.

## Build the candidate archive

Requires a host Rust toolchain with bundled `rust-lld` and the target's standard
library. If the target is not already present,
`rustup target add aarch64-unknown-linux-musl` installs it on the build host.
From the repository root:

```sh
python3 scripts/cross_build.py
python3 scripts/check_elf.py target/aarch64-unknown-linux-musl/release/tv-control
python3 scripts/package.py target/aarch64-unknown-linux-musl/release/tv-control dist/tv-control-aarch64-linux-musl.tar.gz
```

The build uses self-contained linking, static CRT, LTO, one codegen unit, stripped
symbols and abort-on-panic. Do not use `target-cpu=native` for cross-delivery.
The ELF check requires AArch64 ELF64 and rejects `PT_INTERP` and `DT_NEEDED`.
It inspects the file without executing it.

The archive includes the controller, license, README and generic configuration/MCP
examples, plus a separate SHA-256 file. It does not bundle ADB, curl, a tunnel or
the `docs` directory; read the guides in the
[source repository](https://github.com/antoine-berger/tv-control#documentation).
Building the archive does not require router credentials or network access to it.

## Qualify actual firmware

Before a separately authorized smoke test, inspect existing read-only system
information: board and firmware, `uname -m`, kernel version, package architecture
and libc/userland. Vendor package names are not Rust target triples. Firmware
derived from OpenWrt is not necessarily compatible with an upstream OpenWrt SDK.

Verify compatible maintained ADB/curl executables and their TLS capabilities.
Legacy ADB TCP is not encrypted by RSA authorization; old packages may lack
modern pairing/TLS. The controller requires an existing, exclusively owned
loopback daemon with the [documented residual race](ADAPTERS.md#adb-daemon-ownership).

Proposed controller budgets are idle RSS below 20 MiB, command RSS below 32 MiB
and idle CPU below 1% of one core. These are qualification targets, not measured
router results. Measure the controller, transient ADB/curl clients, ADB daemon
and any tunnel separately and together on the actual firmware. Host measurements
cannot substitute for those results.

## Service lifecycle

No `procd` service is installed. A future service needs a dedicated account,
bounded restarts and logs, explicit configuration/secret access and separately
authorized startup persistence. Local stdio MCP is normally supervised by its
client; an unattended stdin is not a service transport design. Router installation,
reboots, firewall changes, VPN configuration and package updates are outside the
packaging procedure.

## References

- [Rust target support](https://doc.rust-lang.org/rustc/platform-support/aarch64-unknown-linux-musl.html)
- [Rust linkage](https://doc.rust-lang.org/reference/linkage.html)
- [Android wireless ADB design](https://android.googlesource.com/platform/packages/modules/adb/+/refs/heads/main/docs/dev/adb_wifi.md)
- [OpenWrt ADB package recipe](https://git.openwrt.org/openwrt/openwrt/plain/package/utils/adb/Makefile)
