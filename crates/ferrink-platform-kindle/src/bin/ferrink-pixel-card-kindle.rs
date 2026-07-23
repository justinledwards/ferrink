use std::ffi::OsString;
use std::path::PathBuf;

#[cfg(all(target_os = "linux", target_arch = "arm", target_pointer_width = "32"))]
use std::io::IsTerminal;
#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
use std::io::{BufRead, Write};

const CARD_ID: &str = "koa3-pixel-write-v1";
const REQUIRED_RESULT_FILE_NAME: &str = "koa3-pixel-write-v1.json";
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

#[cfg(all(target_os = "linux", target_arch = "arm", target_pointer_width = "32"))]
#[derive(Debug, Clone, Copy)]
enum PixelEvidence {
    NotRun,
    Passed(ferrink_platform_kindle::PixelCardPass),
    FailedBeforeWrite,
    FailedRestored,
    FailedRestoration,
    FailedOther,
}

#[cfg(all(target_os = "linux", target_arch = "arm", target_pointer_width = "32"))]
impl PixelEvidence {
    fn from_result(
        result: Result<
            ferrink_platform_kindle::PixelCardPass,
            ferrink_platform_kindle::PixelCardError,
        >,
    ) -> Self {
        use ferrink_platform_kindle::PixelCardError;

        match result {
            Ok(pass) => Self::Passed(pass),
            Err(PixelCardError::Memory(_)) => Self::FailedBeforeWrite,
            Err(PixelCardError::Operation(_)) => Self::FailedRestored,
            Err(
                PixelCardError::Restoration(_) | PixelCardError::OperationAndRestoration { .. },
            ) => Self::FailedRestoration,
            Err(_) => Self::FailedOther,
        }
    }

    const fn outcome(self) -> &'static str {
        match self {
            Self::NotRun => "not_run",
            Self::Passed(_) => "succeeded",
            Self::FailedBeforeWrite => "failed_before_write",
            Self::FailedRestored => "failed_original_bytes_restored",
            Self::FailedRestoration => "failed_restoration_unconfirmed",
            Self::FailedOther => "failed",
        }
    }

    const fn restored_bytes(self) -> Option<usize> {
        match self {
            Self::Passed(pass) => Some(pass.restored_bytes()),
            Self::FailedRestored => Some(4_096),
            Self::NotRun
            | Self::FailedBeforeWrite
            | Self::FailedRestoration
            | Self::FailedOther => None,
        }
    }

    const fn passed(self) -> bool {
        matches!(self, Self::Passed(_))
    }
}

