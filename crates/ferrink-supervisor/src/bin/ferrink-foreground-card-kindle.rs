use std::ffi::OsString;
use std::path::PathBuf;

#[cfg(all(target_os = "linux", target_arch = "arm", target_pointer_width = "32"))]
use std::io::IsTerminal;
#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
use std::io::{BufRead, Write};

const CARD_ID: &str = "koa3-foreground-shell-v1";
const REQUIRED_RESULT_FILE_NAME: &str = "koa3-foreground-shell-v1.json";
#[cfg(all(target_os = "linux", target_arch = "arm", target_pointer_width = "32"))]
const OBSERVATION_MILLIS: u64 = 20_000;
#[cfg(all(target_os = "linux", target_arch = "arm", target_pointer_width = "32"))]
const MAX_REPORT_AGE_SECONDS: u64 = 15 * 60;

#[derive(Debug, PartialEq, Eq)]
struct Arguments {
    profile: PathBuf,
    report: PathBuf,
    result: PathBuf,
}

#[derive(Debug, PartialEq, Eq)]
enum ParseResult {
    Run(Arguments),
    Help,
}

fn main() {
    match run(std::env::args_os().skip(1)) {
        Ok(()) => {}
        Err(error) => {
            eprintln!("ferrink-foreground-card-kindle: {error}");
            std::process::exit(2);
        }
    }
}

fn run(arguments: impl Iterator<Item = OsString>) -> Result<(), Box<dyn std::error::Error>> {
    let arguments = match parse_arguments(arguments)? {
        ParseResult::Run(arguments) => arguments,
        ParseResult::Help => {
            print_help();
            return Ok(());
        }
    };
    run_on_target(arguments)
}

