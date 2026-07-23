mod ffi;

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io::{BufRead, IsTerminal, Read, Write};
use std::num::{NonZeroU16, NonZeroU32};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use ferrink_platform::{
    ActiveDisplayPlan, ActiveDisplayRequestPlan, DISPLAY_TRACE_SCHEMA_VERSION, DeviceProfile,
    DisplayExtent, DisplayObservation, DisplayPreflight, DisplayStockHealth,
    DisplaySubmissionResult, DisplayTrace, DisplayUpdateAttempt, DisplayUpdatePlan,
    FramebufferCapability, FramebufferFingerprint, ProbeReport, ProbeWarning, QuarterTurn,
    characterization_redaction_metadata,
};

const MAX_INPUT_FILE_BYTES: u64 = 1_048_576;
const MAX_REPORT_AGE_SECONDS: u64 = 600;
const MAX_REPORT_FUTURE_SKEW_SECONDS: u64 = 60;
const MINIMUM_USERSTORE_AVAILABLE_BYTES: u64 = 1_048_576;
const REQUIRED_OUTPUT_FILE_NAME: &str = "reference-portrait-display-mechanism-v1.json";
const REQUIRED_CORE_PROCESSES: &[&str] = &[
    "lipc-daemon",
    "volumd",
    "powerd",
    "Xorg",
    "awesome",
    "wifid",
];

#[derive(Debug, PartialEq, Eq)]
struct Arguments {
    plan: PathBuf,
    profile: PathBuf,
    report: PathBuf,
    output: PathBuf,
}

#[derive(Debug, PartialEq, Eq)]
enum ParseResult {
    Run(Arguments),
    Help,
}

#[derive(Debug)]
struct ExecutionResult {
    submission: DisplaySubmissionResult,
    warnings: Vec<ProbeWarning>,
}

