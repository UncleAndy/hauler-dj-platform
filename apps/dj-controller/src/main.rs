//! The controller service.
//!
//! A separate process on purpose (ADR-0006): USB work must not share a thread with the audio callback, and restarting
//! the window must not cost the controller. It reads; it does not write. The screens and the motors are later, and the
//! motors are behind a decision that has not been made.
//!
//! `--replay` closes the other half of the path: a file of control events goes through the binding table into the
//! engine, as the same commands a hand would send. That is how the path is tested without a device — the driver's own
//! bytes are the only part a controller adds.
//!
//! **Off by default.** The setting `controller.enabled` in `config.toml` decides whether the service does anything at
//! all; while it is off — which is what a fresh install has — this program says so and exits successfully, so the
//! program behaves exactly as it did before controllers existed.
//!
//! ```text
//! dj-controller --status              what the setting says and what is plugged in
//! dj-controller --list                every HID node this system has, marking ours
//! dj-controller --capture out.txt     read reports into a file, nothing sent to the device
//! dj-controller --replay events.txt --engine <endpoint>
//!                                     turn control events into engine commands
//! ```
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

use std::fs::File;
use std::io::{BufWriter, Write};
use std::process::ExitCode;

use dj_bridge::{ClientMessage, ServerMessage};
use dj_controller_core::{ControlEvent, Driver, Mapping};
use dj_controller_s4mk3::{NAME, PRODUCT, S4Mk3, VENDOR};
use dj_hid::{DeviceInfo, HidrawTransport, Transport};

const USAGE: &str = "dj-controller --status | --list | --capture <file> [--reports <n>] | --replay <file> [--engine <endpoint>]";

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match run(&arguments) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("dj-controller: {error}");
            ExitCode::from(2)
        }
    }
}

fn run(arguments: &[String]) -> Result<(), String> {
    let mut mode = Mode::Status;
    let mut reports = 200_usize;
    let mut engine: Option<String> = None;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--status" => mode = Mode::Status,
            "--list" => mode = Mode::List,
            "--replay" => {
                let Some(path) = arguments.get(index + 1) else {
                    return Err(format!("--replay wants a file name. {USAGE}"));
                };
                mode = Mode::Replay(path.clone());
                index += 1;
            }
            "--engine" => {
                let Some(endpoint) = arguments.get(index + 1) else {
                    return Err(format!("--engine wants an endpoint. {USAGE}"));
                };
                engine = Some(endpoint.clone());
                index += 1;
            }
            "--capture" => {
                let Some(path) = arguments.get(index + 1) else {
                    return Err(format!("--capture wants a file name. {USAGE}"));
                };
                mode = Mode::Capture(path.clone());
                index += 1;
            }
            "--reports" => {
                let Some(count) = arguments.get(index + 1) else {
                    return Err(format!("--reports wants a number. {USAGE}"));
                };
                reports = count
                    .parse()
                    .map_err(|_| format!("--reports wants a number, got {count}"))?;
                index += 1;
            }
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(());
            }
            other => return Err(format!("unknown argument: {other}. {USAGE}")),
        }
        index += 1;
    }

    let (config, file) = dj_config::Config::load_or_default(None)
        .map_err(|error| format!("the settings file cannot be read: {error}"))?;
    if !config.controller.enabled {
        // The important line: with the setting off, nothing is opened and nothing is started.
        println!(
            "dj-controller: controller support is off in {}; nothing to do",
            file.map_or_else(
                || "the settings".to_string(),
                |path| path.display().to_string()
            )
        );
        return Ok(());
    }

    let transport = HidrawTransport::new();
    let devices = transport
        .enumerate()
        .map_err(|error| format!("the HID nodes cannot be listed: {error}"))?;
    match mode {
        Mode::Status => status(&devices),
        Mode::List => {
            list(&devices);
            Ok(())
        }
        Mode::Capture(path) => capture(&transport, &devices, &path, reports),
        Mode::Replay(path) => replay(&path, engine.as_deref()),
    }
}

