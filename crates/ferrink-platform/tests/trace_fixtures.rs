use ferrink_platform::{
    DisplayObservation, DisplayPoint, DisplayStockHealth, DisplaySubmissionResult, DisplayTrace,
    FramebufferMemoryAccess, InputTrace, LogicalTouchEvent, LogicalTouchPhase,
};

const ORDERING_TRACE: &str = include_str!("fixtures/input-trace-ordering-v1.json");
const DISPLAY_TRACE: &str = include_str!("fixtures/display-trace-shape-v1.json");
const KOA3_ACTIVE_DISPLAY_TRACE: &str =
    include_str!("fixtures/display-trace-reference-portrait-active-v1.json");

#[test]
fn ordering_fixture_preserves_pre_tracking_coordinates_and_primary_contact() {
    let trace = InputTrace::from_json(ORDERING_TRACE).unwrap();
    let events = trace.replay_touch().unwrap();

    assert_eq!(
        events,
        vec![
            LogicalTouchEvent {
                offset_micros: 40,
                phase: LogicalTouchPhase::Pressed,
                point: DisplayPoint { x: 0, y: 0 },
            },
            LogicalTouchEvent {
                offset_micros: 160,
                phase: LogicalTouchPhase::Moved,
                point: DisplayPoint { x: 757, y: 1023 },
            },
            LogicalTouchEvent {
                offset_micros: 180,
                phase: LogicalTouchPhase::Released,
                point: DisplayPoint { x: 757, y: 1023 },
            },
        ]
    );
}

#[test]
fn ordering_fixture_round_trips_through_strict_json() {
    let trace = InputTrace::from_json(ORDERING_TRACE).unwrap();
    let serialized = trace.to_json_pretty().unwrap();
    let reparsed = InputTrace::from_json(&serialized).unwrap();

    assert_eq!(reparsed, trace);
}

#[test]
fn synthetic_display_shape_round_trips_without_framebuffer_data() {
    let trace = DisplayTrace::from_json(DISPLAY_TRACE).unwrap();
    let serialized = trace.to_json_pretty().unwrap();
    let reparsed = DisplayTrace::from_json(&serialized).unwrap();

    assert_eq!(reparsed, trace);
    assert!(!serialized.contains("framebuffer_data"));
}

#[test]
fn koa3_active_trace_preserves_accepted_submission_without_overclaiming_observation() {
    let trace = DisplayTrace::from_json(KOA3_ACTIVE_DISPLAY_TRACE).unwrap();
    assert_eq!(trace.attempts.len(), 1);
    let attempt = &trace.attempts[0];

    assert_eq!(attempt.submission, DisplaySubmissionResult::Submitted);
    assert_eq!(attempt.observation, DisplayObservation::Uncertain);
    assert_eq!(trace.post_run_stock_health, DisplayStockHealth::Healthy);
    assert_eq!(attempt.plan.memory_access, FramebufferMemoryAccess::None);
    assert!(attempt.plan.wait.is_none());
    assert!(attempt.completion.is_none());
    assert!(trace.warnings.is_empty());
    assert_eq!(
        DisplayTrace::from_json(&trace.to_json_pretty().unwrap()).unwrap(),
        trace
    );
}
