use std::ffi::OsString;
#[cfg(target_os = "linux")]
use std::io::Read;
#[cfg(target_os = "linux")]
use std::path::Path;
use std::path::PathBuf;

const CONFIRMATION: &str = "ferrink-read-only-revalidation-v1";
#[cfg(target_os = "linux")]
const MAX_INPUT_FILE_BYTES: u64 = 1_048_576;
const MAX_REPORT_AGE_SECONDS: u64 = 15 * 60;

#[derive(Debug, PartialEq, Eq)]
struct Arguments {
    profile: PathBuf,
    report: PathBuf,
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
            eprintln!("ferrink-revalidate-kindle: {error}");
            std::process::exit(2);
        }
    }
}

#[cfg(target_os = "linux")]
fn run(arguments: impl Iterator<Item = OsString>) -> Result<(), Box<dyn std::error::Error>> {
    use std::time::{SystemTime, UNIX_EPOCH};

    use ferrink_platform::{DeviceProfile, ProbeReport, ResolvedRuntimeDevice};
    use ferrink_platform_kindle::{LinuxReadOnlyDeviceIo, revalidate_read_only};

    let arguments = match parse_arguments(arguments)? {
        ParseResult::Run(arguments) => arguments,
        ParseResult::Help => {
            print_help();
            return Ok(());
        }
    };
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
    let axis_count = runtime.input_capability().axes.len();
    let mut io = LinuxReadOnlyDeviceIo;
    let session = revalidate_read_only(&runtime, &mut io)?;
    drop(session);

    println!("read-only descriptor revalidation passed");
    println!("profile: {}", runtime.profile_id());
    println!("report age: {age} seconds");
    println!("framebuffer: exact passive metadata matched");
    println!("input: exact identity and {axis_count} advertised ABS axes matched");
    println!("descriptors: closed");
    println!("active operations: none");
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn run(arguments: impl Iterator<Item = OsString>) -> Result<(), Box<dyn std::error::Error>> {
    match parse_arguments(arguments)? {
        ParseResult::Help => {
            print_help();
            Ok(())
        }
        ParseResult::Run(_) => Err("Linux target required".into()),
    }
}

fn parse_arguments(mut arguments: impl Iterator<Item = OsString>) -> Result<ParseResult, String> {
    let mut profile = None;
    let mut report = None;
    let mut confirmed = false;
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--profile") => set_path(&mut profile, "--profile", &mut arguments)?,
            Some("--report") => set_path(&mut report, "--report", &mut arguments)?,
            Some("--confirm-read-only") => {
                if confirmed {
                    return Err("--confirm-read-only may be supplied only once".to_owned());
                }
                let value = arguments
                    .next()
                    .ok_or_else(|| "--confirm-read-only requires its exact token".to_owned())?;
                if value != CONFIRMATION {
                    return Err("--confirm-read-only token did not match".to_owned());
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
            "--confirm-read-only {CONFIRMATION} is required after separate approval"
        ));
    }
    Ok(ParseResult::Run(Arguments {
        profile: profile.ok_or_else(|| "--profile is required".to_owned())?,
        report: report.ok_or_else(|| "--report is required".to_owned())?,
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

#[cfg(target_os = "linux")]
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

fn print_help() {
    println!(
        "ferrink-revalidate-kindle {}\n\n\
         Usage:\n  ferrink-revalidate-kindle --profile FILE --report FILE \\\n+         \n    --confirm-read-only {}\n\n\
         Opens only the exact resolved framebuffer and input descriptors. The\n\
         framebuffer is read-only; input is read-only and nonblocking. It\n\
         repeats metadata queries, compares them with a warning-free report no\n\
         older than {} seconds, closes both descriptors, and exits. It does not\n\
         map or read framebuffer pixels, read input events, issue EVIOCGRAB,\n\
         submit a refresh, or change a service/property/file.",
        env!("CARGO_PKG_VERSION"),
        CONFIRMATION,
        MAX_REPORT_AGE_SECONDS
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args<'a>(values: &'a [&'a str]) -> impl Iterator<Item = OsString> + 'a {
        values.iter().map(OsString::from)
    }

    #[test]
    fn exact_confirmation_and_both_paths_are_required_once() {
        assert!(parse_arguments(args(&[])).is_err());
        assert!(parse_arguments(args(&["--profile", "p", "--report", "r"])).is_err());
        assert!(
            parse_arguments(args(&[
                "--profile",
                "p",
                "--report",
                "r",
                "--confirm-read-only",
                "wrong",
            ]))
            .is_err()
        );
        assert_eq!(
            parse_arguments(args(&[
                "--profile",
                "p",
                "--report",
                "r",
                "--confirm-read-only",
                CONFIRMATION,
            ])),
            Ok(ParseResult::Run(Arguments {
                profile: PathBuf::from("p"),
                report: PathBuf::from("r"),
            }))
        );
    }

    #[test]
    fn active_or_substitution_flags_do_not_exist() {
        for flag in [
            "--read-input",
            "--grab",
            "--map-framebuffer",
            "--refresh",
            "--fallback-device",
            "--ignore-age",
            "--force",
        ] {
            assert!(parse_arguments(args(&[flag])).is_err(), "accepted {flag}");
        }
    }
}
