//! Target isolation, version preflight and bounded ADB subprocess transport.
use super::{ConfigDevice, Context, Failure, GenericAdb, PortResult};
use crate::process::Invocation;
use std::{collections::BTreeMap, os::unix::fs::MetadataExt};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct ExecutableStamp {
    device: u64,
    inode: u64,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}
pub(super) type ProtocolCache = Option<(Option<ExecutableStamp>, u32)>;

impl GenericAdb {
    pub(super) fn target(&self, alias: &str) -> PortResult<(&str, &str, u16)> {
        match self.config.devices.get(alias) {
            Some(ConfigDevice::Adb {
                endpoint,
                server_host,
                server_port,
                ..
            }) => Ok((endpoint, server_host, *server_port)),
            Some(_) => Err(Failure::unsupported("unsupported")),
            None => Err(Failure::before("unknown_device")),
        }
    }
    pub(super) fn protocol(&self, ctx: &Context<'_>) -> PortResult<u32> {
        ctx.check()?;
        let stamp = std::fs::metadata(&self.config.executables.adb)
            .ok()
            .map(|m| ExecutableStamp {
                device: m.dev(),
                inode: m.ino(),
                size: m.len(),
                modified: (m.mtime(), m.mtime_nsec()),
                changed: (m.ctime(), m.ctime_nsec()),
            });
        let mut cache = self
            .protocol
            .try_lock()
            .map_err(|_| Failure::before("adapter_state_unavailable"))?;
        if let Some((old, version)) = *cache {
            if old == stamp {
                return Ok(version);
            }
        }
        let result = self.process.run(
            Invocation {
                program: self.config.executables.adb.clone(),
                args: vec!["version".into()],
                env: BTreeMap::new(),
                stdin: Vec::new(),
                mutating: false,
            },
            ctx,
        )?;
        let version = std::str::from_utf8(&result.stdout)
            .ok()
            .filter(|_| result.success)
            .and_then(|s| {
                s.lines().find_map(|line| {
                    line.strip_prefix("Android Debug Bridge version 1.0.")?
                        .trim()
                        .parse::<u32>()
                        .ok()
                })
            })
            .ok_or(Failure::before("adb_client_protocol_unavailable"))?;
        ctx.check()?;
        *cache = Some((stamp, version));
        Ok(version)
    }
    /// Only internal closed input operations use this helper. Dynamic text is a
    /// positional argument, never shell source; stderr shares the bounded capture.
    pub(super) fn inject_input(
        &self,
        alias: &str,
        args: &[&str],
        diagnostic_code: &'static str,
        ctx: &Context<'_>,
    ) -> PortResult<()> {
        let mut command = vec!["sh", "-c", "exec input \"$@\" 2>&1", "tv-control-input"];
        command.extend_from_slice(args);
        let output = self.shell(alias, &command, &[], true, ctx)?;
        if !output.trim().is_empty() {
            return Err(Failure::uncertain(diagnostic_code));
        }
        Ok(())
    }
    pub(super) fn shell(
        &self,
        alias: &str,
        args: &[&str],
        input: &[u8],
        mutating: bool,
        ctx: &Context<'_>,
    ) -> PortResult<String> {
        let output = self.shell_bytes(
            alias,
            args,
            input,
            mutating,
            self.config.preferences.max_output_bytes,
            ctx,
        )?;
        String::from_utf8(output).map_err(|_| {
            if mutating {
                Failure::uncertain("adb_command_failed")
            } else {
                Failure::before("adb_read_failed")
            }
        })
    }
    pub(super) fn shell_bytes(
        &self,
        alias: &str,
        args: &[&str],
        input: &[u8],
        mutating: bool,
        limit: usize,
        ctx: &Context<'_>,
    ) -> PortResult<Vec<u8>> {
        self.command_bytes(alias, "shell", args, input, mutating, limit, ctx)
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn command_bytes(
        &self,
        alias: &str,
        transport: &str,
        args: &[&str],
        input: &[u8],
        mutating: bool,
        limit: usize,
        ctx: &Context<'_>,
    ) -> PortResult<Vec<u8>> {
        let (endpoint, host, port) = self.target(alias)?;
        let protocol = self.protocol(ctx)?;
        // Probe the existing server on every call. No manager-capable client is
        // allowed to contact an incompatible server, even when its version is cached.
        if self.probe.version(host, port, ctx)? != protocol {
            return Err(Failure::before("adb_server_protocol_mismatch"));
        }
        ctx.check()?;
        let command = args
            .iter()
            .map(|s| format!("'{}'", s.replace('\'', "'\\''")))
            .collect::<Vec<_>>()
            .join(" ");
        let result = self.process.run(
            Invocation {
                program: self.config.executables.adb.clone(),
                args: vec![
                    "-H".into(),
                    host.into(),
                    "-P".into(),
                    port.to_string().into(),
                    "-s".into(),
                    endpoint.into(),
                    transport.into(),
                    command.into(),
                ],
                env: BTreeMap::from([
                    ("ADB_MDNS".into(), "0".into()),
                    ("ADB_USB".into(), "0".into()),
                ]),
                stdin: input.to_vec(),
                mutating,
            },
            ctx,
        )?;
        let error = || {
            if mutating {
                Failure::uncertain("adb_command_failed")
            } else {
                Failure::before("adb_read_failed")
            }
        };
        if !result.success {
            return Err(error());
        }
        if result.stdout.len() > limit {
            return Err(if mutating {
                Failure::uncertain("adb_output_limit")
            } else {
                Failure::before("adb_output_limit")
            });
        }
        let output = result.stdout;
        ctx.check()
            .map_err(|e| if mutating { e.after_dispatch() } else { e })?;
        Ok(output)
    }
}