fn main() {
    match run(std::env::args_os().skip(1)) {
        Ok(()) => {}
        Err(error) => {
            eprintln!("ferrink-pixel-card-kindle: {error}");
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
    use std::fs::OpenOptions;
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    use ferrink_platform::{DeviceProfile, ProbeReport, ResolvedRuntimeDevice};
    use ferrink_platform_kindle::{
        Koa3PixelCard, LinuxKoa3PixelTarget, LinuxPixelSignalGuard, LinuxReadOnlyDeviceIo,
        LinuxStockRepaintProcess, StockRepaintCore, revalidate_read_only,
    };
    use serde_json::json;

    const MAX_INPUT_FILE_BYTES: u64 = 1_048_576;

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
        return Err("refusing pixel card without an interactive terminal".into());
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
    let mut card = Koa3PixelCard::try_from_runtime(&runtime)?;
    let repaint_core = StockRepaintCore::try_from_runtime(&runtime)?;

    let mut device_io = LinuxReadOnlyDeviceIo;
    let session = revalidate_read_only(&runtime, &mut device_io)?;
    drop(session);

    let result_path = validate_result_path(&arguments.result)?;
    print_plan(&card, age, &result_path)?;
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let stderr = std::io::stderr();
    let mut prompts = stderr.lock();
    collect_preflight_confirmations(&mut input, &mut prompts)?;

    let signal_guard = LinuxPixelSignalGuard::install()?;
    let mut result_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&result_path)?;
    result_file.sync_all()?;

    let mut target_open = "failed";
    let mut pixel = PixelEvidence::NotRun;
    let mut mapping_close = "not_run";
    if let Ok(mut target) = LinuxKoa3PixelTarget::open(&runtime) {
        target_open = "succeeded";
        pixel = PixelEvidence::from_result(card.execute(&mut target));
        mapping_close = if target.close().is_ok() {
            "succeeded"
        } else {
            "failed"
        };
    }

    let repaint = if target_open == "succeeded" {
        if repaint_core.repaint(&mut LinuxStockRepaintProcess).is_ok() {
            "succeeded"
        } else {
            "failed"
        }
    } else {
        "not_run"
    };
    let (signal_restore, deferred_signal) = match signal_guard.finish() {
        Ok(signal) => ("succeeded", signal.map(std::num::NonZeroI32::get)),
        Err(_) => ("failed", None),
    };

    let (observation, health) = if deferred_signal.is_none() && signal_restore == "succeeded" {
        (
            prompt_pattern_observation(&mut input, &mut prompts),
            prompt_stock_health(&mut input, &mut prompts),
        )
    } else {
        (PatternObservation::Uncertain, StockHealth::Uncertain)
    };
    let passed = target_open == "succeeded"
        && pixel.passed()
        && mapping_close == "succeeded"
        && repaint == "succeeded"
        && signal_restore == "succeeded"
        && deferred_signal.is_none()
        && observation == PatternObservation::Seen
        && health == StockHealth::Healthy;

    let region = card.region();
    let evidence = json!({
        "schema_version": 1,
        "card_id": CARD_ID,
        "profile_id": runtime.profile_id(),
        "report_age_seconds": age,
        "region": {
            "x": region.x(),
            "y": region.y(),
            "width": region.width(),
            "height": region.height()
        },
        "pattern": "black-border-8x8-black-white-checker",
        "refresh": {
            "abi": "zelda88",
            "waveform_mode": 257,
            "update_mode": "partial",
            "completion": "do_not_wait",
            "marker": card.marker().get(),
            "attempts": 1
        },
        "observation_window_millis": card.observation_window().as_millis(),
        "target_open": target_open,
        "pixel_outcome": pixel.outcome(),
        "restored_bytes": pixel.restored_bytes(),
        "mapping_close": mapping_close,
        "deferred_signal": deferred_signal,
        "signal_restore": signal_restore,
        "stock_repaint": repaint,
        "operator_pattern_observation": observation.as_str(),
        "post_run_stock_health": health.as_str(),
        "passed": passed,
        "descriptors_closed_before_pixel_write": true,
        "shell": false,
        "fallbacks": 0,
        "retries": 0
    });
    serde_json::to_writer_pretty(&mut result_file, &evidence)?;
    result_file.write_all(b"\n")?;
    result_file.flush()?;
    result_file.sync_all()?;

    if !passed {
        return Err(
            "pixel card did not pass; review its single-use evidence before any further action"
                .into(),
        );
    }
    println!("KOA3 pixel card passed");
    println!("pattern: operator saw the centered 64x64 checker");
    println!("memory: 4096 original bytes restored before unmap");
    println!("stock display and touch: operator confirmed healthy");
    println!("evidence: written and synced");
    Ok(())
}

#[cfg(not(all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")))]
fn run_on_target(_arguments: Arguments) -> Result<(), Box<dyn std::error::Error>> {
    Err("pixel card requires 32-bit ARM Linux".into())
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
            Some("--confirm-pixel-write") => {
                if confirmed {
                    return Err("--confirm-pixel-write may be supplied only once".to_owned());
                }
                let value = arguments
                    .next()
                    .ok_or_else(|| "--confirm-pixel-write requires its exact token".to_owned())?;
                if value != CARD_ID {
                    return Err("--confirm-pixel-write token did not match".to_owned());
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
            "--confirm-pixel-write {CARD_ID} is required after separate approval"
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
fn print_plan(
    card: &ferrink_platform_kindle::Koa3PixelCard,
    report_age: u64,
    result_path: &std::path::Path,
) -> Result<(), std::io::Error> {
    let region = card.region();
    println!("ACTIVE SINGLE-USE KOA3 PIXEL CARD");
    println!("card: {CARD_ID}");
    println!("report age: {report_age} seconds");
    println!("result: {} (create-new)", result_path.display());
    println!(
        "region: x={} y={} width={} height={} (center)",
        region.x(),
        region.y(),
        region.width(),
        region.height()
    );
    println!("pattern: black-border 8x8 black/white checker for 3000 ms");
    println!("memory: snapshot 4096 bytes, write once, restore before unmap");
    println!("refresh: Zelda-88 partial waveform 257, marker 0x464b0002, no wait");
    println!("return: exactly /usr/bin/xrefresh -d :0.0 once after the attempt");
    println!("signals: HUP, INT, and TERM deferred until restore, unmap, and repaint");
    println!("attempts: one; retries: none; fallbacks: none; shell: none");
    println!("input events, grabs, services, properties, power, packages, and boot: untouched");
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
        "I am physically present and watching the center of the KOA3 screen.",
        "The stock screen and touch are healthy now.",
        "Power is adequate and no sleep or system transition is active.",
        "The stock repaint card passed and its profile promotion was reviewed.",
        "The fresh report and deployed artifact hashes were reviewed.",
    ] {
        prompt_exact(input, output, prompt, "YES")?;
    }
    prompt_exact(
        input,
        output,
        "Final immediate authorization for the printed pixel card.",
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
enum PatternObservation {
    Seen,
    Unchanged,
    Corrupted,
    Uncertain,
}

#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
impl PatternObservation {
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
fn prompt_pattern_observation(
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> PatternObservation {
    if writeln!(
        output,
        "For the three-second center pattern, type one of: seen, unchanged, corrupted, uncertain"
    )
    .and_then(|()| {
        write!(output, "pattern observation: ")?;
        output.flush()
    })
    .is_err()
    {
        return PatternObservation::Uncertain;
    }
    let mut response = String::new();
    if input.read_line(&mut response).is_err() {
        return PatternObservation::Uncertain;
    }
    match response.trim_end_matches(['\r', '\n']) {
        "seen" => PatternObservation::Seen,
        "unchanged" => PatternObservation::Unchanged,
        "corrupted" => PatternObservation::Corrupted,
        _ => PatternObservation::Uncertain,
    }
}

#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
fn prompt_stock_health(input: &mut impl BufRead, output: &mut impl Write) -> StockHealth {
    if writeln!(
        output,
        "After checking both display and touch, type one of: healthy, unhealthy, uncertain"
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
        "ferrink-pixel-card-kindle {}\n\n\
         Usage:\n  ferrink-pixel-card-kindle --profile FILE --report FILE \\\n+         \n    --result /mnt/us/.../{REQUIRED_RESULT_FILE_NAME} \\\n+         \n    --confirm-pixel-write {CARD_ID}\n\n\
         ACTIVE single-use KOA3 card. Requires a fresh exact profile/report and\n\
         previously reviewed stock repaint. It snapshots a centered 64x64 Gray8\n\
         region, shows one checker for three seconds with the exact Zelda-88 ABI,\n\
         restores all 4096 original bytes, unmaps, and invokes the exact stock\n\
         repaint once. It never reads input events or changes system state.",
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
            "/mnt/us/koa3-pixel-write-v1.json",
            "--confirm-pixel-write",
            CARD_ID,
        ]))
        .unwrap();
        assert!(matches!(parsed, ParseResult::Run(_)));
        assert!(parse_arguments(args(&[])).is_err());
        assert!(parse_arguments(args(&["--confirm-pixel-write", "wrong"])).is_err());
    }

    #[test]
    fn substitution_noninteractive_and_active_flags_do_not_exist() {
        for flag in [
            "--device",
            "--region",
            "--pattern",
            "--waveform",
            "--marker",
            "--timeout",
            "--retry",
            "--force",
            "--yes",
            "--read-input",
            "--grab-input",
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
        accepted.extend_from_slice(b"EXECUTE koa3-pixel-write-v1\n");
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
            prompt_pattern_observation(&mut b"seen\n".as_slice(), &mut Vec::new()),
            PatternObservation::Seen
        );
        assert_eq!(
            prompt_pattern_observation(&mut b"typo\n".as_slice(), &mut Vec::new()),
            PatternObservation::Uncertain
        );
        assert_eq!(
            prompt_stock_health(&mut b"healthy\n".as_slice(), &mut Vec::new()),
            StockHealth::Healthy
        );
        assert_eq!(
            prompt_stock_health(&mut b"".as_slice(), &mut Vec::new()),
            StockHealth::Uncertain
        );
    }
}
