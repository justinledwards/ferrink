use ferrink_platform::{
    DeviceProfile, DisplayPoint, DisplayUpdateAbiKind, ProbeReport, ProfileEvaluationStatus,
    QuarterTurn, RawPoint, RefreshMode, ResolvedRuntimeDevice, RuntimeAuthority,
    RuntimeResolutionError, StockRepaintMechanism,
};

const KOA3_REPORT: &str = include_str!("fixtures/probe-reference-portrait.json");
const PW1_REPORT: &str = include_str!("fixtures/probe-reference-landscape.json");
const KOA3_PROFILE: &str = include_str!("../../../device-profiles/reference-portrait.toml");
const PW1_PROFILE: &str = include_str!("../../../device-profiles/reference-landscape.toml");

fn resolve(profile: &str, report: &str) -> ResolvedRuntimeDevice {
    let profile = DeviceProfile::from_toml(profile).unwrap();
    let report = ProbeReport::from_json(report).unwrap();
    ResolvedRuntimeDevice::resolve(&profile, &report).unwrap()
}

#[test]
fn exact_koa3_pair_resolves_foreground_display_input_and_refresh() {
    let device = resolve(KOA3_PROFILE, KOA3_REPORT);

    assert_eq!(device.profile_id(), "reference-portrait");
    assert_eq!(device.authority(), RuntimeAuthority::ForegroundOnly);
    assert_eq!(device.framebuffer_path(), "/dev/fb0");
    assert_eq!(device.framebuffer_layout().visible().width(), 1264);
    assert_eq!(device.framebuffer_layout().line_length(), 1280);
    assert_eq!(device.framebuffer_rotation(), QuarterTurn::Upright);
    assert_eq!(device.input_path(), "/dev/input/event8");
    assert_eq!(device.input_name(), "cyttsp5_mt");
    assert_eq!(device.input_event_abi().libc_input_event_bytes, 16);
    assert_eq!(device.input_transform().output_extent().width(), 1264);
    assert_eq!(
        device
            .input_transform()
            .map(RawPoint { x: 1263, y: 1679 })
            .unwrap(),
        DisplayPoint { x: 1263, y: 1679 }
    );
    let refresh = device.refresh().unwrap();
    assert_eq!(refresh.update_abi(), DisplayUpdateAbiKind::Zelda88);
    assert!(refresh.capabilities().supports(RefreshMode::Partial));
    assert!(refresh.capabilities().supports(RefreshMode::Full));
    assert!(
        refresh
            .capabilities()
            .maximum_completion_wait_millis()
            .is_none()
    );
    assert_eq!(
        device.stock_repaint(),
        Some(StockRepaintMechanism::XrefreshDisplay0)
    );
}

#[test]
fn exact_pw1_pair_resolves_upright_touch_without_unproved_refresh() {
    let device = resolve(PW1_PROFILE, PW1_REPORT);

    assert_eq!(device.profile_id(), "reference-landscape");
    assert_eq!(device.authority(), RuntimeAuthority::ForegroundOnly);
    assert_eq!(device.framebuffer_path(), "/dev/fb0");
    assert_eq!(device.framebuffer_layout().visible().width(), 758);
    assert_eq!(device.framebuffer_layout().line_length(), 768);
    assert_eq!(device.framebuffer_rotation(), QuarterTurn::CounterClockwise);
    assert_eq!(device.input_path(), "/dev/input/event0");
    assert_eq!(device.input_name(), "cyttsp");
    assert_eq!(device.input_transform().output_extent().width(), 758);
    assert_eq!(
        device
            .input_transform()
            .map(RawPoint { x: 758, y: 1024 })
            .unwrap(),
        DisplayPoint { x: 757, y: 1023 }
    );
    assert!(device.refresh().is_none());
}

#[test]
fn historical_profile_without_runtime_policy_parses_but_cannot_resolve() {
    let mut profile = DeviceProfile::from_toml(KOA3_PROFILE).unwrap();
    profile.runtime = None;
    let report = ProbeReport::from_json(KOA3_REPORT).unwrap();

    assert_eq!(
        ResolvedRuntimeDevice::resolve(&profile, &report),
        Err(RuntimeResolutionError::RuntimePolicyMissing)
    );
}

