use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};

use ferrink_platform::{ActiveDisplayPlan, DeviceProfile, ProbeReport};

const MAX_INPUT_FILE_BYTES: u64 = 1_048_576;

#[derive(Debug, PartialEq, Eq)]
struct Arguments {
    plan: PathBuf,
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
            eprintln!("ferrink-characterize-plan: {error}");
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

    let plan_input = read_regular_file("plan", &arguments.plan)?;
    let profile_input = read_regular_file("profile", &arguments.profile)?;
    let report_input = read_regular_file("report", &arguments.report)?;

    let plan = ActiveDisplayPlan::from_json(&plan_input)?;
    let profile = DeviceProfile::from_toml(&profile_input)?;
    let report = ProbeReport::from_json(&report_input)?;
    plan.validate_against(&profile, &report)
        .map_err(|errors| format!("offline validation failed: {errors:?}"))?;

    println!("offline validation passed: {}", plan.plan_id);
    println!("profile: {}", plan.profile_id);
    println!(
        "request: one read-only Zelda-88 submission at x={}, y={}, {}x{}; no pixel access and no completion wait",
        plan.request.region.x,
        plan.request.region.y,
        plan.request.region.width,
        plan.request.region.height
    );
    println!("hardware access: none; this binary contains no device adapter");
    println!(
        "still unverified: operator presence, stock UI, maintenance connection, battery, sleep/update state, userstore, and fresh-capture timing"
    );
    Ok(())
}

fn parse_arguments(mut arguments: impl Iterator<Item = OsString>) -> Result<ParseResult, String> {
    let mut plan = None;
    let mut profile = None;
    let mut report = None;

    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--plan") => set_path(&mut plan, "--plan", &mut arguments)?,
            Some("--profile") => set_path(&mut profile, "--profile", &mut arguments)?,
            Some("--report") => set_path(&mut report, "--report", &mut arguments)?,
            Some("-h" | "--help") => return Ok(ParseResult::Help),
            Some(other) => return Err(format!("unknown argument: {other}")),
            None => return Err("arguments must be valid Unicode".to_owned()),
        }
    }

    Ok(ParseResult::Run(Arguments {
        plan: plan.ok_or_else(|| "--plan is required".to_owned())?,
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
        "ferrink-characterize-plan {}\n\n\
         Offline-only validation of the pinned first KOA3 active-display plan.\n\
         This binary reads three bounded regular files and has no device adapter.\n\n\
         Usage:\n\
           ferrink-characterize-plan --plan PATH --profile PATH --report PATH\n\n\
         A pass validates immutable plan and passive-evidence fields only. It\n\
         does not authorize execution or verify any live/operator condition.",
        env!("CARGO_PKG_VERSION")
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_three_named_inputs_are_required_once() {
        let parsed = parse_arguments(
            [
                "--report",
                "report.json",
                "--plan",
                "plan.json",
                "--profile",
                "profile.toml",
            ]
            .map(OsString::from)
            .into_iter(),
        )
        .unwrap();
        assert_eq!(
            parsed,
            ParseResult::Run(Arguments {
                plan: "plan.json".into(),
                profile: "profile.toml".into(),
                report: "report.json".into(),
            })
        );

        let duplicate = parse_arguments(
            [
                "--plan",
                "a",
                "--plan",
                "b",
                "--profile",
                "c",
                "--report",
                "d",
            ]
            .map(OsString::from)
            .into_iter(),
        )
        .unwrap_err();
        assert!(duplicate.contains("only once"));
    }

    #[test]
    fn execution_style_flags_do_not_exist() {
        let error = parse_arguments([OsString::from("--execute")].into_iter()).unwrap_err();
        assert!(error.contains("unknown argument"));
    }

    #[test]
    fn known_repository_inputs_validate_without_hardware() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let plan_input = read_regular_file(
            "plan",
            &root
                .join("crates/ferrink-platform/tests/fixtures/reference-portrait-display-mechanism-plan-v1.json"),
        )
        .unwrap();
        let profile_input = read_regular_file(
            "profile",
            &root.join("device-profiles/reference-portrait.toml"),
        )
        .unwrap();
        let report_input = read_regular_file(
            "report",
            &root.join("crates/ferrink-platform/tests/fixtures/probe-reference-portrait.json"),
        )
        .unwrap();

        let plan = ActiveDisplayPlan::from_json(&plan_input).unwrap();
        let profile = DeviceProfile::from_toml(&profile_input).unwrap();
        let report = ProbeReport::from_json(&report_input).unwrap();
        plan.validate_against(&profile, &report).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn device_tree_inputs_are_rejected_before_reading() {
        let error = read_regular_file("report", Path::new("/dev/null")).unwrap_err();
        assert!(error.to_string().contains("device or kernel tree"));
    }
}
