use std::{
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use tv_adapters::setup;
fn directory() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "tv-setup-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&dir).unwrap();
    dir
}
#[test]
fn private_atomic_publish_refuses_overwrite_and_supports_identical_resume() {
    let dir = directory();
    let target = dir.join("screen.local.json");
    assert_eq!(
        setup::publish(&target, b"example", false),
        Ok("configuration_saved")
    );
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        setup::publish(&target, b"changed", false),
        Err("output_exists_use_resume_or_new_path")
    );
    assert_eq!(
        setup::publish(&target, b"example", true),
        Ok("already_configured")
    );
    assert_eq!(
        setup::publish(&target, b"changed", true),
        Err("existing_config_differs_use_new_output")
    );
    assert_eq!(fs::read(&target).unwrap(), b"example");
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
    fs::remove_dir_all(dir).unwrap();
}
#[test]
fn symlink_destination_never_overwrites_target() {
    let dir = directory();
    let real = dir.join("real");
    fs::write(&real, b"keep").unwrap();
    let link = dir.join("link");
    symlink(&real, &link).unwrap();
    assert_eq!(
        setup::publish(&link, b"changed", false),
        Err("output_exists_use_resume_or_new_path")
    );
    assert_eq!(
        setup::publish(&link, b"changed", true),
        Err("existing_config_not_private")
    );
    assert_eq!(fs::read(real).unwrap(), b"keep");
    fs::remove_dir_all(dir).unwrap();
}
#[test]
fn concurrent_publish_has_one_winner_without_partial_file() {
    let dir = directory();
    let target = dir.join("screen.local.json");
    let handles: Vec<_> = (0..8)
        .map(|n| {
            let path = target.clone();
            std::thread::spawn(move || {
                setup::publish(&path, format!("complete-{n}").as_bytes(), false)
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert!(String::from_utf8(fs::read(&target).unwrap())
        .unwrap()
        .starts_with("complete-"));
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
    fs::remove_dir_all(dir).unwrap();
}
#[test]
fn command_quoting_preserves_spaces_and_apostrophes() {
    assert_eq!(
        setup::shell_quote("/opt/TV app's/bin"),
        Ok("'/opt/TV app'\\''s/bin'".into())
    );
    assert_eq!(
        setup::shell_quote("bad\ncommand"),
        Err("command_contains_control_character")
    );
}
