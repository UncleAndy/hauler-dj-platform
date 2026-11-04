//! Collection and diagnostics CLI (A08): import, analyse, inspect schema versions.
//!
//! Status: **scaffold**. The command set lands with the SQLite collection; today the
//! binary only answers --help/--version so that CI can build and smoke-test it.

use std::process::ExitCode;

const USAGE: &str = "\
dj-cli — collection and diagnostics commands (not implemented yet)

Usage:
  dj-cli --help
  dj-cli --version
  dj-cli import <path> ...        (planned, A08)
  dj-cli analyse <track_id> ...   (planned, A08)

Exit codes: 0 success, 2 not implemented.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("dj-cli {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    eprintln!("dj-cli: the collection commands are not implemented yet (backlog task A08).");
    eprint!("{USAGE}");
    ExitCode::from(2)
}
