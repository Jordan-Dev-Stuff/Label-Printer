//! `tspl-rs` — command line front end for the driverless print path.
//!
//! Kept dependency-free so it builds and runs before any UI toolchain is
//! installed. See `docs/cli.md` for the walkthrough.

use std::process::ExitCode;

use tspl_core::discover::{best_printer, discover, DiscoveredPrinter};
use tspl_core::{encode, parse_target, Config, Frame, DEFAULT_DPI};

const USAGE: &str = "\
tspl-rs — driverless TSPL label printing

USAGE:
  tspl-rs list [--all]
  tspl-rs print --target <T> [options]
  tspl-rs send  --target <T> --job <FILE>

TARGETS:
  raw:PRINTER     submit a RAW job to a spooler queue
  usb:PATH        write bytes straight to a USB printer (no queue)
  serial:PORT     write bytes to a serial port or device node
  file:PATH       write the job to a file (no printer needed)

OPTIONS (print):
  --pattern <name>   testcard (default) or checker
  --pgm <file>       read a binary PGM (P5) image instead of a pattern
  --width <mm>       label width in millimetres (default 101.6)
  --height <mm>      label height in millimetres (default 152.4)
  --dpi <n>          printer resolution (default 203)
  --density <n>      print darkness 1-15 (default 8)
  --speed <n>        print speed 1-12 (default 4)
  --gap <mm>         gap between labels, 0 = continuous (default 3)
  --copies <n>       copies to print (default 1)
  --compact          send only the inked parts of the label (smaller job)
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let Some(command) = args.first() else {
        print!("{USAGE}");
        return Ok(());
    };

    match command.as_str() {
        "list" => {
            cmd_list(&args[1..]);
            Ok(())
        }
        "print" => cmd_print(&args[1..]),
        "send" => cmd_send(&args[1..]),
        "help" | "--help" | "-h" => {
            print!("{USAGE}");
            Ok(())
        }
        other => Err(format!("unknown command {other:?}\n\n{USAGE}")),
    }
}

/// `list` — enumerate queues, USB printers and serial ports.
///
/// `--all` also shows virtual queues, ports that are not printers and the
/// direct-USB twin of a queue.
fn cmd_list(args: &[String]) {
    let show_all = args.iter().any(|arg| arg == "--all");
    let discovery = discover();
    let preferred = best_printer(&discovery.printers, None).map(DiscoveredPrinter::target_spec);

    let shown: Vec<&DiscoveredPrinter> = discovery
        .printers
        .iter()
        .filter(|printer| show_all || !printer.noise)
        .collect();

    if shown.is_empty() {
        println!("no label printers found (try `tspl-rs list --all`)");
    }
    for printer in &shown {
        let spec = printer.target_spec();
        let marker = if preferred.as_deref() == Some(spec.as_str()) {
            "*"
        } else {
            " "
        };
        let state = if printer.online { "" } else { "  [offline]" };
        println!(
            "{marker} {:<11} {}{state}",
            printer.kind.label(),
            printer.name
        );
        println!("    {spec}");
        if !printer.detail.is_empty() {
            println!("    {}", printer.detail);
        }
    }

    let hidden = discovery.printers.len() - shown.len();
    if hidden > 0 {
        println!("({hidden} more hidden: virtual queues and non-printer ports; --all shows them)");
    }
    if preferred.is_some() {
        println!("* = chosen automatically by the UI");
    }

    for warning in &discovery.warnings {
        eprintln!("warning: {warning}");
    }
}

/// `print` — render a pattern or PGM, encode it and send it.
fn cmd_print(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args)?;
    let target_spec = required(&flags, "target")?;
    let target = parse_target(&target_spec).map_err(|error| error.to_string())?;

    let config = Config {
        width_mm: number(&flags, "width", 101.6)?,
        height_mm: number(&flags, "height", 152.4)?,
        gap_mm: number(&flags, "gap", tspl_core::DEFAULT_GAP_MM)?,
        dpi: integer(&flags, "dpi", DEFAULT_DPI)?,
        density: integer(&flags, "density", 8)?,
        speed: integer(&flags, "speed", 4)?,
        copies: integer(&flags, "copies", 1)?,
        skip_blank: flags.iter().any(|(name, _)| name == "compact"),
    };
    let config = config.normalized();
    config.validate().map_err(|error| error.to_string())?;

    let width_px = mm_to_px(config.width_mm, config.dpi);
    let height_px = mm_to_px(config.height_mm, config.dpi);

    let frame = if let Some(path) = flags_value(&flags, "pgm") {
        read_pgm(&path)?
    } else {
        match flags_value(&flags, "pattern").as_deref() {
            None | Some("testcard") => {
                Frame::test_card(width_px, height_px).map_err(|error| error.to_string())?
            }
            Some("checker") => {
                Frame::checkerboard(width_px, height_px, 16).map_err(|error| error.to_string())?
            }
            Some(other) => {
                return Err(format!(
                    "unknown pattern {other:?}, want testcard or checker"
                ))
            }
        }
    };

    let frame = frame.dither();
    let job = encode(&frame, &config).map_err(|error| error.to_string())?;

    println!(
        "encoding {}x{} px ({:.1}x{:.1} mm) -> {} bytes for {}",
        frame.width(),
        frame.height(),
        config.width_mm,
        config.height_mm,
        job.len(),
        target.describe()
    );

    target.send(&job).map_err(|error| error.to_string())?;
    println!("sent: {}", target.describe());

    Ok(())
}