fn main() {
    match run(std::env::args_os().skip(1)) {
        Ok(()) => {}
        Err(error) => {
            eprintln!("ferrink-characterize-display: {error}");
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

    require_active_target()?;

    let plan_input = read_regular_file("plan", &arguments.plan)?;
    let profile_input = read_regular_file("profile", &arguments.profile)?;
    let report_input = read_regular_file("report", &arguments.report)?;
    let plan = ActiveDisplayPlan::from_json(&plan_input)?;
    let profile = DeviceProfile::from_toml(&profile_input)?;
    let report = ProbeReport::from_json(&report_input)?;
    plan.validate_against(&profile, &report)
        .map_err(|errors| format!("offline validation failed: {errors:?}"))?;

    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    validate_report_age(report.captured_at_unix_seconds, now)?;
    validate_userstore(&report)?;
    validate_core_processes(&report)?;
    let output_path = validate_output_path(&arguments.output, &plan)?;

    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err("refusing active display operation without an interactive terminal".into());
    }

    print_active_plan(&plan, &report, &output_path)?;
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let stderr = std::io::stderr();
    let mut prompts = stderr.lock();
    collect_preflight_confirmations(&mut input, &mut prompts, &plan.plan_id)?;

    let mut output = create_output_file(&output_path)?;
    output.sync_all()?;

    let execution = execute_once(&plan.request);
    println!("submission result: {:?}", execution.submission);
    let observation = match execution.submission {
        DisplaySubmissionResult::OpenError { .. } => DisplayObservation::NotObserved,
        DisplaySubmissionResult::Submitted | DisplaySubmissionResult::Error { .. } => {
            prompt_observation(&mut input, &mut prompts).unwrap_or(DisplayObservation::Uncertain)
        }
    };
    let stock_health =
        prompt_stock_health(&mut input, &mut prompts).unwrap_or(DisplayStockHealth::Uncertain);

    let trace = build_trace(
        &plan,
        &report,
        execution.submission,
        observation,
        stock_health,
        execution.warnings,
    )?;
    write_trace(&mut output, &trace)?;
    println!("evidence written and synced: {}", output_path.display());

    let safe_result = matches!(execution.submission, DisplaySubmissionResult::Submitted)
        && matches!(
            observation,
            DisplayObservation::Unchanged
                | DisplayObservation::Updated
                | DisplayObservation::Flashed
        )
        && stock_health == DisplayStockHealth::Healthy;
    if !safe_result {
        return Err(
            "active result was unsuccessful or uncertain; evidence is preserved and no recovery action was attempted"
                .into(),
        );
    }

    println!("active mechanism result accepted; no completion wait or second ioctl was issued");
    Ok(())
}

fn parse_arguments(mut arguments: impl Iterator<Item = OsString>) -> Result<ParseResult, String> {
    let mut plan = None;
    let mut profile = None;
    let mut report = None;
    let mut output = None;

    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--plan") => set_path(&mut plan, "--plan", &mut arguments)?,
            Some("--profile") => set_path(&mut profile, "--profile", &mut arguments)?,
            Some("--report") => set_path(&mut report, "--report", &mut arguments)?,
            Some("--output") => set_path(&mut output, "--output", &mut arguments)?,
            Some("-h" | "--help") => return Ok(ParseResult::Help),
            Some(other) => return Err(format!("unknown argument: {other}")),
            None => return Err("arguments must be valid Unicode".to_owned()),
        }
    }

    Ok(ParseResult::Run(Arguments {
        plan: plan.ok_or_else(|| "--plan is required".to_owned())?,
        profile: profile.ok_or_else(|| "--profile is required".to_owned())?,
        report: report.ok_or_else(|| "--report is required".to_owned())?,
        output: output.ok_or_else(|| "--output is required".to_owned())?,
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

fn require_active_target() -> Result<(), String> {
    if cfg!(all(
        target_os = "linux",
        target_arch = "arm",
        target_pointer_width = "32"
    )) {
        Ok(())
    } else {
        Err("active display operation requires a 32-bit ARM Linux binary".to_owned())
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
    if !metadata.is_file() {
        return Err(format!("refusing non-regular {label} file: {}", canonical.display()).into());
    }
    if metadata.len() > MAX_INPUT_FILE_BYTES {
        return Err(format!(
            "{label} is {} bytes; maximum is {MAX_INPUT_FILE_BYTES}",
            metadata.len()
        )
        .into());
    }

    let mut input = String::new();
    file.take(MAX_INPUT_FILE_BYTES + 1)
        .read_to_string(&mut input)
        .map_err(|error| format!("cannot read UTF-8 {label} {}: {error}", canonical.display()))?;
    if input.len() as u64 > MAX_INPUT_FILE_BYTES {
        return Err(
            format!("{label} grew beyond {MAX_INPUT_FILE_BYTES} bytes while reading").into(),
        );
    }
    Ok(input)
}

fn validate_report_age(captured: u64, now: u64) -> Result<(), String> {
    if captured > now.saturating_add(MAX_REPORT_FUTURE_SKEW_SECONDS) {
        return Err(format!(
            "passive report timestamp is more than {MAX_REPORT_FUTURE_SKEW_SECONDS} seconds in the future"
        ));
    }
    let age = now.saturating_sub(captured);
    if age > MAX_REPORT_AGE_SECONDS {
        return Err(format!(
            "passive report is {age} seconds old; maximum is {MAX_REPORT_AGE_SECONDS}"
        ));
    }
    Ok(())
}

fn validate_userstore(report: &ProbeReport) -> Result<(), String> {
    let userstore = &report.storage.userstore;
    if userstore.path != "/mnt/us"
        || !userstore.exists
        || !userstore.mounted
        || userstore.read_only != Some(false)
    {
        return Err("fresh report does not show a mounted writable /mnt/us userstore".to_owned());
    }
    let available = report
        .storage
        .filesystems
        .iter()
        .find(|filesystem| filesystem.path == "/mnt/us")
        .map(|filesystem| filesystem.available_bytes)
        .ok_or_else(|| "fresh report has no bounded /mnt/us space record".to_owned())?;
    if available < MINIMUM_USERSTORE_AVAILABLE_BYTES {
        return Err(format!(
            "fresh report shows only {available} available userstore bytes; minimum is {MINIMUM_USERSTORE_AVAILABLE_BYTES}"
        ));
    }
    Ok(())
}

fn validate_core_processes(report: &ProbeReport) -> Result<(), String> {
    let observed = report
        .processes
        .iter()
        .map(|process| process.name.as_str())
        .collect::<BTreeSet<_>>();
    let missing = REQUIRED_CORE_PROCESSES
        .iter()
        .copied()
        .filter(|required| !observed.contains(required))
        .collect::<Vec<_>>();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "fresh report is missing core stock processes: {}",
            missing.join(", ")
        ))
    }
}

fn validate_output_path(path: &Path, plan: &ActiveDisplayPlan) -> Result<PathBuf, String> {
    if path.file_name().and_then(|name| name.to_str()) != Some(REQUIRED_OUTPUT_FILE_NAME) {
        return Err(format!(
            "output file name must be {REQUIRED_OUTPUT_FILE_NAME}"
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| "output path has no parent".to_owned())?
        .canonicalize()
        .map_err(|error| format!("cannot resolve output parent: {error}"))?;
    let expected_parent = Path::new(&plan.output_parent)
        .canonicalize()
        .map_err(|error| format!("cannot resolve planned output parent: {error}"))?;
    if parent != expected_parent {
        return Err(format!(
            "output parent {} does not match planned {}",
            parent.display(),
            expected_parent.display()
        ));
    }
    let canonical_target = parent.join(REQUIRED_OUTPUT_FILE_NAME);
    if canonical_target.try_exists().map_err(|error| {
        format!(
            "cannot check output target {}: {error}",
            canonical_target.display()
        )
    })? {
        return Err(format!(
            "single-use output already exists: {}",
            canonical_target.display()
        ));
    }
    Ok(canonical_target)
}

fn print_active_plan(
    plan: &ActiveDisplayPlan,
    report: &ProbeReport,
    output: &Path,
) -> Result<(), std::io::Error> {
    println!("ACTIVE SINGLE-USE DISPLAY REQUEST");
    println!("plan: {}", plan.plan_id);
    println!("fresh passive capture: {}", report.captured_at_unix_seconds);
    println!("output: {} (create-new)", output.display());
    println!("device: {} (read-only, close-on-exec)", plan.request.device);
    println!(
        "region: x={}, y={}, width={}, height={}",
        plan.request.region.x,
        plan.request.region.y,
        plan.request.region.width,
        plan.request.region.height
    );
    println!(
        "ioctl: 0x{:08x}; Zelda {} bytes; waveform={}; update_mode={}; marker=0x{:08x}",
        plan.request.request_ioctl,
        plan.request.abi.request_size,
        plan.request.waveform_mode,
        plan.request.update_mode,
        plan.request.marker
    );
    println!(
        "temperature={}; flags={}; dither={}; quant={}; alternate_buffer_zeroed={}; hist={:?}; timestamps={:?}",
        plan.request.temperature,
        plan.request.flags,
        plan.request.dither_mode,
        plan.request.quant_bit,
        plan.request.alternate_buffer_zeroed,
        plan.request.histogram_modes,
        plan.request.timestamps
    );
    println!("pixel access: none; completion wait: none; fallback: none; attempts: one");
    println!("the tool will stop after this request and will not attempt recovery");
    std::io::stdout().flush()
}

fn collect_preflight_confirmations(
    input: &mut impl BufRead,
    output: &mut impl Write,
    plan_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let confirmations = [
        "I am physically present at the Kindle.",
        "The stock screen is readable and responsive now.",
        "An independent maintenance connection is healthy now.",
        "Power is adequate and the Kindle is not entering sleep.",
        "No OTA, reboot, shutdown, USB-mode, or storage transition is active.",
        "The create-new output location is ready.",
        "The fresh passive report was collected in this maintenance window.",
    ];
    for confirmation in confirmations {
        prompt_exact(input, output, confirmation, "YES")?;
    }
    prompt_exact(
        input,
        output,
        "Final immediate authorization for the one printed ioctl.",
        &format!("EXECUTE {plan_id}"),
    )
}

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

fn create_output_file(path: &Path) -> Result<std::fs::File, std::io::Error> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn execute_once(plan: &ActiveDisplayRequestPlan) -> ExecutionResult {
    let framebuffer = match open_framebuffer(&plan.device) {
        Ok(framebuffer) => framebuffer,
        Err(error) => {
            let (errno, warning) = errno_for_trace(&error, "open");
            return ExecutionResult {
                submission: DisplaySubmissionResult::OpenError { errno },
                warnings: warning.into_iter().collect(),
            };
        }
    };

    match ffi::submit(&framebuffer, plan) {
        Ok(()) => ExecutionResult {
            submission: DisplaySubmissionResult::Submitted,
            warnings: Vec::new(),
        },
        Err(error) => {
            let (errno, warning) = errno_for_trace(&error, "ioctl");
            ExecutionResult {
                submission: DisplaySubmissionResult::Error { errno },
                warnings: warning.into_iter().collect(),
            }
        }
    }
}

#[cfg(unix)]
fn open_framebuffer(path: &str) -> Result<std::fs::File, std::io::Error> {
    use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};

    if path != "/dev/fb0" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "only /dev/fb0 is permitted",
        ));
    }
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_char_device() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "/dev/fb0 is not a character device",
        ));
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC)
        .open(path)?;
    if !file.metadata()?.file_type().is_char_device() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "opened /dev/fb0 is not a character device",
        ));
    }
    Ok(file)
}

