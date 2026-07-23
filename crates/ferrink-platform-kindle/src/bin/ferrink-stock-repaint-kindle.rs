use std::ffi::OsString;
use std::path::PathBuf;

#[cfg(all(target_os = "linux", target_arch = "arm", target_pointer_width = "32"))]
use std::io::IsTerminal;
#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
use std::io::{BufRead, Write};

const CARD_ID: &str = "koa3-stock-repaint-v1";
const REQUIRED_RESULT_FILE_NAME: &str = "koa3-stock-repaint-v1.json";
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
            eprintln!("ferrink-stock-repaint-kindle: {error}");
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
        LinuxReadOnlyDeviceIo, LinuxStockRepaintProcess, StockRepaintCore, revalidate_read_only,
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
        return Err("refusing stock repaint candidate without an interactive terminal".into());
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
    let core = StockRepaintCore::koa3_card_candidate(&runtime)?;

    let mut device_io = LinuxReadOnlyDeviceIo;
    let session = revalidate_read_only(&runtime, &mut device_io)?;
    drop(session);

    let result_path = validate_result_path(&arguments.result)?;
    print_plan(core, age, &result_path)?;
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

    let repaint = core.repaint(&mut LinuxStockRepaintProcess);
    let health = prompt_stock_health(&mut input, &mut prompts);
    let outcome = match &repaint {
        Ok(()) => "succeeded",
        Err(_) => "failed",
    };
    let evidence = json!({
        "schema_version": 1,
        "card_id": CARD_ID,
        "profile_id": runtime.profile_id(),
        "report_age_seconds": age,
        "mechanism": "xrefresh_display0",
        "command": {
            "executable": core.command().executable(),
            "arguments": core.command().arguments(),
            "timeout_millis": core.command().timeout().as_millis(),
            "shell": false,
            "attempts": 1
        },
        "outcome": outcome,
        "post_run_stock_health": health.as_str(),
        "descriptors_closed_before_repaint": true
    });
    serde_json::to_writer_pretty(&mut result_file, &evidence)?;
    result_file.write_all(b"\n")?;
    result_file.flush()?;
    result_file.sync_all()?;

    repaint?;
    if health != StockHealth::Healthy {
        return Err("stock display or touch health was not confirmed healthy".into());
    }
    println!("stock repaint candidate passed");
    println!("profile: {}", runtime.profile_id());
    println!("mechanism: /usr/bin/xrefresh -d :0.0");
    println!("child: exited successfully within 5000 ms");
    println!("stock display and touch: operator confirmed healthy");
    println!("evidence: written and synced");
    Ok(())
}

#[cfg(not(all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")))]
fn run_on_target(_arguments: Arguments) -> Result<(), Box<dyn std::error::Error>> {
    Err("stock repaint candidate requires 32-bit ARM Linux".into())
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
            Some("--confirm-stock-repaint") => {
                if confirmed {
                    return Err("--confirm-stock-repaint may be supplied only once".to_owned());
                }
                let value = arguments
                    .next()
                    .ok_or_else(|| "--confirm-stock-repaint requires its exact token".to_owned())?;
                if value != CARD_ID {
                    return Err("--confirm-stock-repaint token did not match".to_owned());
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
            "--confirm-stock-repaint {CARD_ID} is required after separate approval"
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
    core: ferrink_platform_kindle::StockRepaintCore,
    report_age: u64,
    result_path: &std::path::Path,
) -> Result<(), std::io::Error> {
    println!("ACTIVE SINGLE-USE STOCK REPAINT CANDIDATE");
    println!("card: {CARD_ID}");
    println!("report age: {report_age} seconds");
    println!("result: {} (create-new)", result_path.display());
    println!("executable: {}", core.command().executable());
    println!("arguments: {:?}", core.command().arguments());
    println!("environment: empty; shell: none; attempts: one");
    println!(
        "deadline: {} ms; timed-out child: killed and reaped",
        core.command().timeout().as_millis()
    );
    println!("framebuffer pixels, input events, services, properties, and power: untouched");
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
        "I am physically present at the KOA3.",
        "The stock screen and touch are healthy now.",
        "Power is adequate and no sleep or system transition is active.",
        "A second maintenance terminal can stop only this exact test PID.",
        "The fresh report and deployed artifact hashes were reviewed.",
    ] {
        prompt_exact(input, output, prompt, "YES")?;
    }
    prompt_exact(
        input,
        output,
        "Final immediate authorization for the printed candidate.",
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
        "ferrink-stock-repaint-kindle {}\n\n\
         Usage:\n  ferrink-stock-repaint-kindle --profile FILE --report FILE \\\n+         \n    --result /mnt/us/.../{REQUIRED_RESULT_FILE_NAME} \\\n+         \n    --confirm-stock-repaint {CARD_ID}\n\n\
         ACTIVE single-use KOA3 candidate. After fresh resolution and read-only\n\
         descriptor revalidation, runs exactly /usr/bin/xrefresh -d :0.0 once\n\
         with an empty environment and a five-second deadline. It never invokes\n\
         a shell or touches framebuffer pixels, input events, services, power,\n\
         packages, or boot state.",
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
            "/mnt/us/koa3-stock-repaint-v1.json",
            "--confirm-stock-repaint",
            CARD_ID,
        ]))
        .unwrap();
        assert!(matches!(parsed, ParseResult::Run(_)));
        assert!(parse_arguments(args(&[])).is_err());
        assert!(parse_arguments(args(&["--confirm-stock-repaint", "wrong"])).is_err());
    }

    #[test]
    fn substitution_noninteractive_and_active_flags_do_not_exist() {
        for flag in [
            "--command",
            "--argument",
            "--shell",
            "--environment",
            "--timeout",
            "--retry",
            "--force",
            "--yes",
            "--map-framebuffer",
            "--read-input",
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
        accepted.extend_from_slice(b"EXECUTE koa3-stock-repaint-v1\n");
        collect_preflight_confirmations(&mut accepted.as_slice(), &mut Vec::new()).unwrap();

        let mut declined = accepted;
        declined[0] = b'y';
        assert!(
            collect_preflight_confirmations(&mut declined.as_slice(), &mut Vec::new()).is_err()
        );
    }

    #[test]
    fn stock_health_defaults_to_uncertain_on_invalid_or_missing_input() {
        assert_eq!(
            prompt_stock_health(&mut b"healthy\n".as_slice(), &mut Vec::new()),
            StockHealth::Healthy
        );
        assert_eq!(
            prompt_stock_health(&mut b"typo\n".as_slice(), &mut Vec::new()),
            StockHealth::Uncertain
        );
        assert_eq!(
            prompt_stock_health(&mut b"".as_slice(), &mut Vec::new()),
            StockHealth::Uncertain
        );
    }
}
