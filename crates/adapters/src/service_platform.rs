//! Typed per-user launchd boundary. It does not establish ownership or activate itself.
//!
//! The caller must validate its private installation receipt and unchanged artifact
//! before calling a lifecycle method. These commands never inspect or adopt a job
//! by parsing launchctl's human-readable output.
use crate::process::{Invocation, Process};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
};
use tv_application::Context;
use tv_domain::{Failure, PortResult};

const MAX_PATH_BYTES: usize = 4096;
const MAX_LABEL_BYTES: usize = 128;
const MAX_PLIST_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceSpec {
    pub label: String,
    pub executable: PathBuf,
    pub configuration: PathBuf,
    pub working_directory: PathBuf,
    pub artifact: PathBuf,
    pub uid: u32,
}

fn valid_path(path: &Path) -> bool {
    path.is_absolute()
        && path.to_str().is_some_and(|text| {
            text.len() <= MAX_PATH_BYTES
                && !text.chars().any(char::is_control)
                && !text.split('/').any(|part| matches!(part, "." | ".."))
        })
}

impl ServiceSpec {
    pub fn validate(&self) -> PortResult<()> {
        if self.uid == 0 || self.uid == u32::MAX {
            return Err(Failure::unsupported("service_user_domain_unsupported"));
        }
        if self.label.is_empty()
            || self.label.len() > MAX_LABEL_BYTES
            || !self.label.as_bytes()[0].is_ascii_alphanumeric()
            || !self
                .label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            || ![
                &self.executable,
                &self.configuration,
                &self.working_directory,
                &self.artifact,
            ]
            .into_iter()
            .all(|path| valid_path(path))
        {
            return Err(Failure::before("invalid_service_specification"));
        }
        Ok(())
    }

    fn domain(&self) -> String {
        format!("gui/{}", self.uid)
    }
    fn target(&self) -> String {
        format!("{}/{}", self.domain(), self.label)
    }

    /// Render only. No filesystem, subprocess, launchd or runtime operation occurs.
    /// A caller may statically validate the returned bytes in a temporary file.
    pub fn render(&self) -> PortResult<Vec<u8>> {
        self.validate()?;
        let text = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\"><dict>\n\
             <key>Label</key><string>{}</string>\n\
             <key>ProgramArguments</key><array><string>{}</string><string>service</string><string>run</string><string>{}</string></array>\n\
             <key>WorkingDirectory</key><string>{}</string>\n\
             <key>RunAtLoad</key><true/>\n\
             <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>\n\
             <key>ThrottleInterval</key><integer>30</integer>\n\
             <key>ExitTimeOut</key><integer>20</integer>\n\
             <key>Umask</key><integer>63</integer>\n\
             <key>StandardOutPath</key><string>/dev/null</string>\n\
             <key>StandardErrorPath</key><string>/dev/null</string>\n\
             </dict></plist>\n",
            xml(&self.label), xml(self.executable.to_str().unwrap()),
            xml(self.configuration.to_str().unwrap()), xml(self.working_directory.to_str().unwrap())
        );
        if text.len() > MAX_PLIST_BYTES {
            return Err(Failure::before("service_artifact_limit"));
        }
        Ok(text.into_bytes())
    }
}

fn xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        out.push_str(match ch {
            '&' => "&amp;",
            '<' => "&lt;",
            '>' => "&gt;",
            '\'' => "&apos;",
            '"' => "&quot;",
            _ => {
                out.push(ch);
                continue;
            }
        });
    }
    out
}

/// The composition root selects this adapter only on macOS. Injectable Process
/// permits static/fixture qualification on other hosts without invoking launchctl.
pub struct Launchd<'a> {
    pub process: &'a dyn Process,
}
enum Command {
    Bootstrap,
    Kickstart,
    Bootout,
}
impl Launchd<'_> {
    pub fn bootstrap(&self, spec: &ServiceSpec, ctx: &Context<'_>) -> PortResult<()> {
        self.call(spec, Command::Bootstrap, ctx)
    }
    pub fn kickstart(&self, spec: &ServiceSpec, ctx: &Context<'_>) -> PortResult<()> {
        self.call(spec, Command::Kickstart, ctx)
    }
    pub fn bootout(&self, spec: &ServiceSpec, ctx: &Context<'_>) -> PortResult<()> {
        self.call(spec, Command::Bootout, ctx)
    }
    fn call(&self, spec: &ServiceSpec, command: Command, ctx: &Context<'_>) -> PortResult<()> {
        ctx.check()?;
        spec.validate()?;
        let args: Vec<OsString> = match command {
            Command::Bootstrap => vec![
                "bootstrap".into(),
                spec.domain().into(),
                spec.artifact.as_os_str().into(),
            ],
            Command::Kickstart => vec!["kickstart".into(), spec.target().into()],
            Command::Bootout => vec!["bootout".into(), spec.target().into()],
        };
        let output = self.process.run(
            Invocation {
                program: "/bin/launchctl".into(),
                args,
                env: BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())]),
                stdin: Vec::new(),
                mutating: true,
            },
            ctx,
        )?;
        // A cancellation/deadline observed after dispatch must not become success.
        ctx.check().map_err(Failure::after_dispatch)?;
        if !output.success {
            return Err(Failure::uncertain("service_platform_command_failed"));
        }
        // The exit acknowledgement says only that this platform operation returned
        // successfully. It is not proof of controller readiness or physical effect.
        Ok(())
    }
}
