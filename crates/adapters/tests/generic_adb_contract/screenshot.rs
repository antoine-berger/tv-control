use super::*;

const PNG: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0,
    0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 96, 96, 96, 248, 15, 0, 1,
    4, 1, 0, 95, 229, 195, 75, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];
#[test]
fn screenshots_are_explicit_bounded_and_binary_without_pty() {
    let (adb, process, _) = adapter(false, &[VERSION]);
    process.replies.lock().unwrap().push_back(Ok(PNG.to_vec()));
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    let result = adb.screenshot("screen", &ctx(&clock, &flag)).unwrap();
    assert_eq!((result.width, result.height), (1, 1));
    assert_eq!(result.png, PNG);
    let calls = process.calls.lock().unwrap();
    assert_eq!(calls[1].args[calls[1].args.len() - 2], "exec-out");
    assert_eq!(calls[1].args.last().unwrap(), "'screencap' '-p'");
    assert!(!calls[1].mutating);
}
#[test]
fn screenshots_reject_missing_oversized_and_corrupt_png() {
    let mut corrupt = PNG.to_vec();
    corrupt[20] ^= 1;
    for bad in [
        Vec::new(),
        vec![0; 8 * 1024 * 1024 + 1],
        b"protected screen".to_vec(),
        corrupt,
        PNG[..PNG.len() - 12].to_vec(),
    ] {
        let (adb, process, _) = adapter(false, &[VERSION]);
        process.replies.lock().unwrap().push_back(Ok(bad));
        let clock = TestClock::default();
        let flag = CancelFlag::default();
        assert!(adb.screenshot("screen", &ctx(&clock, &flag)).is_err());
    }
}
#[test]
fn screenshot_above_one_mib_is_accepted_within_the_explicit_bound() {
    let mut png = PNG[..PNG.len() - 12].to_vec();
    let mut data = b"Comment\0".to_vec();
    data.extend(vec![b'x'; 2 * 1024 * 1024]);
    png.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = png.len();
    png.extend_from_slice(b"tEXt");
    png.extend_from_slice(&data);
    let mut crc = u32::MAX;
    for byte in &png[start..] {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320u32 & 0u32.wrapping_sub(crc & 1));
        }
    }
    png.extend_from_slice(&(!crc).to_be_bytes());
    png.extend_from_slice(&PNG[PNG.len() - 12..]);
    let (adb, process, _) = adapter(false, &[VERSION]);
    process.replies.lock().unwrap().push_back(Ok(png.clone()));
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    assert_eq!(
        adb.screenshot("screen", &ctx(&clock, &flag)).unwrap().png,
        png
    );
}

#[test]
fn screenshot_size_limit_is_not_reported_as_transport_failure() {
    let (adb, process, _) = adapter(false, &[VERSION]);
    process
        .replies
        .lock()
        .unwrap()
        .push_back(Ok(vec![0; 8 * 1024 * 1024 + 1]));
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    assert_eq!(
        adb.screenshot("screen", &ctx(&clock, &flag))
            .unwrap_err()
            .code,
        "screenshot_too_large"
    );
}
