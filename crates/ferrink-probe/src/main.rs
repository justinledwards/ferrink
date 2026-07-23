mod collector;
mod elf;

use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

#[derive(Debug)]
struct Arguments {
    output: Option<PathBuf>,
    root: PathBuf,
    pretty: bool,
}

#[derive(Debug)]
enum ParseResult {
    Run(Arguments),
    Help,
}

fn main() {
    match run() {
        Ok(()) => {}
        Err(error) => {
            eprintln!("ferrink-probe: {error}");
            std::process::exit(2);
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = match parse_arguments(std::env::args_os().skip(1))? {
        ParseResult::Run(arguments) => arguments,
        ParseResult::Help => {
            print_help();
            return Ok(());
        }
    };

    let report = collector::collect(&arguments.root);
    if let Err(errors) = report.validate() {
        return Err(format!("refusing to write invalid report: {}", errors.join("; ")).into());
    }

    match arguments.output {
        Some(path) => {
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            let mut writer = BufWriter::new(file);
            write_report(&mut writer, &report, arguments.pretty)?;
            writer.flush()?;
            writer.get_ref().sync_all()?;
        }
        None => {
            let stdout = std::io::stdout();
            let mut writer = stdout.lock();
            write_report(&mut writer, &report, arguments.pretty)?;
            writer.flush()?;
        }
    }
    Ok(())
}

fn write_report(
    writer: &mut impl Write,
    report: &ferrink_platform::ProbeReport,
    pretty: bool,
) -> Result<(), serde_json::Error> {
    if pretty {
        serde_json::to_writer_pretty(&mut *writer, report)?;
    } else {
        serde_json::to_writer(&mut *writer, report)?;
    }
    writer.write_all(b"\n").map_err(serde_json::Error::io)
}

fn parse_arguments(
    mut arguments: impl Iterator<Item = std::ffi::OsString>,
) -> Result<ParseResult, String> {
    let mut output = None;
    let mut root = PathBuf::from("/");
    let mut pretty = true;
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--output") => {
                if output.is_some() {
                    return Err("--output may be supplied only once".to_owned());
                }
                output = Some(PathBuf::from(
                    arguments
                        .next()
                        .ok_or_else(|| "--output requires a path".to_owned())?,
                ));
            }
            Some("--root") => {
                root = PathBuf::from(
                    arguments
                        .next()
                        .ok_or_else(|| "--root requires a path".to_owned())?,
                );
            }
            Some("--compact") => pretty = false,
            Some("-h" | "--help") => return Ok(ParseResult::Help),
            Some(other) => return Err(format!("unknown argument: {other}")),
            None => return Err("arguments must be valid Unicode".to_owned()),
        }
    }
    if !root.is_dir() {
        return Err(format!("probe root is not a directory: {}", root.display()));
    }
    Ok(ParseResult::Run(Arguments {
        output,
        root,
        pretty,
    }))
}

fn print_help() {
    println!(
        "ferrink-probe {}\n\n\
         Read-only, redacted device inventory. Without --output, JSON is written\n\
         to stdout. An output file is created only when explicitly named and is\n\
         never overwritten.\n\n\
         Usage: ferrink-probe [--output PATH] [--compact] [--root PATH]",
        env!("CARGO_PKG_VERSION")
    );
}

#[cfg(test)]
mod tests {
    use super::{ParseResult, parse_arguments};

    #[test]
    fn output_is_optional_and_redaction_has_no_disable_flag() {
        let parsed = parse_arguments(Vec::<std::ffi::OsString>::new().into_iter()).unwrap();
        let ParseResult::Run(arguments) = parsed else {
            panic!("expected runnable arguments");
        };
        assert!(arguments.output.is_none());
        assert!(arguments.pretty);
    }

    #[test]
    fn unknown_flags_are_rejected() {
        let error = parse_arguments(["--unredacted".into()].into_iter()).unwrap_err();
        assert!(error.contains("unknown argument"));
    }
}
