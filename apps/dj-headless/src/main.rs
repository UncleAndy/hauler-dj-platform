//! Headless runner (A01): renders scripted command scenarios to PCM/WAV so every
//! audio feature can be checked without Qt or a sound card.
//!
//! Status: **scaffold**. Argument parsing and the scenario format are fixed by A01;
//! until then the binary reports that it is not implemented instead of pretending to
//! render anything (DJP-ARCH-001, DJP-NFR-001).

use std::process::ExitCode;

const USAGE: &str = "\
dj-headless — offline renderer and scenario runner (not implemented yet)

Usage:
  dj-headless --help
  dj-headless --version
  dj-headless --scenario <file.yaml> --out <out.wav>   (planned, A01)

Exit codes: 0 success, 2 not implemented.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("dj-headless {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    eprintln!("dj-headless: offline render is not implemented yet (backlog task A01).");
    eprint!("{USAGE}");
    ExitCode::from(2)
}
