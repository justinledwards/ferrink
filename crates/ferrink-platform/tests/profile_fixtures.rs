use ferrink_platform::{DeviceProfile, ProbeReport, ProfileEvaluationStatus, ProfileParseError};

const KOA3_REPORT: &str = include_str!("fixtures/probe-reference-portrait.json");
const PW1_REPORT: &str = include_str!("fixtures/probe-reference-landscape.json");
const KOA3_PROFILE: &str = include_str!("../../../device-profiles/reference-portrait.toml");
const PW1_PROFILE: &str = include_str!("../../../device-profiles/reference-landscape.toml");

#[test]
fn known_koa3_shape_matches_only_the_reviewed_identity() {
    let report = ProbeReport::from_json(KOA3_REPORT).unwrap();
    report.validate().unwrap();
    let profile = DeviceProfile::from_toml(KOA3_PROFILE).unwrap();
    let evaluation = profile.evaluate(&report);
    assert_eq!(evaluation.status, ProfileEvaluationStatus::Match);
    assert!(evaluation.missing.is_empty());
    assert!(evaluation.mismatches.is_empty());
}

#[test]
fn observed_koa3_axes_contain_only_advertised_touch_codes() {
    let report = ProbeReport::from_json(KOA3_REPORT).unwrap();
    let touch = report
        .inputs
        .iter()
        .find(|input| input.name.as_deref() == Some("cyttsp5_mt"))
        .expect("observed KOA3 fixture must contain its touchscreen");
    let codes = touch.axes.iter().map(|axis| axis.code).collect::<Vec<_>>();

    assert_eq!(codes, vec![47, 53, 54, 57, 58]);
    assert!(report.warnings.is_empty());
}

#[test]
fn known_pw1_shape_matches_only_the_reviewed_identity() {
    let report = ProbeReport::from_json(PW1_REPORT).unwrap();
    report.validate().unwrap();
    assert_eq!(report.framebuffers[0].visible_width, 758);
    assert_eq!(report.framebuffers[0].visible_height, 1024);

    let profile = DeviceProfile::from_toml(PW1_PROFILE).unwrap();
    let evaluation = profile.evaluate(&report);
    assert_eq!(evaluation.status, ProfileEvaluationStatus::Match);
    assert!(evaluation.missing.is_empty());
    assert!(evaluation.mismatches.is_empty());
}

#[test]
fn pw1_geometry_without_identity_fails_closed() {
    let mut report = ProbeReport::from_json(PW1_REPORT).unwrap();
    report.identity.serial_prefix = None;
    let profile = DeviceProfile::from_toml(PW1_PROFILE).unwrap();
    let evaluation = profile.evaluate(&report);
    assert_eq!(
        evaluation.status,
        ProfileEvaluationStatus::MissingCapabilities
    );
    assert!(
        evaluation
            .missing
            .contains(&"identity.serial_prefix".to_owned())
    );
}

#[test]
fn observed_pw1_axes_contain_only_advertised_touch_codes() {
    let report = ProbeReport::from_json(PW1_REPORT).unwrap();
    let touch = report
        .inputs
        .iter()
        .find(|input| input.name.as_deref() == Some("cyttsp"))
        .expect("observed PW1 fixture must contain its touchscreen");
    let codes = touch.axes.iter().map(|axis| axis.code).collect::<Vec<_>>();

    assert_eq!(codes, vec![0, 1, 47, 53, 54, 57]);
    assert!(report.warnings.is_empty());
}

#[test]
fn missing_touch_capabilities_fail_closed() {
    let mut report = ProbeReport::from_json(KOA3_REPORT).unwrap();
    report.inputs.clear();
    let profile = DeviceProfile::from_toml(KOA3_PROFILE).unwrap();
    let evaluation = profile.evaluate(&report);
    assert_eq!(
        evaluation.status,
        ProfileEvaluationStatus::MissingCapabilities
    );
    assert!(
        evaluation
            .missing
            .contains(&"input.abs_mt_position_x_y".to_owned())
    );
}

#[test]
fn malformed_reports_and_profiles_are_rejected() {
    assert!(ProbeReport::from_json("{\"schema_version\":1}").is_err());
    assert!(ProbeReport::from_json("{ definitely not json }").is_err());

    let malformed = KOA3_PROFILE.replace("width = 1264", "width = \"wide\"");
    assert!(matches!(
        DeviceProfile::from_toml(&malformed),
        Err(ProfileParseError::Toml(_))
    ));
    let wrong_version = KOA3_PROFILE.replace("schema_version = 1", "schema_version = 99");
    assert!(matches!(
        DeviceProfile::from_toml(&wrong_version),
        Err(ProfileParseError::Validation(_))
    ));
}

#[test]
fn runtime_extension_is_optional_to_parse_but_strict_when_present() {
    let profile = DeviceProfile::from_toml(KOA3_PROFILE).unwrap();
    assert!(profile.runtime.is_some());

    let mut historical = profile.clone();
    historical.runtime = None;
    historical.validate().unwrap();

    let duplicate_axes = KOA3_PROFILE.replace("y_axis_code = 54", "y_axis_code = 53");
    assert!(matches!(
        DeviceProfile::from_toml(&duplicate_axes),
        Err(ProfileParseError::Validation(_))
    ));
    let wrong_abi = KOA3_PROFILE.replace("update_abi = \"zelda88\"", "update_abi = \"rex80\"");
    assert!(matches!(
        DeviceProfile::from_toml(&wrong_abi),
        Err(ProfileParseError::Validation(_))
    ));
}

#[test]
fn fixture_identity_is_redacted_and_unredacted_reports_are_invalid() {
    let mut report = ProbeReport::from_json(KOA3_REPORT).unwrap();
    assert_eq!(report.identity.serial_prefix.as_deref(), Some("TEST"));
    assert_eq!(
        report.identity.serial_redacted.as_deref(),
        Some("TEST…REDACTED")
    );
    report.identity.serial_redacted = Some("TEST123456789012".to_owned());
    assert!(report.validate().is_err());

    let mut report = ProbeReport::from_json(KOA3_REPORT).unwrap();
    report.redaction.enabled = false;
    assert!(report.validate().is_err());

    let report = ProbeReport::from_json(PW1_REPORT).unwrap();
    assert_eq!(report.identity.serial_prefix.as_deref(), Some("DEMO"));
    assert_eq!(
        report.identity.serial_redacted.as_deref(),
        Some("DEMO…REDACTED")
    );
}
