use std::num::NonZeroU32;

use ferrink_platform::{
    DeviceProfile, Endianness, InputEventDecoder, LogicalTouchPhase, ProbeReport,
    ResolvedRuntimeDevice, TouchTracker,
};

const KOA3_REPORT: &str = include_str!("fixtures/probe-reference-portrait.json");
const PW1_REPORT: &str = include_str!("fixtures/probe-reference-landscape.json");
const KOA3_PROFILE: &str = include_str!("../../../device-profiles/reference-portrait.toml");

fn decoder_from_report(report: &str) -> InputEventDecoder {
    let report = ProbeReport::from_json(report).unwrap();
    let executable = report.system.executable_abi.unwrap();
    assert_eq!(executable.endianness, Endianness::Little);
    InputEventDecoder::try_new(
        report.system.input_event_abi,
        executable.endianness,
        NonZeroU32::new(16).unwrap(),
    )
    .unwrap()
}

#[test]
fn exact_koa3_fixture_selects_the_reviewed_sixteen_byte_input_event_layout() {
    let decoder = decoder_from_report(KOA3_REPORT);
    assert_eq!(decoder.record_bytes(), 16);
}

#[test]
fn exact_pw1_fixture_selects_the_reviewed_sixteen_byte_input_event_layout() {
    let decoder = decoder_from_report(PW1_REPORT);
    assert_eq!(decoder.record_bytes(), 16);
}

#[test]
fn exact_koa3_decoded_records_feed_the_incremental_touch_tracker() {
    let profile = DeviceProfile::from_toml(KOA3_PROFILE).unwrap();
    let report = ProbeReport::from_json(KOA3_REPORT).unwrap();
    let device = ResolvedRuntimeDevice::resolve(&profile, &report).unwrap();
    let mut decoder = InputEventDecoder::try_new(
        device.input_event_abi(),
        device.input_endianness(),
        NonZeroU32::new(4).unwrap(),
    )
    .unwrap();
    let mut tracker = TouchTracker::new(device.input_transform());

    let mut bytes = Vec::new();
    bytes.extend_from_slice(&record32(1, 0, 3, 53, 100));
    bytes.extend_from_slice(&record32(1, 1, 3, 54, 200));
    bytes.extend_from_slice(&record32(1, 2, 3, 57, 7));
    bytes.extend_from_slice(&record32(1, 3, 0, 0, 0));

    let mut contacts = Vec::new();
    for chunk in [&bytes[..5], &bytes[5..22], &bytes[22..]] {
        for event in decoder.push(chunk).unwrap() {
            if let Some(contact) = tracker
                .push(event.event_type, event.code, event.value)
                .unwrap()
            {
                contacts.push(contact);
            }
        }
    }

    assert_eq!(decoder.decoded_records(), 4);
    assert_eq!(decoder.partial_bytes(), 0);
    assert_eq!(contacts.len(), 1);
    assert_eq!(contacts[0].phase, LogicalTouchPhase::Pressed);
    assert_eq!(contacts[0].point.x, 100);
    assert_eq!(contacts[0].point.y, 200);
}

fn record32(seconds: i32, microseconds: i32, event_type: u16, code: u16, value: i32) -> [u8; 16] {
    let mut record = [0; 16];
    record[0..4].copy_from_slice(&seconds.to_le_bytes());
    record[4..8].copy_from_slice(&microseconds.to_le_bytes());
    record[8..10].copy_from_slice(&event_type.to_le_bytes());
    record[10..12].copy_from_slice(&code.to_le_bytes());
    record[12..16].copy_from_slice(&value.to_le_bytes());
    record
}