#[cfg(all(target_os = "linux", target_arch = "arm", target_pointer_width = "32"))]
fn run_on_target(arguments: Arguments) -> Result<(), Box<dyn std::error::Error>> {
    use std::cell::RefCell;
    use std::fs::OpenOptions;
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::Path;
    use std::rc::Rc;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use ferrink_platform::{
        DeviceProfile, Gray8Conversion, ProbeReport, RefreshCompletionPolicy, RefreshMode,
        ResolvedRuntimeDevice,
    };
    use ferrink_platform_kindle::{
        L0DisplayCore, LinuxForegroundDisplayTarget, LinuxReadOnlyDeviceIo, SlintFrameBuffer,
        new_slint_window, revalidate_read_only,
    };
    use ferrink_shell::{
        ShellController, ShellProfile, ShellWindow, configure_shell_window, install_shell_font,
        sync_shell_ui,
    };
    use ferrink_supervisor::{
        LinuxForegroundSignalGuard, LinuxKindleForegroundSystem, PillowState, acquire_foreground,
    };
    use serde_json::json;
    use slint::ComponentHandle;
    use slint::platform::{Platform, PlatformError, WindowAdapter};

    const MAX_INPUT_FILE_BYTES: u64 = 1_048_576;

    struct CardPlatform {
        window: Rc<slint::platform::software_renderer::MinimalSoftwareWindow>,
    }

    impl Platform for CardPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
            Ok(self.window.clone())
        }
    }

    fn read_regular_file(label: &str, path: &Path) -> Result<String, Box<dyn std::error::Error>> {
        let canonical = path
            .canonicalize()
            .map_err(|error| format!("cannot resolve {label} {}: {error}", path.display()))?;
        if ["/dev", "/proc", "/sys"]
            .iter()
            .any(|root| canonical.starts_with(root))
        {
            return Err(format!(
                "refusing {label} path in a device or kernel tree: {}",
                canonical.display()
            )
            .into());
        }
        let file = std::fs::File::open(&canonical)
            .map_err(|error| format!("cannot open {label} {}: {error}", canonical.display()))?;
        let metadata = file
            .metadata()
            .map_err(|error| format!("cannot inspect {label} {}: {error}", canonical.display()))?;
        if !metadata.is_file() || metadata.len() > MAX_INPUT_FILE_BYTES {
            return Err(format!("refusing invalid or oversized {label} file").into());
        }
        let mut input = String::new();
        file.take(MAX_INPUT_FILE_BYTES + 1)
            .read_to_string(&mut input)
            .map_err(|error| format!("cannot read UTF-8 {label}: {error}"))?;
        if input.len() as u64 > MAX_INPUT_FILE_BYTES {
            return Err(format!("{label} grew beyond {MAX_INPUT_FILE_BYTES} bytes").into());
        }
        Ok(input)
    }

    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err("refusing foreground card without an interactive terminal".into());
    }

    let profile_input = read_regular_file("profile", &arguments.profile)?;
    let report_input = read_regular_file("report", &arguments.report)?;
    let profile = DeviceProfile::from_toml(&profile_input)?;
    let report = ProbeReport::from_json(&report_input)?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let age = now
        .checked_sub(report.captured_at_unix_seconds)
        .ok_or("probe report timestamp is in the future")?;
    if age > MAX_REPORT_AGE_SECONDS {
        return Err(format!(
            "probe report is {age} seconds old; maximum is {MAX_REPORT_AGE_SECONDS}"
        )
        .into());
    }
    let runtime = ResolvedRuntimeDevice::resolve(&profile, &report)?;
    if runtime.profile_id() != "reference-portrait" {
        return Err("foreground card requires the exact reviewed KOA3 profile".into());
    }

    let mut device_io = LinuxReadOnlyDeviceIo;
    let session = revalidate_read_only(&runtime, &mut device_io)?;
    drop(session);

    // Allocate and initialize everything possible before the first stock mutation.
    let window = new_slint_window();
    slint::platform::set_platform(Box::new(CardPlatform {
        window: Rc::clone(&window),
    }))?;
    install_shell_font()?;
    let ui = ShellWindow::new()?;
    configure_shell_window(&ui, ShellProfile::Oasis3);
    let controller = RefCell::new(ShellController::default());
    sync_shell_ui(&ui, &controller.borrow());
    ui.show()?;
    window.request_redraw();
    let mut frame = SlintFrameBuffer::try_for_display(&L0DisplayCore::try_from_runtime(&runtime)?)?;
    let mut display = L0DisplayCore::try_from_runtime(&runtime)?;
    let mut system = LinuxKindleForegroundSystem::try_new(&runtime, PillowState::Enabled)?;

    let result_path = validate_result_path(&arguments.result)?;
    print_plan(age, &result_path)?;
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let stderr = std::io::stderr();
    let mut prompts = stderr.lock();
    collect_preflight_confirmations(&mut input, &mut prompts)?;

    let signal_guard = LinuxForegroundSignalGuard::install()?;
    let mut result_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&result_path)?;
    result_file.sync_all()?;

    let mut acquire = "failed";
    let mut paused_processes = None;
    let mut target_open = "not_run";
    let mut presentation = PresentationEvidence::NotRun;
    let mut mapping_close = "not_run";
    let mut hold_millis = 0_u64;
    let mut restore = "not_run";

    if let Ok(lease) = acquire_foreground(&mut system) {
        acquire = "succeeded";
        paused_processes = Some(lease.paused_process_count());

        match LinuxForegroundDisplayTarget::open(&runtime) {
            Ok(mut target) => {
                target_open = "succeeded";
                presentation = PresentationEvidence::from_result(frame.present_if_needed(
                    &window,
                    &mut display,
                    &mut target,
                    RefreshMode::Full,
                    RefreshCompletionPolicy::DoNotWait,
                    Gray8Conversion::Grayscale,
                ));

                if presentation.was_presented() {
                    let started = Instant::now();
                    let hold = Duration::from_millis(OBSERVATION_MILLIS);
                    while started.elapsed() < hold && signal_guard.pending_signal().is_none() {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    hold_millis = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
                }

                mapping_close = if target.close().is_ok() {
                    "succeeded"
                } else {
                    "failed"
                };
            }
            Err(_) => target_open = "failed",
        }

        restore = if lease.restore(&mut system).is_ok() {
            "succeeded"
        } else {
            "failed"
        };
    }

    let ui_hide = if ui.hide().is_ok() {
        "succeeded"
    } else {
        "failed"
    };
    let (signal_restore, deferred_signal) = match signal_guard.finish() {
        Ok(signal) => ("succeeded", signal.map(std::num::NonZeroI32::get)),
        Err(_) => ("failed", None),
    };

    let (observation, health) = if signal_restore == "succeeded" {
        let observation = if presentation.was_presented() {
            prompt_shell_observation(&mut input, &mut prompts)
        } else {
            ShellObservation::Uncertain
        };
        let health = prompt_stock_health(&mut input, &mut prompts);
        (observation, health)
    } else {
        (ShellObservation::Uncertain, StockHealth::Uncertain)
    };

    let full_screen = presentation.is_exact_full_screen(1264, 1680);
    let passed = acquire == "succeeded"
        && paused_processes.is_some_and(|count| count >= 2)
        && target_open == "succeeded"
        && full_screen
        && hold_millis >= OBSERVATION_MILLIS
        && mapping_close == "succeeded"
        && restore == "succeeded"
        && ui_hide == "succeeded"
        && signal_restore == "succeeded"
        && deferred_signal.is_none()
        && observation == ShellObservation::Seen
        && health == StockHealth::Healthy;

    let evidence = json!({
        "schema_version": 1,
        "card_id": CARD_ID,
        "profile_id": runtime.profile_id(),
        "report_age_seconds": age,
        "foreground_acquire": acquire,
        "paused_processes": paused_processes,
        "confirmed_initial_pillow_state": "enabled",
        "target_open": target_open,
        "presentation": presentation.as_json(),
        "observation_window_requested_millis": OBSERVATION_MILLIS,
        "observation_window_actual_millis": hold_millis,
        "mapping_close": mapping_close,
        "foreground_restore_and_stock_repaint": restore,
        "slint_hide": ui_hide,
        "signal_restore": signal_restore,
        "deferred_signal": deferred_signal,
        "operator_shell_observation": observation.as_str(),
        "post_run_stock_health": health.as_str(),
        "input_device_reads": 0,
        "input_grabs": 0,
        "refresh_attempts": if presentation.was_attempted() { 1 } else { 0 },
        "fallbacks": 0,
        "retries": 0,
        "boot_configuration_changes": 0,
        "reboots": 0,
        "passed": passed
    });
    serde_json::to_writer_pretty(&mut result_file, &evidence)?;
    result_file.write_all(b"\n")?;
    result_file.flush()?;
    result_file.sync_all()?;

    if !passed {
        return Err("foreground shell card did not pass; review its single-use evidence".into());
    }
    println!("KOA3 foreground shell card passed");
    println!("Ferrink shell: operator confirmed visible for 20 seconds");
    println!("foreground ownership: exact stock identities restored");
    println!("stock display and touch: operator confirmed healthy");
    println!("evidence: written and synced");
    Ok(())
}

