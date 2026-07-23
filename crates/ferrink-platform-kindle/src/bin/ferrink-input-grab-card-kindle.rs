use std::ffi::OsString;
use std::path::PathBuf;

#[cfg(all(target_os = "linux", target_arch = "arm", target_pointer_width = "32"))]
use std::io::IsTerminal;
#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
use std::io::{BufRead, Write};

const CARD_ID: &str = "koa3-input-grab-v1";
const REQUIRED_RESULT_FILE_NAME: &str = "koa3-input-grab-v1.json";
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
            eprintln!("ferrink-input-grab-card-kindle: {error}");
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
    use std::num::NonZeroU32;
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::Path;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use ferrink_platform::{DeviceProfile, ProbeReport, ResolvedRuntimeDevice};
    use ferrink_platform_kindle::{
        BoundedInputPump, ExclusiveInputSession, InputLoopError, InputLoopLimits, InputPumpOutcome,
        Koa3TouchReadCard, L0InputCore, LinuxReadOnlyDeviceIo, revalidate_read_only,
    };
    use serde_json::json;

    const MAX_INPUT_FILE_BYTES: u64 = 1_048_576;
    const GRAB_WINDOW: Duration = Duration::from_secs(15);
    const POLL_SLICE: Duration = Duration::from_millis(100);
    const MAXIMUM_STEPS: u32 = 200;
    const MAXIMUM_BYTES: u32 = 65_536;
    const READ_BUFFER_BYTES: u32 = 4_096;
    const MAXIMUM_RECORDS: u32 = 4_096;

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
        return Err("refusing input-grab card without an interactive terminal".into());
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
    let mut card = Koa3TouchReadCard::try_from_runtime(&runtime)?;
    let mut device_io = LinuxReadOnlyDeviceIo;
    let session = revalidate_read_only(&runtime, &mut device_io)?;
    let mut input_core = L0InputCore::try_from_runtime(
        &runtime,
        NonZeroU32::new(MAXIMUM_RECORDS).expect("fixed record bound is non-zero"),
    )?;

    let result_path = validate_result_path(&arguments.result)?;
    print_plan(age, &result_path)?;
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let stderr = std::io::stderr();
    let mut prompts = stderr.lock();
    collect_preflight_confirmations(&mut input, &mut prompts)?;

    let mut result_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&result_path)?;
    result_file.sync_all()?;

    let mut grab_acquire = "failed";
    let mut grab_release = "not_run";
    let mut read_outcome = "not_run";
    let mut accepted_bytes = 0_u32;
    let mut read_chunks = 0_u32;
    let mut elapsed_millis = 0_u64;
    let mut pass = None;

    let started = Instant::now();
    let mut pump = BoundedInputPump::try_new(
        started,
        InputLoopLimits {
            maximum_duration: GRAB_WINDOW,
            maximum_poll_slice: POLL_SLICE,
            maximum_steps: NonZeroU32::new(MAXIMUM_STEPS).expect("fixed step bound is non-zero"),
            maximum_bytes: NonZeroU32::new(MAXIMUM_BYTES).expect("fixed byte bound is non-zero"),
            read_buffer_bytes: NonZeroU32::new(READ_BUFFER_BYTES)
                .expect("fixed buffer bound is non-zero"),
        },
    )?;
    if let Ok(mut exclusive) = ExclusiveInputSession::acquire(session) {
        grab_acquire = "succeeded";
        eprintln!("GRAB ACQUIRED: TOUCH NOW; exactly one normal tap");
        read_outcome = "deadline";
        let mut card_failure = false;

        while !card.is_complete() {
            match exclusive.pump_input_at(&mut pump, Instant::now(), &mut input_core) {
                Ok(
                    InputPumpOutcome::TimedOut
                    | InputPumpOutcome::Interrupted
                    | InputPumpOutcome::WouldBlock,
                ) => {}
                Ok(InputPumpOutcome::Read { bytes, contacts }) => {
                    accepted_bytes = accepted_bytes.saturating_add(bytes);
                    read_chunks = read_chunks.saturating_add(1);
                    if card.observe(&contacts).is_err() {
                        read_outcome = "invalid_sequence";
                        card_failure = true;
                        break;
                    }
                }
                Err(InputLoopError::DeadlineReached) => break,
                Err(_) => {
                    read_outcome = "failed";
                    card_failure = true;
                    break;
                }
            }
        }

        if !card_failure && let Ok(card_pass) = card.finish() {
            read_outcome = "complete";
            pass = Some(card_pass);
        }
        pump.stop();
        elapsed_millis = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        grab_release = if exclusive.release().is_ok() {
            "succeeded"
        } else {
            "failed_descriptor_closed"
        };
    }

    let decoded_records = input_core.decoded_records();
    let partial_bytes = input_core.partial_bytes();
    let (during_grab, after_release, health) = if pass.is_some() && grab_release == "succeeded" {
        (
            prompt_during_grab_response(&mut input, &mut prompts),
            prompt_after_release_response(&mut input, &mut prompts),
            prompt_stock_health(&mut input, &mut prompts),
        )
    } else {
        (
            DuringGrabResponse::Uncertain,
            AfterReleaseResponse::Uncertain,
            StockHealth::Uncertain,
        )
    };
    let move_count = pass.map(|pass| pass.move_count());
    let passed = grab_acquire == "succeeded"
        && grab_release == "succeeded"
        && read_outcome == "complete"
        && move_count.is_some()
        && partial_bytes == 0
        && during_grab == DuringGrabResponse::Blocked
        && after_release == AfterReleaseResponse::Responded
        && health == StockHealth::Healthy;

    let evidence = json!({
        "schema_version": 1,
        "card_id": CARD_ID,
        "profile_id": runtime.profile_id(),
        "report_age_seconds": age,
        "input_path_revalidated": true,
        "input_open_mode": "read_only_nonblocking_then_exact_grab",
        "grab_acquire": grab_acquire,
        "grab_release": grab_release,
        "descriptor_closed_after_release_attempt": true,
        "grab_window_maximum_millis": GRAB_WINDOW.as_millis(),
        "grab_window_actual_millis": elapsed_millis,
        "maximum_steps": MAXIMUM_STEPS,
        "maximum_bytes": MAXIMUM_BYTES,
        "maximum_records": MAXIMUM_RECORDS,
        "accepted_bytes": accepted_bytes,
        "read_chunks": read_chunks,
        "decoded_records": decoded_records,
        "partial_bytes": partial_bytes,
        "read_outcome": read_outcome,
        "classified_sequence": {
            "presses": u8::from(pass.is_some()),
            "moves": move_count,
            "releases": u8::from(pass.is_some())
        },
        "coordinates_persisted": false,
        "raw_events_persisted": false,
        "grab_attempts": 1,
        "release_attempts": u8::from(grab_acquire == "succeeded"),
        "operator_during_grab_stock_response": during_grab.as_str(),
        "operator_after_release_stock_response": after_release.as_str(),
        "post_run_stock_health": health.as_str(),
        "fallbacks": 0,
        "retries": 0,
        "display_writes": 0,
        "refresh_submissions": 0,
        "service_or_property_changes": 0,
        "boot_configuration_changes": 0,
        "reboots": 0,
        "passed": passed
    });
    serde_json::to_writer_pretty(&mut result_file, &evidence)?;
    result_file.write_all(b"\n")?;
    result_file.flush()?;
    result_file.sync_all()?;

    if !passed {
        return Err("input-grab card did not pass; review its single-use evidence".into());
    }
    println!("KOA3 input-grab card passed");
    println!("grab: one exact acquire and one explicit release succeeded");
    println!("touch: one complete coordinate-redacted sequence classified");
    println!("stock: blocked during grab, responsive and healthy after release");
    println!("evidence: written and synced");
    Ok(())
}

