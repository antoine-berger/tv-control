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
        UI.replace("[10,20][110,60]", "[10,20][oops,0]"),
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
            "Sessions:\ntag=x, package=org.example.one\ntag=x, package=org.example.two",
            "org.example.one"
        )
        .unwrap_err()
        .code,
        "media_tag_ambiguous"
    );
}

#[test]
fn clipped_ui_nodes_do_not_hide_visible_siblings() {
    let clipped = UI.replace("[10,20][110,60]", "[0,0][0,0]");
    assert!(parse_ui("living", 1, &clipped).unwrap().elements.is_empty());
    let node = "<node text=\"Hidden\" enabled=\"true\" focusable=\"true\" bounds=\"[0,0][0,0]\" />";
    let mixed = UI.replace("</hierarchy>", &format!("{node}</hierarchy>"));
    let view = parse_ui("living", 1, &mixed).unwrap();
    assert_eq!(view.elements.len(), 1);
    assert_eq!(view.elements[0].text, "Search & play");
}

#[test]
fn labels_without_actions_remain_visible_but_not_actionable() {
    let raw = UI.replace(
        "clickable=\"true\" focused=\"true\"",
        "clickable=\"false\" focused=\"false\"",
    );
    let view = parse_ui("living", 1, &raw).unwrap();
    assert_eq!(view.elements.len(), 1);
    assert!(!view.elements[0].actionable);
    assert!(!view.elements[0].editable);
    assert_eq!(view.focused_count, 0);
}

#[test]
fn bounded_snapshot_reports_omissions_without_hiding_focus_ambiguity() {
    let node = "<node text=\"Title\" class=\"android.widget.TextView\" focused=\"true\" bounds=\"[1,1][20,20]\"/>";
    let raw = format!(
        "<hierarchy>{}</hierarchy>",
        node.repeat(MAX_UI_ELEMENTS + 2)
    );
    let view = parse_ui("living", 1, &raw).unwrap();
    assert_eq!(view.elements.len(), MAX_UI_ELEMENTS);
    assert_eq!(view.focused_count, MAX_UI_ELEMENTS + 2);
    assert_eq!(view.truncation, vec![UiTruncation::Elements]);
    let raw = UI.replace("Search &amp; play", &"é".repeat(257));
    let view = parse_ui("living", 1, &raw).unwrap();
    assert_eq!(view.elements[0].text.chars().count(), 256);
    assert_eq!(view.truncation, vec![UiTruncation::Text]);
    // Still parse the entire document: truncation cannot conceal malformed XML.
    assert!(parse_ui(
        "living",
        1,
        &format!(
            "<hierarchy>{}<node broken/></hierarchy>",
            node.repeat(MAX_UI_ELEMENTS + 2)
        )
    )
    .is_err());
}

#[test]
fn password_focus_is_counted_without_exposing_text() {
    let secret = UI.replace("index=\"0\"", "password=\"true\" index=\"0\"");
    let view = parse_ui("living", 1, &secret).unwrap();
    assert_eq!(view.focused_count, 1);
    assert!(view.elements.is_empty());
}

#[test]
fn visible_text_takes_precedence_over_static_accessibility_description() {
    let raw = UI.replace("content-desc=\"\"", "content-desc=\"Search field\"");
    let view = parse_ui("living", 1, &raw).unwrap();
    assert_eq!(view.elements[0].text, "Search & play");
    let blank = raw.replace("text=\"Search &amp; play\"", "text=\"\"");
    assert_eq!(
        parse_ui("living", 1, &blank).unwrap().elements[0].text,
        "Search field"
    );
}
