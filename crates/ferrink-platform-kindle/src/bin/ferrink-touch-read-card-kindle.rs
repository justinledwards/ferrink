use std::ffi::OsString;
use std::path::PathBuf;

#[cfg(all(target_os = "linux", target_arch = "arm", target_pointer_width = "32"))]
use std::io::IsTerminal;
#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
use std::io::{BufRead, Write};

const CARD_ID: &str = "koa3-touch-read-v1";
const REQUIRED_RESULT_FILE_NAME: &str = "koa3-touch-read-v1.json";
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
            eprintln!("ferrink-touch-read-card-kindle: {error}");
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
        BoundedInputPump, InputLoopError, InputLoopLimits, InputPumpOutcome, Koa3TouchReadCard,
        L0InputCore, LinuxReadOnlyDeviceIo, revalidate_read_only,
    };
    use serde_json::json;

    const MAX_INPUT_FILE_BYTES: u64 = 1_048_576;
    const READ_WINDOW: Duration = Duration::from_secs(15);
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
        return Err("refusing touch-read card without an interactive terminal".into());
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
    let mut session = revalidate_read_only(&runtime, &mut device_io)?;
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

    let started = Instant::now();
    let mut pump = BoundedInputPump::try_new(
        started,
        InputLoopLimits {
            maximum_duration: READ_WINDOW,
            maximum_poll_slice: POLL_SLICE,
            maximum_steps: NonZeroU32::new(MAXIMUM_STEPS).expect("fixed step bound is non-zero"),
            maximum_bytes: NonZeroU32::new(MAXIMUM_BYTES).expect("fixed byte bound is non-zero"),
            read_buffer_bytes: NonZeroU32::new(READ_BUFFER_BYTES)
                .expect("fixed buffer bound is non-zero"),
        },
    )?;
    eprintln!("TOUCH NOW: activate exactly one safe, ordinary stock control");
    let mut accepted_bytes = 0_u32;
    let mut read_chunks = 0_u32;
    let mut read_outcome = "deadline";
    let mut card_failure = false;

    while !card.is_complete() {
        match session.pump_input_at(&mut pump, Instant::now(), &mut input_core) {
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

    let pass = if card_failure {
        None
    } else {
        match card.finish() {
            Ok(pass) => {
                read_outcome = "complete";
                Some(pass)
            }
            Err(_) => None,
        }
    };
    pump.stop();
    let elapsed_millis = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let decoded_records = input_core.decoded_records();
    let partial_bytes = input_core.partial_bytes();
    drop(session);

    let (stock_response, health) = if pass.is_some() {
        (
            prompt_stock_response(&mut input, &mut prompts),
            prompt_stock_health(&mut input, &mut prompts),
        )
    } else {
        (StockResponse::Uncertain, StockHealth::Uncertain)
    };
    let move_count = pass.map(|pass| pass.move_count());
    let passed = read_outcome == "complete"
        && move_count.is_some()
        && partial_bytes == 0
        && stock_response == StockResponse::Responded
        && health == StockHealth::Healthy;

    let evidence = json!({
        "schema_version": 1,
        "card_id": CARD_ID,
        "profile_id": runtime.profile_id(),
        "report_age_seconds": age,
        "input_path_revalidated": true,
        "input_open_mode": "read_only_nonblocking",
        "read_window_maximum_millis": READ_WINDOW.as_millis(),
        "read_window_actual_millis": elapsed_millis,
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
        "input_grab_attempts": 0,
        "descriptor_closed_before_prompts": true,
        "operator_stock_response": stock_response.as_str(),
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
        return Err("touch-read card did not pass; review its single-use evidence".into());
    }
    println!("KOA3 touch-read card passed");
    println!("touch: one complete coordinate-redacted sequence classified");
    println!("stock: operator confirmed the same touch responded and remains healthy");
    println!("evidence: written and synced");
    Ok(())
}

#[cfg(not(all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")))]
fn run_on_target(_arguments: Arguments) -> Result<(), Box<dyn std::error::Error>> {
    Err("touch-read card requires 32-bit ARM Linux".into())
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
            Some("--confirm-touch-read") => {
                if confirmed {
                    return Err("--confirm-touch-read may be supplied only once".to_owned());
                }
                let value = arguments
                    .next()
                    .ok_or_else(|| "--confirm-touch-read requires its exact token".to_owned())?;
                if value != CARD_ID {
                    return Err("--confirm-touch-read token did not match".to_owned());
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
            "--confirm-touch-read {CARD_ID} is required after separate approval"
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
    println!("ACTIVE SINGLE-USE KOA3 TOUCH-READ CARD");
    println!("card: {CARD_ID}");
    println!("report age: {report_age} seconds");
    println!("result: {} (create-new)", result_path.display());
    println!("input: exact revalidated path, read-only, nonblocking, no grab");
    println!("sequence: exactly one primary press, bounded moves, and release");
    println!("deadline: 15000 ms; 200 steps; 65536 bytes; 4096 records");
    println!("privacy: no raw events or coordinates persisted");
    println!("display writes, refresh, process/property/service/boot changes: none");
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
        "Stock display and touch are healthy now.",
        "I selected one safe ordinary stock control whose response is visible.",
        "Power is adequate and no sleep or system transition is active.",
        "The fresh report and deployed artifact hashes were reviewed.",
    ] {
        prompt_exact(input, output, prompt, "YES")?;
    }
    prompt_exact(
        input,
        output,
        "Final immediate authorization for the printed touch-read card.",
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
enum StockResponse {
    Responded,
    NoResponse,
    Uncertain,
}

#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
impl StockResponse {
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
fn prompt_stock_response(input: &mut impl BufRead, output: &mut impl Write) -> StockResponse {
    if writeln!(
        output,
        "Did the selected stock control visibly respond? Type: responded, no_response, uncertain"
    )
    .and_then(|()| {
        write!(output, "stock response: ")?;
        output.flush()
    })
    .is_err()
    {
        return StockResponse::Uncertain;
    }
    let mut response = String::new();
    if input.read_line(&mut response).is_err() {
        return StockResponse::Uncertain;
    }
    match response.trim_end_matches(['\r', '\n']) {
        "responded" => StockResponse::Responded,
        "no_response" => StockResponse::NoResponse,
        _ => StockResponse::Uncertain,
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
        "ferrink-touch-read-card-kindle {}\n\n\
         Usage:\n  ferrink-touch-read-card-kindle --profile FILE --report FILE \\\n+         \n    --result /mnt/us/.../{REQUIRED_RESULT_FILE_NAME} \\\n+         \n    --confirm-touch-read {CARD_ID}\n\n\
         ACTIVE single-use KOA3 card. It reads the exact revalidated touch\n\
         descriptor without a grab for at most 15 seconds, classifies exactly\n\
         one touch sequence, and stores no raw events or coordinates.",
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
            "/mnt/us/koa3-touch-read-v1.json",
            "--confirm-touch-read",
            CARD_ID,
        ]))
        .unwrap();
        assert!(matches!(parsed, ParseResult::Run(_)));
        assert!(parse_arguments(args(&[])).is_err());
        assert!(parse_arguments(args(&["--confirm-touch-read", "wrong"])).is_err());
    }

    #[test]
    fn escalation_and_substitution_flags_do_not_exist() {
        for flag in [
            "--device",
            "--duration",
            "--records",
            "--coordinates",
            "--trace",
            "--grab",
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
        accepted.extend_from_slice(b"EXECUTE koa3-touch-read-v1\n");
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
            prompt_stock_response(&mut b"responded\n".as_slice(), &mut Vec::new()),
            StockResponse::Responded
        );
        assert_eq!(
            prompt_stock_response(&mut b"typo\n".as_slice(), &mut Vec::new()),
            StockResponse::Uncertain
        );
        assert_eq!(
            prompt_stock_health(&mut b"healthy\n".as_slice(), &mut Vec::new()),
            StockHealth::Healthy
        );
    }
}