#[cfg(not(all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")))]
fn run_on_target(_arguments: Arguments) -> Result<(), Box<dyn std::error::Error>> {
    Err("input-grab card requires 32-bit ARM Linux".into())
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
            Some("--confirm-input-grab") => {
                if confirmed {
                    return Err("--confirm-input-grab may be supplied only once".to_owned());
                }
                let value = arguments
                    .next()
                    .ok_or_else(|| "--confirm-input-grab requires its exact token".to_owned())?;
                if value != CARD_ID {
                    return Err("--confirm-input-grab token did not match".to_owned());
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
            "--confirm-input-grab {CARD_ID} is required after separate approval"
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
    println!("ACTIVE SINGLE-USE KOA3 INPUT-GRAB CARD");
    println!("card: {CARD_ID}");
    println!("report age: {report_age} seconds");
    println!("result: {} (create-new)", result_path.display());
    println!("input: exact revalidated path; one EVIOCGRAB(1), one EVIOCGRAB(0)");
    println!("sequence: exactly one primary press, bounded moves, and release");
    println!("grab deadline: 15000 ms; 200 steps; 65536 bytes; 4096 records");
    println!("crash fallback: descriptor close; no ioctl or retry from Drop");
    println!("privacy: no raw events or coordinates persisted");
    println!("display/process/service/property/power/storage/boot changes: none");
    println!("attempts: one; retries: none; fallbacks: none; reboot: none");
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
        "I am physically present with the KOA3 stock home screen visible.",
        "The separate KOA3 touch-read card passed and its evidence was reviewed.",
        "Stock display and touch are healthy and one safe stock control is selected.",
        "Power is adequate and no sleep or system transition is active.",
        "A second maintenance terminal and reviewed fresh report and artifact hashes are available.",
    ] {
        prompt_exact(input, output, prompt, "YES")?;
    }
    prompt_exact(
        input,
        output,
        "Final immediate authorization for the printed input-grab card.",
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
enum DuringGrabResponse {
    Blocked,
    Responded,
    Uncertain,
}

#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
impl DuringGrabResponse {
    #[cfg(all(target_os = "linux", target_arch = "arm", target_pointer_width = "32"))]
    const fn as_str(self) -> &'static str {
        match self {
            Self::Blocked => "blocked",
            Self::Responded => "responded",
            Self::Uncertain => "uncertain",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
enum AfterReleaseResponse {
    Responded,
    NoResponse,
    Uncertain,
}

#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
impl AfterReleaseResponse {
    #[cfg(all(target_os = "linux", target_arch = "arm", target_pointer_width = "32"))]
    const fn as_str(self) -> &'static str {
        match self {
            Self::Responded => "responded",
            Self::NoResponse => "no_response",
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
fn prompt_during_grab_response(
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> DuringGrabResponse {
    if writeln!(
        output,
        "During the grabbed tap, did stock stay unchanged? Type: blocked, responded, uncertain"
    )
    .and_then(|()| {
        write!(output, "during-grab stock response: ")?;
        output.flush()
    })
    .is_err()
    {
        return DuringGrabResponse::Uncertain;
    }
    let mut response = String::new();
    if input.read_line(&mut response).is_err() {
        return DuringGrabResponse::Uncertain;
    }
    match response.trim_end_matches(['\r', '\n']) {
        "blocked" => DuringGrabResponse::Blocked,
        "responded" => DuringGrabResponse::Responded,
        _ => DuringGrabResponse::Uncertain,
    }
}

#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
fn prompt_after_release_response(
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> AfterReleaseResponse {
    if writeln!(
        output,
        "Now tap the same stock control after release. Type: responded, no_response, uncertain"
    )
    .and_then(|()| {
        write!(output, "after-release stock response: ")?;
        output.flush()
    })
    .is_err()
    {
        return AfterReleaseResponse::Uncertain;
    }
    let mut response = String::new();
    if input.read_line(&mut response).is_err() {
        return AfterReleaseResponse::Uncertain;
    }
    match response.trim_end_matches(['\r', '\n']) {
        "responded" => AfterReleaseResponse::Responded,
        "no_response" => AfterReleaseResponse::NoResponse,
        _ => AfterReleaseResponse::Uncertain,
    }
}

#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
fn prompt_stock_health(input: &mut impl BufRead, output: &mut impl Write) -> StockHealth {
    if writeln!(
        output,
        "After checking stock display and touch, type: healthy, unhealthy, uncertain"
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
        "ferrink-input-grab-card-kindle {}\n\n\
         Usage:\n  ferrink-input-grab-card-kindle --profile FILE --report FILE \\\n+         \n    --result /mnt/us/.../{REQUIRED_RESULT_FILE_NAME} \\\n+         \n    --confirm-input-grab {CARD_ID}\n\n\
         ACTIVE single-use KOA3 card. It acquires the exact input grab once,\n\
         classifies one coordinate-redacted touch for at most 15 seconds,\n\
         releases once, closes descriptors, and verifies stock response.",
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
            "/mnt/us/koa3-input-grab-v1.json",
            "--confirm-input-grab",
            CARD_ID,
        ]))
        .unwrap();
        assert!(matches!(parsed, ParseResult::Run(_)));
        assert!(parse_arguments(args(&[])).is_err());
        assert!(parse_arguments(args(&["--confirm-input-grab", "wrong"])).is_err());
    }

    #[test]
    fn escalation_and_substitution_flags_do_not_exist() {
        for flag in [
            "--device",
            "--duration",
            "--records",
            "--coordinates",
            "--trace",
            "--keep-grabbed",
            "--retry",
            "--force",
            "--yes",
            "--write-display",
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
        accepted.extend_from_slice(b"EXECUTE koa3-input-grab-v1\n");
        collect_preflight_confirmations(&mut accepted.as_slice(), &mut Vec::new()).unwrap();

        let mut declined = accepted;
        declined[0] = b'y';
        assert!(
            collect_preflight_confirmations(&mut declined.as_slice(), &mut Vec::new()).is_err()
        );
    }

    #[test]
    fn operator_observations_default_to_uncertain() {
        assert_eq!(
            prompt_during_grab_response(&mut b"blocked\n".as_slice(), &mut Vec::new()),
            DuringGrabResponse::Blocked
        );
        assert_eq!(
            prompt_after_release_response(&mut b"responded\n".as_slice(), &mut Vec::new()),
            AfterReleaseResponse::Responded
        );
        assert_eq!(
            prompt_stock_health(&mut b"typo\n".as_slice(), &mut Vec::new()),
            StockHealth::Uncertain
        );
    }
}
