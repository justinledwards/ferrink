use ferrink_platform::{Gray8FramebufferLayout, ProbeReport};

const KOA3_REPORT: &str = include_str!("fixtures/probe-reference-portrait.json");
const PW1_REPORT: &str = include_str!("fixtures/probe-reference-landscape.json");

fn first_layout(report: &str) -> Gray8FramebufferLayout {
    let report = ProbeReport::from_json(report).unwrap();
    Gray8FramebufferLayout::try_from_capability(&report.framebuffers[0]).unwrap()
}

#[test]
fn exact_koa3_fixture_builds_the_reviewed_gray8_layout() {
    let layout = first_layout(KOA3_REPORT);

    assert_eq!(layout.visible().width(), 1264);
    assert_eq!(layout.visible().height(), 1680);
    assert_eq!(layout.virtual_extent().width(), 1280);
    assert_eq!(layout.virtual_extent().height(), 3584);
    assert_eq!(layout.x_offset(), 0);
    assert_eq!(layout.y_offset(), 0);
    assert_eq!(layout.line_length(), 1280);
    assert_eq!(layout.memory_length(), 4_587_520);
}

#[test]
fn exact_pw1_fixture_builds_the_reviewed_gray8_layout() {
    let layout = first_layout(PW1_REPORT);

    assert_eq!(layout.visible().width(), 758);
    assert_eq!(layout.visible().height(), 1024);
    assert_eq!(layout.virtual_extent().width(), 768);
    assert_eq!(layout.virtual_extent().height(), 6144);
    assert_eq!(layout.x_offset(), 0);
    assert_eq!(layout.y_offset(), 0);
    assert_eq!(layout.line_length(), 768);
    assert_eq!(layout.memory_length(), 4_718_592);
}
