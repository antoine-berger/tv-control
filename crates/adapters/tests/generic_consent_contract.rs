use std::{fs, os::unix::fs::PermissionsExt};
use tv_adapters::{config::Config, setup::grant_generic_control};

#[test]
fn legacy_configuration_requires_an_explicit_category_grant() {
    let original = include_bytes!("../../../examples/config.json");
    assert!(!Config::from_json(original).unwrap().generic_adb_control);
    let root = std::env::temp_dir().join(format!("tv-generic-consent-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let path = root.join("config.json");
    fs::write(&path, original).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        grant_generic_control(&path, original, false),
        Err("generic_control_not_authorized")
    );
    assert_eq!(fs::read(&path).unwrap(), original);
    let granted = grant_generic_control(&path, original, true).unwrap();
    assert!(Config::from_json(&granted).unwrap().generic_adb_control);
    assert_eq!(fs::read(&path).unwrap(), granted);
    assert!(grant_generic_control(&path, original, true).is_err());
    assert_eq!(
        grant_generic_control(&path, &granted, true).unwrap(),
        granted
    );
    fs::remove_dir_all(root).unwrap();
}
