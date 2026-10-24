use super::*;
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

struct ShellInput {
    root: PathBuf,
    calls: Mutex<Vec<Invocation>>,
    ui: String,
}
impl ShellInput {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "tv-input-diagnostics-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let input = root.join("input");
        fs::write(&input, "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$INPUT_LOG\"\nprintf 'Error: private diagnostic\\n' >&2\nexit 0\n").unwrap();
        fs::set_permissions(input, fs::Permissions::from_mode(0o700)).unwrap();
        Self { root, calls: Mutex::new(vec![]), ui: "<hierarchy><node resource-id=\"fixture:id/query\" package=\"org.example.player\" class=\"android.widget.EditText\" text=\"\" enabled=\"true\" focused=\"true\" clickable=\"true\" bounds=\"[10,20][110,60]\" /></hierarchy>".into() }
    }
}
impl Drop for ShellInput {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
impl Process for ShellInput {
    fn run(&self, call: Invocation, context: &Context<'_>) -> PortResult<Output> {
        let remote = call.args.last().unwrap().to_string_lossy().into_owned();
        let mutation = call.mutating;
        self.calls.lock().unwrap().push(call);
        if mutation {
            return tv_adapters::process::BoundedProcess {
                max_output: 4096,
                poll_ms: 1,
            }
            .run(
                Invocation {
                    program: "/bin/sh".into(),
                    args: vec!["-c".into(), remote.into()],
                    env: [
                        (
                            "PATH".into(),
                            format!("{}:/usr/bin:/bin", self.root.display()).into(),
                        ),
                        ("INPUT_LOG".into(), self.root.join("args").into_os_string()),
                    ]
                    .into_iter()
                    .collect(),
                    stdin: vec![],
                    mutating: true,
                },
                context,
            );
        }
        let output = if remote == "version" {
            VERSION
        } else if remote.contains("getprop") {
            "35"
        } else if remote.contains("uiautomator") {
            &self.ui
        } else if remote.contains("activity") {
            FOREGROUND
        } else {
            "keyevent [--longpress] <key code number or name> ...\n"
        };
        Ok(Output {
            stdout: output.as_bytes().to_vec(),
            success: true,
        })
    }
}

#[test]
fn all_input_operations_reject_zero_exit_stderr_without_revalidation_or_retry() {
    for action in [
        Intent::Wake,
        Intent::Standby,
        Intent::LongSelect,
        Intent::Navigate {
            keys: vec![Key::Down, Key::Select],
        },
        Intent::RemoteMedia {
            command: RemoteMediaCommand::Stop,
        },
    ] {
        let process = Arc::new(ShellInput::new());
        let adb = GenericAdb::new(
            Arc::new(cfg(true)),
            process.clone(),
            Arc::new(Probe(AtomicU32::new(41))),
        );
        let error = adb
            .apply(
                "screen",
                &action,
                None,
                &ctx(&TestClock::default(), &CancelFlag::default()),
            )
            .unwrap_err();
        assert_eq!(error.status(), tv_domain::Status::OutcomeUnknown);
        let calls = process.calls.lock().unwrap();
        assert!(
            calls.last().unwrap().mutating,
            "diagnostic must stop before post-read"
        );
        assert_eq!(calls.iter().filter(|call| call.mutating).count(), 1);
    }
}

#[test]
fn tap_and_hostile_text_keep_literal_arguments_and_consume_refs_on_diagnostics() {
    for text in [
        None,
        Some("a b's $HOME `id`; & | \\ $(touch SHOULD_NOT_EXIST)"),
    ] {
        let process = Arc::new(ShellInput::new());
        let adb = GenericAdb::new(
            Arc::new(cfg(true)),
            process.clone(),
            Arc::new(Probe(AtomicU32::new(41))),
        );
        let clock = TestClock::default();
        let cancel = CancelFlag::default();
        let context = ctx(&clock, &cancel);
        let view = adb.ui("screen", &context).unwrap();
        let reference = view.elements[0].reference.clone();
        let action = match text {
            Some(text) => Intent::TypeText {
                reference,
                text: text.into(),
            },
            None => Intent::Activate { reference },
        };
        let error = adb.apply("screen", &action, None, &context).unwrap_err();
        assert_eq!(error.status(), tv_domain::Status::OutcomeUnknown);
        let expected = text
            .map(|t| format!("text\n{t}\n"))
            .unwrap_or("tap\n60\n40\n".into());
        assert_eq!(
            fs::read_to_string(process.root.join("args")).unwrap(),
            expected
        );
        let before = process.calls.lock().unwrap().len();
        assert_eq!(
            adb.apply("screen", &action, None, &context)
                .unwrap_err()
                .code,
            "ui_reference_unknown"
        );
        assert_eq!(process.calls.lock().unwrap().len(), before);
    }
}