#[cfg(not(all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")))]
fn run_on_target(_arguments: Arguments) -> Result<(), Box<dyn std::error::Error>> {
    Err("foreground card requires 32-bit ARM Linux".into())
}

#[cfg(all(target_os = "linux", target_arch = "arm", target_pointer_width = "32"))]
#[derive(Debug, Clone, Copy)]
enum PresentationEvidence {
    NotRun,
    Failed,
    Idle,
    RedrawnWithoutPixels,
    Presented {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        marker: u32,
    },
}

#[cfg(all(target_os = "linux", target_arch = "arm", target_pointer_width = "32"))]
impl PresentationEvidence {
    fn from_result(
        result: Result<
            ferrink_platform_kindle::SlintPresentOutcome,
            ferrink_platform_kindle::SlintRenderError,
        >,
    ) -> Self {
        match result {
            Ok(ferrink_platform_kindle::SlintPresentOutcome::Idle) => Self::Idle,
            Ok(ferrink_platform_kindle::SlintPresentOutcome::RedrawnWithoutPixels) => {
                Self::RedrawnWithoutPixels
            }
            Ok(ferrink_platform_kindle::SlintPresentOutcome::Presented { region, marker }) => {
                Self::Presented {
                    x: region.x(),
                    y: region.y(),
                    width: region.width(),
                    height: region.height(),
                    marker: marker.get(),
                }
            }
            Err(_) => Self::Failed,
        }
    }

