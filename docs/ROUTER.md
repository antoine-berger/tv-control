# Router packaging and qualification

Candidate target: `aarch64-unknown-linux-musl` (64-bit little-endian ARMv8-A Linux).
Rust lists the target as supported through rustup; this does not establish an
installed router's userland, kernel, firmware or external executable compatibility.
Do not use `target-cpu=native` for cross-delivery.

```sh
rustup target add aarch64-unknown-linux-musl
python3 scripts/cross_build.py
python3 scripts/check_elf.py target/aarch64-unknown-linux-musl/release/tv-control
python3 scripts/package.py target/aarch64-unknown-linux-musl/release/tv-control dist/tv-control-aarch64-linux-musl.tar.gz
```

The build uses bundled rust-lld, self-contained linking, static CRT, LTO, one
codegen unit, stripped symbols and abort-on-panic. The ELF check requires AArch64
ELF64 and rejects PT_INTERP and DT_NEEDED. It does not execute the program.
The archive contains only the controller, licence, README and generic examples.
ADB/curl and an optional tunnel are not bundled or installed.

Before a separately authorized router smoke test, inspect existing read-only
system information: exact board and firmware, `uname -m`, kernel version, package
architecture and libc/userland. Vendor package names are not Rust target triples.
Stock OpenWrt-derived firmware is not interchangeable with upstream OpenWrt SDKs.
No SSH credential, router installation, reboot, firewall, VPN or package change is
required to build this archive.

Proposed qualification budgets (not guarantees): controller idle RSS <20 MiB,
command RSS <32 MiB, idle CPU <1% of one core. Measure controller, transient curl/
ADB client, ADB daemon and any tunnel separately and together on actual firmware.
Host crossbuild and host RSS are not router measurements. An old ADB package may
support only legacy TCP, without modern pairing/TLS. Legacy TCP is not encrypted
by RSA authorization. Smart-socket client access still requires an ADB server.

No procd service is installed. A future service should use a dedicated account,
bounded restart policy and logs, explicit read-only configuration/secret access,
and separate approval for startup persistence. Standard stdio MCP is normally
supervised by its client; a procd service needs a deliberately designed transport
and lifecycle rather than attaching an unattended stdin to this binary.

Sources: [Rust target](https://doc.rust-lang.org/rustc/platform-support/aarch64-unknown-linux-musl.html),
[Rust linkage](https://doc.rust-lang.org/reference/linkage.html),
[ADB wireless design](https://android.googlesource.com/platform/packages/modules/adb/+/refs/heads/main/docs/dev/adb_wifi.md),
[OpenWrt ADB recipe](https://git.openwrt.org/openwrt/openwrt/plain/package/utils/adb/Makefile).