#[derive(Debug, Clone)]
enum Mode {
    Status,
    List,
    Capture(String),
    Replay(String),
}

fn status(devices: &[DeviceInfo]) -> Result<(), String> {
    let ours: Vec<&DeviceInfo> = devices
        .iter()
        .filter(|device| device.is(VENDOR, PRODUCT))
        .collect();
    println!(
        "dj-controller: controller support is on; {} HID nodes, {} of them {NAME}",
        devices.len(),
        ours.len()
    );
    for device in &ours {
        println!("  {} ({})", device.path.display(), device.name);
    }
    if ours.is_empty() {
        println!("  nothing that looks like a {NAME} is plugged in; the driver stays idle");
    }
    Ok(())
}

fn list(devices: &[DeviceInfo]) {
    for device in devices {
        let mark = if device.is(VENDOR, PRODUCT) { "*" } else { " " };
        println!(
            "{mark} {} {:04x}:{:04x} {}",
            device.path.display(),
            device.vendor,
            device.product,
            device.name
        );
    }
}

fn capture(
    transport: &HidrawTransport,
    devices: &[DeviceInfo],
    path: &str,
    reports: usize,
) -> Result<(), String> {
    let Some(device) = devices.iter().find(|device| device.is(VENDOR, PRODUCT)) else {
        return Err(format!(
            "no {NAME} is plugged in, so there is nothing to capture"
        ));
    };
    let mut source = transport.open(device).map_err(|error| {
        format!(
            "{} cannot be opened for reading: {error}",
            device.path.display()
        )
    })?;
    let file = File::create(path).map_err(|error| format!("{path} cannot be written: {error}"))?;
    let mut out = BufWriter::new(file);
    let mut driver = S4Mk3::new();
    let mut buffer = [0_u8; 128];
    let mut taken = 0_usize;
    writeln!(
        out,
        "# reports from {} ({}), read only",
        device.path.display(),
        device.name
    )
    .map_err(|error| format!("{path} cannot be written: {error}"))?;
    while taken < reports {
        let read = match source.read_report(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(format!("reading {} failed: {error}", device.path.display())),
        };
        let report = &buffer[..read];
        let events = driver.feed(report);
        let bytes: Vec<String> = report.iter().map(|byte| format!("{byte:02x}")).collect();
        write!(out, "{}", bytes.join(" "))
            .map_err(|error| format!("{path} cannot be written: {error}"))?;
        if !events.is_empty() {
            let described: Vec<String> = events
                .iter()
                .map(|event| format!("{}={:.3}", event.control, event.value))
                .collect();
            write!(out, "   {}", described.join(" "))
                .map_err(|error| format!("{path} cannot be written: {error}"))?;
        }
        writeln!(out).map_err(|error| format!("{path} cannot be written: {error}"))?;
        taken += 1;
    }
    out.flush()
        .map_err(|error| format!("{path} cannot be written: {error}"))?;
    println!("dj-controller: {taken} reports written to {path}; nothing was sent to the device");
    Ok(())
}

/// Turns a file of control events into engine commands.
///
/// The file is what a capture records, annotated: `deck_a.play=1` on a line, `#` for a comment. Going through the same
/// binding table the driver feeds is the point — a test of this path proves the path, and the only thing a device adds
/// later is the bytes that produce the same events.
fn replay(path: &str, endpoint: Option<&str>) -> Result<(), String> {
    let text =
        std::fs::read_to_string(path).map_err(|error| format!("{path} cannot be read: {error}"))?;
    let mapping = Mapping::new();
    let mut commands = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((control, value)) = line.split_once('=') else {
            return Err(format!(
                "{}:{}: wanted control=value, got {line}",
                number + 1,
                path
            ));
        };
        let value: f32 = value
            .trim()
            .parse()
            .map_err(|_| format!("{}:{}: {value} is not a number", number + 1, path))?;
        let event = ControlEvent::level(control.trim(), value);
        match mapping.resolve(&event, false) {
            Some((deck, command)) => commands.push((control.trim().to_string(), deck, command)),
            None => println!(
                "dj-controller: {} asks for nothing the engine knows ({control})",
                line
            ),
        }
    }
    let Some(endpoint) = endpoint else {
        for (control, _, command) in &commands {
            println!("dj-controller: {control} would send {command:?} (no --engine, nothing sent)");
        }
        return Ok(());
    };
    send_commands(endpoint, &commands)
}