    const fn was_attempted(self) -> bool {
        !matches!(self, Self::NotRun)
    }

    const fn was_presented(self) -> bool {
        matches!(self, Self::Presented { .. })
    }

    const fn is_exact_full_screen(self, width: u32, height: u32) -> bool {
        matches!(
            self,
            Self::Presented {
                x: 0,
                y: 0,
                width: actual_width,
                height: actual_height,
                ..
            } if actual_width == width && actual_height == height
        )
    }

    fn as_json(self) -> serde_json::Value {
        match self {
            Self::NotRun => serde_json::json!({ "outcome": "not_run" }),
            Self::Failed => serde_json::json!({ "outcome": "failed" }),
            Self::Idle => serde_json::json!({ "outcome": "idle" }),
            Self::RedrawnWithoutPixels => {
                serde_json::json!({ "outcome": "redrawn_without_pixels" })
            }
            Self::Presented {
                x,
                y,
                width,
                height,
                marker,
            } => serde_json::json!({
                "outcome": "presented",
                "region": { "x": x, "y": y, "width": width, "height": height },
                "refresh": {
                    "abi": "zelda88",
                    "waveform": "gc16",
                    "update_mode": "full",
                    "completion": "do_not_wait",
                    "marker": marker
                }
            }),
        }
    }
}

fn parse_arguments(mut arguments: impl Iterator<Item = OsString>) -> Result<ParseResult, String> {
    let mut profile = None;
    let mut report = None;
    let mut result = None;
    let mut confirmed = false;
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--profile") => set_path(&mut profile, "--profile", &mut arguments)?,
            Some("--report") => set_path(&mut report, "--report", &mut arguments)?,
            Some("--result") => set_path(&mut result, "--result", &mut arguments)?,
            Some("--confirm-foreground") => {
                if confirmed {
                    return Err("--confirm-foreground may be supplied only once".to_owned());
                }
                let value = arguments
                    .next()
                    .ok_or_else(|| "--confirm-foreground requires its exact token".to_owned())?;
                if value != CARD_ID {
                    return Err("--confirm-foreground token did not match".to_owned());
                }
                confirmed = true;
            }
            Some("-h" | "--help") => return Ok(ParseResult::Help),
            Some(other) => return Err(format!("unknown argument: {other}")),
            None => return Err("arguments must be valid Unicode".to_owned()),
        }
    }
    if !confirmed {
        return Err(format!(
            "--confirm-foreground {CARD_ID} is required after separate approval"
        ));
    }
    Ok(ParseResult::Run(Arguments {
        profile: profile.ok_or_else(|| "--profile is required".to_owned())?,
        report: report.ok_or_else(|| "--report is required".to_owned())?,
        result: result.ok_or_else(|| "--result is required".to_owned())?,
    }))
}

fn set_path(
    slot: &mut Option<PathBuf>,
    name: &str,
    arguments: &mut impl Iterator<Item = OsString>,
) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("{name} may be supplied only once"));
    }
    let value = arguments
        .next()
        .ok_or_else(|| format!("{name} requires a path"))?;
    *slot = Some(PathBuf::from(value));
    Ok(())
}

#[cfg(all(target_os = "linux", target_arch = "arm", target_pointer_width = "32"))]
fn validate_result_path(path: &std::path::Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if path.file_name().and_then(|name| name.to_str()) != Some(REQUIRED_RESULT_FILE_NAME) {
        return Err(format!("result file name must be {REQUIRED_RESULT_FILE_NAME}").into());
    }
    let parent = path
        .parent()
        .ok_or("result path has no parent")?
        .canonicalize()?;
    if !parent.starts_with("/mnt/us") {
        return Err("result parent must be within /mnt/us".into());
    }
    let target = parent.join(REQUIRED_RESULT_FILE_NAME);
    if target.try_exists()? {
        return Err(format!("single-use result already exists: {}", target.display()).into());
    }
    Ok(target)
}

