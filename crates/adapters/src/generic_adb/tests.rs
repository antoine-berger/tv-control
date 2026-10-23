use super::*;
const UI: &str = "<?xml version=\"1.0\"?><hierarchy rotation=\"0\"><node index=\"0\" text=\"Search &amp; play\" class=\"android.widget.Button\" content-desc=\"\" enabled=\"true\" clickable=\"true\" focused=\"true\" bounds=\"[10,20][110,60]\" /></hierarchy>UI hierchary dumped to: /dev/tty";
#[test]
fn ui_is_bounded_and_decodes_entities() {
    let view = parse_ui("living", 1, UI).unwrap();
    assert_eq!(view.elements[0].text, "Search & play");
    assert_eq!(view.elements[0].bounds, [10, 20, 110, 60]);
    assert_ne!(view, parse_ui("bedroom", 1, UI).unwrap());
    assert_ne!(view, parse_ui("living", 2, UI).unwrap());
}
#[test]
fn rejects_hostile_and_malformed_xml() {
    for bad in [
        UI.replace("Search &amp; play", "&external;"),
        UI.replace("index=\"0\"", "index=\"0\" index=\"1\""),
        UI.replace("[10,20][110,60]", "[10,20][0,0]"),
        format!("<!DOCTYPE foo>{UI}"),
        UI.replace("</hierarchy>", "</node></hierarchy>"),
    ] {
        assert!(parse_ui("living", 1, &bad).is_err(), "{bad}");
    }
}
#[test]
fn password_nodes_are_omitted() {
    assert!(parse_ui(
        "living",
        1,
        &UI.replace("index=\"0\"", "password=\"true\" index=\"0\"")
    )
    .unwrap()
    .elements
    .is_empty());
}
#[test]
fn activity_metadata_preserves_real_names_only() {
    let raw = "1 activities found:\n  Activity #0:\n    ActivityInfo:\n      name=com.example.tv.Main\n      packageName=com.example.tv\n      labelRes=0x7f010001 nonLocalizedLabel=Example TV icon=0x0\n      ApplicationInfo:\n        name=Ignored\n        packageName=com.example.tv\n";
    let app = parse_activities(raw).unwrap().remove(0);
    assert_eq!(app.label.as_deref(), Some("Example TV"));
    assert_eq!(app.component, "com.example.tv/com.example.tv.Main");
    assert!(
        parse_activities(&raw.replace("com.example.tv.Main", "com.example.tv.Main;reboot"))
            .is_err()
    );
    assert_eq!(
        parse_activities(&raw.replace("Example TV", "null")).unwrap()[0].label,
        None
    );
}
#[test]
fn resolver_metadata_cannot_bleed_into_previous_app() {
    let raw = "2 activities found:\n  Activity #0:\n    ActivityInfo:\n      name=.Main\n      packageName=org.example.first\n      ApplicationInfo:\n        packageName=org.example.first\n  Activity #1:\n    labelRes=0x0 nonLocalizedLabel=Second App icon=0x0\n    ActivityInfo:\n      name=.Main\n      packageName=org.example.second\n      ApplicationInfo:\n        packageName=org.example.second\n";
    let apps = parse_activities(raw).unwrap();
    assert_eq!(apps[0].label, None);
    assert_eq!(apps[1].label.as_deref(), Some("Second App"));
    assert!(parse_activities(&raw.replace("2 activities", "3 activities")).is_err());
}
#[test]
fn duplicate_media_tag_is_rejected() {
    assert_eq!(
        session_tag(
            "tag=x, package=org.example.one\ntag=x, package=org.example.two",
            "org.example.one"
        )
        .unwrap_err()
        .code,
        "media_tag_ambiguous"
    );
}