/// Sends the commands to the engine and waits for their answers.
fn send_commands(
    endpoint: &str,
    commands: &[(String, dj_domain::DeckId, dj_domain::DjCommand)],
) -> Result<(), String> {
    let mut stream = dj_bridge::transport::connect(std::path::Path::new(endpoint))
        .map_err(|error| format!("{endpoint} cannot be reached: {error}"))?;
    dj_bridge::write_frame(
        &mut stream,
        &ClientMessage::Hello {
            protocol: dj_domain::COMMAND_PROTOCOL.to_string(),
        },
    )
    .map_err(|error| format!("the engine refused the handshake: {error}"))?;
    let mut ids = Vec::new();
    for (index, (_, _, command)) in commands.iter().enumerate() {
        let id = u64::try_from(index + 1).unwrap_or(1);
        ids.push(id);
        let scheduled = dj_domain::ScheduledCommand {
            id,
            // No clock of our own: the engine applies it at the next block, which is where a hand's command lands too.
            effective_sample: 0,
            command: command.clone(),
        };
        dj_bridge::write_frame(&mut stream, &ClientMessage::Command(scheduled))
            .map_err(|error| format!("the command could not be sent: {error}"))?;
    }
    let mut answered = Vec::new();
    while answered.len() < ids.len() {
        let message: ServerMessage = dj_bridge::read_frame(&mut stream).map_err(|error| {
            format!(
                "the engine stopped answering after {} answers: {error}",
                answered.len()
            )
        })?;
        if let ServerMessage::Ack(ack) = message {
            if !ids.contains(&ack.command_id) {
                continue;
            }
            let (control, _, _) = commands
                .iter()
                .zip(ids.iter())
                .find(|(_, id)| **id == ack.command_id)
                .map(|(entry, _)| entry.clone())
                .ok_or("an answer with no question")?;
            println!("dj-controller: {control} → {:?}", ack.status);
            answered.push(ack.command_id);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_is_not_an_error() {
        assert!(run(&["--help".to_string()]).is_ok());
    }

    #[test]
    fn an_unknown_argument_is_refused_by_name() {
        let refused = run(&["--nonsense".to_string()]);
        assert!(refused.is_err());
        if let Err(text) = refused {
            assert!(text.contains("--nonsense"), "{text}");
        }
    }

    #[test]
    fn capture_without_a_file_is_refused() {
        assert!(run(&["--capture".to_string()]).is_err());
    }

    #[test]
    fn a_report_count_must_be_a_number() {
        assert!(run(&["--reports".to_string(), "many".to_string()]).is_err());
    }

    #[test]
    fn a_replay_line_that_is_not_a_pair_is_refused() {
        let dir = std::env::temp_dir().join("dj-controller-replay-test");
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("bad.txt");
        let _ = std::fs::write(&file, "deck_a.play\n");
        let refused = replay(&file.to_string_lossy(), None);
        assert!(refused.is_err());
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn a_replay_without_an_engine_sends_nothing() {
        let dir = std::env::temp_dir().join("dj-controller-replay-test");
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("quiet.txt");
        let _ = std::fs::write(&file, "# a comment\ndeck_a.play=1\nwheel_left=1.5\n");
        // No endpoint: the commands are printed, not sent, and an event the table does not know is said out loud.
        assert!(replay(&file.to_string_lossy(), None).is_ok());
        let _ = std::fs::remove_file(&file);
    }
}