#[cfg(not(unix))]
fn open_framebuffer(_path: &str) -> Result<std::fs::File, std::io::Error> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "framebuffer open requires Unix",
    ))
}

fn errno_for_trace(error: &std::io::Error, stage: &str) -> (NonZeroU16, Option<ProbeWarning>) {
    let errno = error
        .raw_os_error()
        .and_then(|value| u16::try_from(value).ok())
        .and_then(NonZeroU16::new);
    match errno {
        Some(errno) => (errno, None),
        None => (
            NonZeroU16::new(libc::EIO as u16).unwrap_or(NonZeroU16::MIN),
            Some(ProbeWarning {
                subsystem: "display".to_owned(),
                code: "unrepresentable_errno".to_owned(),
                message: format!(
                    "{stage} failure had no representable positive errno; recorded EIO"
                ),
            }),
        ),
    }
}

fn prompt_observation(
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> Result<DisplayObservation, Box<dyn std::error::Error>> {
    writeln!(
        output,
        "Observe the 64x64 center region. Type one of: unchanged, updated, flashed, corrupted, uncertain"
    )?;
    write!(output, "physical observation: ")?;
    output.flush()?;
    let mut response = String::new();
    if input.read_line(&mut response)? == 0 {
        return Err("observation input ended".into());
    }
    match response.trim_end_matches(['\r', '\n']) {
        "unchanged" => Ok(DisplayObservation::Unchanged),
        "updated" => Ok(DisplayObservation::Updated),
        "flashed" => Ok(DisplayObservation::Flashed),
        "corrupted" => Ok(DisplayObservation::Corrupted),
        "uncertain" => Ok(DisplayObservation::Uncertain),
        _ => Err("invalid physical observation".into()),
    }
}

fn prompt_stock_health(
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> Result<DisplayStockHealth, Box<dyn std::error::Error>> {
    writeln!(
        output,
        "Is the stock UI still readable and responsive? Type one of: healthy, unhealthy, uncertain"
    )?;
    write!(output, "post-run stock health: ")?;
    output.flush()?;
    let mut response = String::new();
    if input.read_line(&mut response)? == 0 {
        return Err("stock-health input ended".into());
    }
    match response.trim_end_matches(['\r', '\n']) {
        "healthy" => Ok(DisplayStockHealth::Healthy),
        "unhealthy" => Ok(DisplayStockHealth::Unhealthy),
        "uncertain" => Ok(DisplayStockHealth::Uncertain),
        _ => Err("invalid stock-health result".into()),
    }
}

fn build_trace(
    plan: &ActiveDisplayPlan,
    report: &ProbeReport,
    submission: DisplaySubmissionResult,
    observation: DisplayObservation,
    stock_health: DisplayStockHealth,
    warnings: Vec<ProbeWarning>,
) -> Result<DisplayTrace, Box<dyn std::error::Error>> {
    let framebuffer = report
        .framebuffers
        .iter()
        .find(|framebuffer| framebuffer.device == plan.request.device)
        .ok_or("validated report lost its selected framebuffer")?;
    let trace = DisplayTrace {
        schema_version: DISPLAY_TRACE_SCHEMA_VERSION,
        redaction: characterization_redaction_metadata(),
        profile_id: plan.profile_id.clone(),
        framebuffer: framebuffer_fingerprint(framebuffer)?,
        preflight: DisplayPreflight::Confirmed {
            operator_present: true,
            stock_ui_healthy: true,
            maintenance_connection_healthy: true,
            power_adequate_and_awake: true,
            no_transition_or_update: true,
            output_ready: true,
            fresh_passive_report: true,
        },
        attempts: vec![DisplayUpdateAttempt {
            plan: DisplayUpdatePlan {
                open_mode: plan.request.open_mode,
                memory_access: plan.request.memory_access,
                region: plan.request.region,
                abi: plan.request.abi,
                request_ioctl: plan.request.request_ioctl,
                waveform_mode: plan.request.waveform_mode,
                update_mode: plan.request.update_mode,
                temperature: plan.request.temperature,
                flags: plan.request.flags,
                dither_mode: plan.request.dither_mode,
                quant_bit: plan.request.quant_bit,
                alternate_buffer_zeroed: plan.request.alternate_buffer_zeroed,
                histogram_modes: plan.request.histogram_modes,
                timestamps: plan.request.timestamps,
                marker: plan.request.marker,
                wait: None,
            },
            submission,
            completion: None,
            observation,
        }],
        post_run_stock_health: stock_health,
        warnings,
    };
    trace
        .validate()
        .map_err(|errors| format!("refusing invalid active evidence: {errors:?}"))?;
    Ok(trace)
}

fn framebuffer_fingerprint(
    framebuffer: &FramebufferCapability,
) -> Result<FramebufferFingerprint, Box<dyn std::error::Error>> {
    Ok(FramebufferFingerprint {
        device: framebuffer.device.clone(),
        driver_id: framebuffer.driver_id.clone(),
        visible: DisplayExtent::try_new(framebuffer.visible_width, framebuffer.visible_height)?,
        virtual_extent: DisplayExtent::try_new(
            framebuffer.virtual_width,
            framebuffer.virtual_height,
        )?,
        line_length: NonZeroU32::new(framebuffer.line_length)
            .ok_or("framebuffer line length is zero")?,
        memory_length: NonZeroU32::new(framebuffer.memory_length)
            .ok_or("framebuffer memory length is zero")?,
        bits_per_pixel: NonZeroU32::new(framebuffer.bits_per_pixel)
            .ok_or("framebuffer bit depth is zero")?,
        pixel_layout: framebuffer.pixel_layout,
        rotation: QuarterTurn::try_from_linux_framebuffer(framebuffer.rotation)?,
    })
}

fn write_trace(output: &mut std::fs::File, trace: &DisplayTrace) -> Result<(), std::io::Error> {
    let json = trace
        .to_json_pretty()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    output.write_all(json.as_bytes())?;
    output.write_all(b"\n")?;
    output.flush()?;
    output.sync_all()
}

fn print_help() {
    println!(
        "ferrink-characterize-display {}\n\n\
         ACTIVE, single-purpose KOA3 display characterization. This binary can\n\
         submit exactly one reviewed Zelda update after strict passive checks\n\
         and eight interactive confirmations. It never maps or writes pixels,\n\
         waits for completion, retries, falls back, or attempts recovery.\n\n\
         Usage:\n\
           ferrink-characterize-display --plan PATH --profile PATH \\\n+             --report PATH --output /mnt/us/ferrink/evidence/reference-portrait-display-mechanism-v1.json",
        env!("CARGO_PKG_VERSION")
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN: &str = include_str!(
        "../../ferrink-platform/tests/fixtures/reference-portrait-display-mechanism-plan-v1.json"
    );
    const PROFILE: &str = include_str!("../../../device-profiles/reference-portrait.toml");
    const REPORT: &str =
        include_str!("../../ferrink-platform/tests/fixtures/probe-reference-portrait.json");

    #[test]
    fn arguments_have_no_noninteractive_or_fallback_flags() {
        let parsed = parse_arguments(
            [
                "--plan",
                "plan.json",
                "--profile",
                "profile.toml",
                "--report",
                "report.json",
                "--output",
                "output.json",
            ]
            .map(OsString::from)
            .into_iter(),
        )
        .unwrap();
        assert!(matches!(parsed, ParseResult::Run(_)));

        for forbidden in ["--yes", "--force", "--fallback", "--device", "--wait"] {
            let error = parse_arguments([OsString::from(forbidden)].into_iter()).unwrap_err();
            assert!(error.contains("unknown argument"));
        }
    }

    #[test]
    fn passive_report_age_is_bounded_without_underflow() {
        assert!(validate_report_age(1_000, 1_500).is_ok());
        assert!(validate_report_age(1_000, 1_601).is_err());
        assert!(validate_report_age(1_061, 1_000).is_err());
        assert!(validate_report_age(1_060, 1_000).is_ok());
    }

    #[test]
    fn every_preflight_confirmation_and_final_phrase_is_exact() {
        let plan = ActiveDisplayPlan::from_json(PLAN).unwrap();
        let mut accepted = Vec::new();
        for _ in 0..7 {
            accepted.extend_from_slice(b"YES\n");
        }
        accepted.extend_from_slice(b"EXECUTE reference-portrait-display-mechanism-v1\n");
        collect_preflight_confirmations(&mut accepted.as_slice(), &mut Vec::new(), &plan.plan_id)
            .unwrap();

        let mut declined = accepted;
        declined[0] = b'y';
        assert!(
            collect_preflight_confirmations(
                &mut declined.as_slice(),
                &mut Vec::new(),
                &plan.plan_id
            )
            .is_err()
        );
    }

    #[test]
    fn accepted_result_builds_complete_strict_trace() {
        let plan = ActiveDisplayPlan::from_json(PLAN).unwrap();
        let profile = DeviceProfile::from_toml(PROFILE).unwrap();
        let report = ProbeReport::from_json(REPORT).unwrap();
        plan.validate_against(&profile, &report).unwrap();

        let trace = build_trace(
            &plan,
            &report,
            DisplaySubmissionResult::Submitted,
            DisplayObservation::Unchanged,
            DisplayStockHealth::Healthy,
            Vec::new(),
        )
        .unwrap();
        assert_eq!(trace.attempts.len(), 1);
        assert_eq!(trace.attempts[0].plan.request_ioctl.get(), 0x4058_462e);
        assert_eq!(trace.post_run_stock_health, DisplayStockHealth::Healthy);
        assert!(trace.to_json_pretty().unwrap().contains("confirmed"));
    }

    #[test]
    fn observation_and_stock_health_inputs_are_enumerated() {
        assert_eq!(
            prompt_observation(&mut b"flashed\n".as_slice(), &mut Vec::new()).unwrap(),
            DisplayObservation::Flashed
        );
        assert!(prompt_observation(&mut b"looks fine\n".as_slice(), &mut Vec::new()).is_err());
        assert_eq!(
            prompt_stock_health(&mut b"healthy\n".as_slice(), &mut Vec::new()).unwrap(),
            DisplayStockHealth::Healthy
        );
        assert!(prompt_stock_health(&mut b"probably\n".as_slice(), &mut Vec::new()).is_err());
    }
}