#[cfg(all(target_os = "linux", target_arch = "arm", target_pointer_width = "32"))]
fn print_plan(report_age: u64, result_path: &std::path::Path) -> Result<(), std::io::Error> {
    println!("ACTIVE SINGLE-USE KOA3 FOREGROUND SHELL CARD");
    println!("card: {CARD_ID}");
    println!("report age: {report_age} seconds");
    println!("result: {} (create-new)", result_path.display());
    println!("ownership: inhibit sleep, disable Pillow, SIGSTOP exact awesome then cvm identities");
    println!("render: one full-screen 1264x1680 Slint frame, Zelda-88 full GC16, no wait");
    println!("observation: 20000 ms; no input event reads or grabs");
    println!(
        "return: unmap; SIGCONT cvm then awesome; enable Pillow; restore sleep value; xrefresh once"
    );
    println!("signals: HUP, INT, and TERM deferred until exact restoration completes");
    println!("attempts: one; retries: none; fallbacks: none; boot changes: none; reboot: none");
    std::io::stdout().flush()
}

#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
fn collect_preflight_confirmations(
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> Result<(), Box<dyn std::error::Error>> {
    for prompt in [
        "I am physically present and watching the entire KOA3 screen.",
        "The stock home screen and touch are healthy now.",
        "Power is adequate and no sleep, OTA, reboot, USB/storage, package, or recovery transition is active.",
        "The visible stock home screen confirms Pillow is initially enabled and no alternate launcher is active.",
        "A second maintenance terminal is available and the fresh report and artifact hashes were reviewed.",
    ] {
        prompt_exact(input, output, prompt, "YES")?;
    }
    prompt_exact(
        input,
        output,
        "Final immediate authorization for the printed foreground shell card.",
        &format!("EXECUTE {CARD_ID}"),
    )
}

#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
fn prompt_exact(
    input: &mut impl BufRead,
    output: &mut impl Write,
    prompt: &str,
    required: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    writeln!(output, "{prompt}")?;
    write!(output, "Type {required:?} exactly to confirm: ")?;
    output.flush()?;
    let mut response = String::new();
    if input.read_line(&mut response)? == 0 {
        return Err("confirmation input ended".into());
    }
    if response.trim_end_matches(['\r', '\n']) != required {
        return Err("confirmation declined or did not match exactly".into());
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
enum ShellObservation {
    Seen,
    Unchanged,
    Corrupted,
    Uncertain,
}

#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
impl ShellObservation {
    #[cfg(all(target_os = "linux", target_arch = "arm", target_pointer_width = "32"))]
    const fn as_str(self) -> &'static str {
        match self {
            Self::Seen => "seen",
            Self::Unchanged => "unchanged",
            Self::Corrupted => "corrupted",
            Self::Uncertain => "uncertain",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
enum StockHealth {
    Healthy,
    Unhealthy,
    Uncertain,
}

#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
impl StockHealth {
    #[cfg(all(target_os = "linux", target_arch = "arm", target_pointer_width = "32"))]
    const fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Unhealthy => "unhealthy",
            Self::Uncertain => "uncertain",
        }
    }
}

#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
fn prompt_shell_observation(input: &mut impl BufRead, output: &mut impl Write) -> ShellObservation {
    if writeln!(
        output,
        "For the 20-second full-screen shell, type one of: seen, unchanged, corrupted, uncertain"
    )
    .and_then(|()| {
        write!(output, "shell observation: ")?;
        output.flush()
    })
    .is_err()
    {
        return ShellObservation::Uncertain;
    }
    let mut response = String::new();
    if input.read_line(&mut response).is_err() {
        return ShellObservation::Uncertain;
    }
    match response.trim_end_matches(['\r', '\n']) {
        "seen" => ShellObservation::Seen,
        "unchanged" => ShellObservation::Unchanged,
        "corrupted" => ShellObservation::Corrupted,
        _ => ShellObservation::Uncertain,
    }
}

#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
fn prompt_stock_health(input: &mut impl BufRead, output: &mut impl Write) -> StockHealth {
    if writeln!(
        output,
        "After checking both stock display and touch, type one of: healthy, unhealthy, uncertain"
    )
    .and_then(|()| {
        write!(output, "post-run stock health: ")?;
        output.flush()
    })
    .is_err()
    {
        return StockHealth::Uncertain;
    }
    let mut response = String::new();
    if input.read_line(&mut response).is_err() {
        return StockHealth::Uncertain;
    }
    match response.trim_end_matches(['\r', '\n']) {
        "healthy" => StockHealth::Healthy,
        "unhealthy" => StockHealth::Unhealthy,
        _ => StockHealth::Uncertain,
    }
}

fn print_help() {
    println!(
        "ferrink-foreground-card-kindle {}\n\n\
         Usage:\n  ferrink-foreground-card-kindle --profile FILE --report FILE \\\n+         \n    --result /mnt/us/.../{REQUIRED_RESULT_FILE_NAME} \\\n+         \n    --confirm-foreground {CARD_ID}\n\n\
         ACTIVE single-use KOA3 card. It temporarily pauses the exact stock\n\
         foreground identities, renders one full-screen Ferrink Slint shell for\n\
         20 seconds, then restores stock and invokes the promoted repaint. It\n\
         never reads or grabs input and never changes boot configuration.",
        env!("CARGO_PKG_VERSION")
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args<'a>(values: &'a [&'a str]) -> impl Iterator<Item = OsString> + 'a {
        values.iter().map(OsString::from)
    }

    #[test]
    fn exact_paths_result_and_confirmation_are_required_once() {
        let parsed = parse_arguments(args(&[
            "--profile",
            "profile.toml",
            "--report",
            "report.json",
            "--result",
            "/mnt/us/koa3-foreground-shell-v1.json",
            "--confirm-foreground",
            CARD_ID,
        ]))
        .unwrap();
        assert!(matches!(parsed, ParseResult::Run(_)));
        assert!(parse_arguments(args(&[])).is_err());
        assert!(parse_arguments(args(&["--confirm-foreground", "wrong"])).is_err());
    }

    #[test]
    fn substitution_noninteractive_and_active_flags_do_not_exist() {
        for flag in [
            "--device",
            "--duration",
            "--waveform",
            "--marker",
            "--retry",
            "--force",
            "--yes",
            "--read-input",
            "--grab-input",
            "--reboot",
        ] {
            assert!(parse_arguments(args(&[flag])).is_err(), "accepted {flag}");
        }
    }

    #[test]
    fn every_confirmation_and_final_phrase_is_exact() {
        let mut accepted = Vec::new();
        for _ in 0..5 {
            accepted.extend_from_slice(b"YES\n");
        }
        accepted.extend_from_slice(b"EXECUTE koa3-foreground-shell-v1\n");
        collect_preflight_confirmations(&mut accepted.as_slice(), &mut Vec::new()).unwrap();

        let mut declined = accepted;
        declined[0] = b'y';
        assert!(
            collect_preflight_confirmations(&mut declined.as_slice(), &mut Vec::new()).is_err()
        );
    }

    #[test]
    fn observations_default_to_uncertain() {
        assert_eq!(
            prompt_shell_observation(&mut b"seen\n".as_slice(), &mut Vec::new()),
            ShellObservation::Seen
        );
        assert_eq!(
            prompt_shell_observation(&mut b"typo\n".as_slice(), &mut Vec::new()),
            ShellObservation::Uncertain
        );
        assert_eq!(
            prompt_stock_health(&mut b"healthy\n".as_slice(), &mut Vec::new()),
            StockHealth::Healthy
        );
    }
}