#[test]
fn identity_mismatch_and_probe_warnings_fail_before_selection() {
    let profile = DeviceProfile::from_toml(KOA3_PROFILE).unwrap();
    let mut report = ProbeReport::from_json(KOA3_REPORT).unwrap();
    report.identity.serial_prefix = Some("DEMO".to_owned());
    assert!(matches!(
        ResolvedRuntimeDevice::resolve(&profile, &report),
        Err(RuntimeResolutionError::ProfileDidNotMatch(evaluation))
            if evaluation.status == ProfileEvaluationStatus::Mismatch
    ));

    let mut report = ProbeReport::from_json(KOA3_REPORT).unwrap();
    report.warnings.push(ferrink_platform::ProbeWarning {
        subsystem: "test".to_owned(),
        code: "forced".to_owned(),
        message: "synthetic warning".to_owned(),
    });
    assert_eq!(
        ResolvedRuntimeDevice::resolve(&profile, &report),
        Err(RuntimeResolutionError::ProbeWarningsPresent { count: 1 })
    );
}

#[test]
fn duplicate_framebuffer_or_touch_identity_is_rejected_as_ambiguous() {
    let profile = DeviceProfile::from_toml(KOA3_PROFILE).unwrap();
    let mut report = ProbeReport::from_json(KOA3_REPORT).unwrap();
    report.framebuffers.push(report.framebuffers[0].clone());
    assert_eq!(
        ResolvedRuntimeDevice::resolve(&profile, &report),
        Err(RuntimeResolutionError::FramebufferAmbiguous { count: 2 })
    );

    let mut report = ProbeReport::from_json(KOA3_REPORT).unwrap();
    let touch = report
        .inputs
        .iter()
        .find(|input| input.name.as_deref() == Some("cyttsp5_mt"))
        .unwrap()
        .clone();
    report.inputs.push(touch);
    assert_eq!(
        ResolvedRuntimeDevice::resolve(&profile, &report),
        Err(RuntimeResolutionError::TouchAmbiguous { count: 2 })
    );
}

#[test]
fn missing_axis_invalid_path_and_missing_executable_abi_fail_closed() {
    let mut profile = DeviceProfile::from_toml(KOA3_PROFILE).unwrap();
    profile.requirements.multitouch_axes = false;
    let mut report = ProbeReport::from_json(KOA3_REPORT).unwrap();
    let touch = report
        .inputs
        .iter_mut()
        .find(|input| input.name.as_deref() == Some("cyttsp5_mt"))
        .unwrap();
    touch.axes.retain(|axis| axis.code != 54);
    assert_eq!(
        ResolvedRuntimeDevice::resolve(&profile, &report),
        Err(RuntimeResolutionError::TouchAxisNotFound { code: 54 })
    );

    let profile = DeviceProfile::from_toml(KOA3_PROFILE).unwrap();
    let mut report = ProbeReport::from_json(KOA3_REPORT).unwrap();
    let touch = report
        .inputs
        .iter_mut()
        .find(|input| input.name.as_deref() == Some("cyttsp5_mt"))
        .unwrap();
    touch.device = "/tmp/event8".to_owned();
    assert_eq!(
        ResolvedRuntimeDevice::resolve(&profile, &report),
        Err(RuntimeResolutionError::InvalidInputPath)
    );

    let mut report = ProbeReport::from_json(KOA3_REPORT).unwrap();
    report.system.executable_abi = None;
    assert_eq!(
        ResolvedRuntimeDevice::resolve(&profile, &report),
        Err(RuntimeResolutionError::ExecutableAbiMissing)
    );
}

#[test]
fn evidence_pending_and_unsupported_input_abi_cannot_resolve() {
    let mut profile = DeviceProfile::from_toml(KOA3_PROFILE).unwrap();
    profile.review_status = ferrink_platform::ProfileReviewStatus::EvidencePending;
    let report = ProbeReport::from_json(KOA3_REPORT).unwrap();
    assert_eq!(
        ResolvedRuntimeDevice::resolve(&profile, &report),
        Err(RuntimeResolutionError::ProfileEvidencePending)
    );

    let profile = DeviceProfile::from_toml(KOA3_PROFILE).unwrap();
    let mut report = ProbeReport::from_json(KOA3_REPORT).unwrap();
    report.system.input_event_abi.libc_input_event_bytes = 20;
    assert!(matches!(
        ResolvedRuntimeDevice::resolve(&profile, &report),
        Err(RuntimeResolutionError::InputEventAbi(_))
    ));
}
