use super::*;
const DUMP: &str = include_str!("../../../tests/fixtures/tv_input.txt");

#[test]
fn current_user_and_unique_hardware_join_only() {
    let result = parse(DUMP).unwrap();
    assert_eq!(result.current_user, 10);
    assert_eq!(result.inputs.len(), 3);
    assert!(!result.truncated);
    let hdmi = result
        .inputs
        .iter()
        .find(|i| i.id.ends_with("HW42"))
        .unwrap();
    assert_eq!(hdmi.hdmi_port, Some(3));
    assert_eq!(hdmi.connection, InputConnection::Connected);
    let cec = result
        .inputs
        .iter()
        .find(|i| i.id.ends_with("HDMI111"))
        .unwrap();
    assert_eq!(cec.hdmi_port, None);
    assert_eq!(cec.connection, InputConnection::Standby);
    assert_eq!(result.inputs[2].connection, InputConnection::Disconnected);
}
#[test]
fn states_are_not_boolean_and_ids_do_not_infer_ports() {
    let dump = DUMP
        .replace("; state: 0", "; state: 999")
        .replace("    42: org.example/.Input/HW42\n", "");
    let result = parse(&dump).unwrap();
    let hdmi = result
        .inputs
        .iter()
        .find(|i| i.id.ends_with("HW42"))
        .unwrap();
    assert_eq!(hdmi.connection, InputConnection::Unknown);
    assert_eq!(hdmi.hdmi_port, None);
}
#[test]
fn failure_categories_and_complete_empty_map_are_distinct() {
    for (dump, code) in [
        ("", "tv_inputs_dump_empty"),
        (
            "Can't find service: tv_input\n",
            "tv_inputs_service_missing",
        ),
        (
            "Permission Denial: can't dump TvInputManagerService\n",
            "tv_inputs_permission_denied",
        ),
        ("Vendor unknown format\n", "tv_inputs_format_unsupported"),
        ("User Ids (Current user: 10):", "tv_inputs_dump_truncated"),
    ] {
        let error = parse(dump).unwrap_err();
        assert_eq!(error.code, code);
        assert_eq!(error.effect, tv_domain::Effect::None);
    }
    let dump = DUMP
        .lines()
        .filter(|l| !l.contains(": info:"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    assert!(parse(&dump).unwrap().inputs.is_empty());
}
#[test]
fn truncated_sections_and_oversize_are_never_partial_success() {
    for end in [
        "  packageSet:\n    org.example",
        "  mHdmiDeviceList:",
        "  mHdmiInputIdMap:",
        "  mInputMap:",
    ] {
        let prefix = end.split(':').next().unwrap();
        let cut = DUMP.rfind(prefix).unwrap();
        assert_eq!(
            parse(&DUMP[..cut]).unwrap_err().code,
            "tv_inputs_dump_truncated"
        );
    }
    assert_eq!(
        parse(&"x".repeat(MAX_DUMP + 1)).unwrap_err().code,
        "tv_inputs_dump_truncated"
    );
}
#[test]
fn duplicates_injection_and_malformed_evidence_fail_closed() {
    for (from, to) in [
        (
            "    42: org.example/.Input/HW42\n",
            "    42: org.example/.Input/HW42\n    42: org.other/.Input/HW1\n",
        ),
        (
            "    1: org.other/.Input/HW1",
            "    1: org.example/.Input/HW42",
        ),
        ("UserState (0):", "UserState (10):"),
        ("org.example/.Input/HW42", "org.example/.Input/HW42;reboot"),
        ("hdmi_port=3", "hdmi_port=3, hdmi_port=2"),
        (
            "type=9, audio_type=0, audio_addr=, hdmi_port=3",
            "type=8, audio_type=0, audio_addr=, hdmi_port=3",
        ),
        ("; state: 0", "; state: connected"),
        (
            "info: TvInputInfo{id=org.example/.Input/HW42",
            "info: TvInputInfo{id=org.example/.Input/HW99",
        ),
    ] {
        assert!(parse(&DUMP.replace(from, to)).is_err(), "{to}");
    }
    let line = "    TvInputHardwareInfo {id=42, type=9, audio_type=0, audio_addr=, hdmi_port=3, cable_connection_status=0}\n";
    assert!(parse(&DUMP.replace(line, &line.repeat(2))).is_err());
    let line = DUMP.lines().find(|l| l.contains("HW42: info")).unwrap();
    assert!(parse(&DUMP.replace(line, &format!("{line}\n{line}"))).is_err());
}
#[test]
fn list_cap_is_explicit_and_deterministic() {
    let entries = (0..40).map(|i| format!("    org.example/.Input/HW{i}: info: TvInputInfo{{id=org.example/.Input/HW{i}, pkg=org.example, service=org.example.Input}}; state: 0\n")).collect::<String>();
    let dump = DUMP.replacen(
        "UserState (10):\n  inputMap: inputId -> TvInputState\n",
        &format!("UserState (10):\n  inputMap: inputId -> TvInputState\n{entries}"),
        1,
    );
    let result = parse(&dump).unwrap();
    assert_eq!(result.inputs.len(), MAX_INPUTS);
    assert!(result.truncated);
}

#[test]
fn framing_preserves_status_and_refuses_ambiguous_or_incomplete_data() {
    assert!(parse_framed(&format!("{DUMP}\nTV_INPUT_DUMP_EXIT=0\n")).is_ok());
    for status in [1, 124, 255] {
        assert_eq!(
            parse_framed(&format!("{DUMP}\nTV_INPUT_DUMP_EXIT={status}\n"))
                .unwrap_err()
                .code,
            "adb_read_failed"
        );
    }
    for (body, code) in [
        ("Can't find service: tv_input", "tv_inputs_service_missing"),
        (
            "Permission Denial: can't dump TvInputManagerService",
            "tv_inputs_permission_denied",
        ),
    ] {
        for status in [0, 1] {
            assert_eq!(
                parse_framed(&format!("{body}\n\nTV_INPUT_DUMP_EXIT={status}\n"))
                    .unwrap_err()
                    .code,
                code
            );
        }
    }
    for suffix in [
        "",
        "\nTV_INPUT_DUMP_EXIT=",
        "\nTV_INPUT_DUMP_EXIT=0",
        "\nTV_INPUT_DUMP_EXIT=0\nextra\n",
        "\nTV_INPUT_DUMP_EXIT=0\n\nTV_INPUT_DUMP_EXIT=0\n",
        "\nTV_INPUT_DUMP_EXIT=256\n",
    ] {
        assert!(
            parse_framed(&format!("{DUMP}{suffix}")).is_err(),
            "{suffix:?}"
        );
    }
    assert!(parse_framed(&format!(
        "{DUMP}    TV_INPUT_DUMP_EXIT=0\n\nTV_INPUT_DUMP_EXIT=0\n"
    ))
    .is_err());
}

#[test]
fn zero_status_with_dump_failure_diagnostics_is_not_success() {
    for diagnostic in [
        "*** SERVICE 'tv_input' DUMP TIMEOUT (10000ms) EXPIRED ***",
        "Error dumping service info status_t: -1 tv_input",
        "Failed to create pipe to dump service info for tv_input: error",
        "Error in poll while dumping service tv_input : error",
        "Failed to read while dumping service tv_input: error",
        "Failed to write while dumping service tv_input: error",
    ] {
        assert_eq!(
            parse_framed(&format!("{DUMP}{diagnostic}\n\nTV_INPUT_DUMP_EXIT=0\n"))
                .unwrap_err()
                .code,
            "tv_inputs_dump_truncated"
        );
    }
}

#[test]
fn sanitized_panel_excerpt_preserves_seven_inputs_and_three_physical_port_joins() {
    // Two noncontiguous sections from an observed panel dump; only package/class
    // namespaces are anonymized. Negative audio types and hardware IDs are evidence,
    // not production constants. No device command is performed by this regression.
    let body = include_str!("../../../tests/fixtures/tv_input_panel.txt");
    let listing = parse_framed(&format!("{body}\nTV_INPUT_DUMP_EXIT=0\n")).unwrap();
    assert_eq!(listing.current_user, 0);
    assert_eq!(listing.inputs.len(), 7);
    assert!(!listing.truncated);
    for (hardware_id, port) in [(15, 1), (16, 2), (17, 3)] {
        let id = format!("org.example.tvinput/.TvPassThroughService/HW{hardware_id}");
        let input = listing.inputs.iter().find(|i| i.id == id).unwrap();
        assert_eq!(input.hdmi_port, Some(port));
        // Framework availability only, not physical presence or selected/video state.
        assert_eq!(input.connection, InputConnection::Standby);
    }
    let tuner = listing
        .inputs
        .iter()
        .find(|i| i.id.ends_with("/HW2"))
        .unwrap();
    assert_eq!(tuner.connection, InputConnection::Connected);
    assert_eq!(tuner.hdmi_port, None);
    assert_eq!(
        listing
            .inputs
            .iter()
            .filter(|i| i.hdmi_port.is_some())
            .count(),
        3
    );
    assert_eq!(
        listing
            .inputs
            .iter()
            .filter(|i| i.connection == InputConnection::Connected)
            .count(),
        4
    );
}