/// `send` — deliver an already encoded job file.
fn cmd_send(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args)?;
    let target_spec = required(&flags, "target")?;
    let job_path = required(&flags, "job")?;

    let target = parse_target(&target_spec).map_err(|error| error.to_string())?;
    let job = std::fs::read(&job_path).map_err(|error| format!("read {job_path}: {error}"))?;

    target.send(&job).map_err(|error| error.to_string())?;
    println!("sent {} bytes: {}", job.len(), target.describe());

    Ok(())
}

/// Parses `--key value` pairs; unknown flags are rejected.
fn parse_flags(args: &[String]) -> Result<Vec<(String, String)>, String> {
    const KNOWN: [&str; 10] = [
        "target", "pattern", "pgm", "width", "height", "dpi", "density", "speed", "gap", "copies",
    ];

    let mut parsed = Vec::new();
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];
        let Some(name) = arg.strip_prefix("--") else {
            return Err(format!("unexpected argument {arg:?}"));
        };
        if name == "compact" {
            parsed.push((name.to_string(), String::new()));
            index += 1;
            continue;
        }
        if !KNOWN.contains(&name) {
            return Err(format!("unknown flag --{name}"));
        }
        let Some(value) = args.get(index + 1) else {
            return Err(format!("flag --{name} needs a value"));
        };
        parsed.push((name.to_string(), value.clone()));
        index += 2;
    }

    Ok(parsed)
}

fn flags_value(flags: &[(String, String)], name: &str) -> Option<String> {
    flags
        .iter()
        .rev()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
}

fn required(flags: &[(String, String)], name: &str) -> Result<String, String> {
    flags_value(flags, name).ok_or_else(|| format!("missing required flag --{name}"))
}

fn number(flags: &[(String, String)], name: &str, default: f64) -> Result<f64, String> {
    match flags_value(flags, name) {
        None => Ok(default),
        Some(value) => value
            .parse::<f64>()
            .map_err(|error| format!("--{name} {value:?}: {error}")),
    }
}

fn integer<T>(flags: &[(String, String)], name: &str, default: T) -> Result<T, String>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match flags_value(flags, name) {
        None => Ok(default),
        Some(value) => value
            .parse::<T>()
            .map_err(|error| format!("--{name} {value:?}: {error}")),
    }
}

/// Converts millimetres to dots, rounding to nearest.
// The caller has already validated the geometry against the 108 mm head, so the
// value is small and non-negative; the casts cannot mangle a meaningful digit.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn mm_to_px(mm: f64, dpi: u32) -> u32 {
    let px = (mm * f64::from(dpi) / 25.4).round();
    if px < 1.0 {
        1
    } else {
        px as u32
    }
}

/// Reads a binary PGM (`P5`) image as a grayscale frame.
fn read_pgm(path: &str) -> Result<Frame, String> {
    let data = std::fs::read(path).map_err(|error| format!("read {path}: {error}"))?;

    let mut cursor = 0_usize;
    let mut token = || -> Result<String, String> {
        // Skip whitespace and comments.
        loop {
            while cursor < data.len() && data[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            if cursor < data.len() && data[cursor] == b'#' {
                while cursor < data.len() && data[cursor] != b'\n' {
                    cursor += 1;
                }
            } else {
                break;
            }
        }

        let start = cursor;
        while cursor < data.len() && !data[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if start == cursor {
            return Err("unexpected end of PGM header".to_string());
        }
        String::from_utf8(data[start..cursor].to_vec()).map_err(|error| error.to_string())
    };

    if token()? != "P5" {
        return Err("only binary PGM (P5) is supported".to_string());
    }
    let width: u32 = token()?
        .parse()
        .map_err(|error| format!("PGM width: {error}"))?;
    let height: u32 = token()?
        .parse()
        .map_err(|error| format!("PGM height: {error}"))?;
    let maxval: u32 = token()?
        .parse()
        .map_err(|error| format!("PGM maxval: {error}"))?;
    if maxval != 255 {
        return Err(format!("only 8-bit PGM is supported, got maxval {maxval}"));
    }

    // Exactly one whitespace byte separates the header from the raster.
    cursor += 1;
    let expected = width as usize * height as usize;
    let pixels = &data[cursor.min(data.len())..];
    if pixels.len() < expected {
        return Err(format!(
            "PGM truncated: {} bytes of raster, need {expected}",
            pixels.len()
        ));
    }

    Frame::from_gray(width, height, pixels[..expected].to_vec()).map_err(|error| error.to_string())
}
