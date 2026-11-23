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
use dj_hid::write::ReportSink;
use dj_hid::{DeviceInfo, HidrawTransport, Transport};

const USAGE: &str = "dj-controller --status | --list | --capture <file> [--reports <n>] | --replay <file> [--engine <endpoint>] | --screen-preview <file.ppm> [--panel left|right] | --lights-sweep [--from N] [--to M] [--colour C] [--interval MS] [--body-only] [--send] | --lights-map <file> [--from N] [--to M] [--colour C] [--send] | --lights-colours <byte> --colours 4,8,12 [--interval MS] [--send] | --lights-demo [--interval MS] [--send] | --lights-show --engine <endpoint> | --lights-fill <code> [--send] | --lights-night [--rest <code>] [--send] | --lights-bright <byte> [--colour N] [--rounds N] [--send] | --lights-colour-walk <byte> [--shift] [--interval MS] [--send]";

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
    let mut sweep_from = 0usize;
    let mut sweep_to = dj_controller_s4mk3::lights::LIGHTS_LENGTH - 1;
    let mut sweep_colour = 4u8;
    let mut sweep_interval_ms = 800u64;
    let mut sweep_body_only = false;
    let mut sweep_send = false;
    let mut sweep_colours: Vec<u8> = vec![4];
    let mut rest_colour: Option<dj_controller_s4mk3::Colour> = None;
    let mut night = false;
    let mut rounds = 3usize;
    let mut shifted = false;
    let mut pick_out: Option<String> = None;
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
            "--lights-demo" => mode = Mode::LightsDemo,
            "--lights-show" => mode = Mode::LightsShow,
            "--lights-night" => {
                night = true;
                mode = Mode::LightsShow;
            }
            "--rest" => {
                let Some(name) = arguments.get(index + 1) else {
                    return Err(format!("--rest wants a colour name. {USAGE}"));
                };
                rest_colour = Some(
                    dj_controller_s4mk3::lights::colour_by_name(name)
                        .ok_or_else(|| format!("--rest: no colour called {name:?}"))?,
                );
                index += 1;
            }
            "--rounds" => {
                rounds = arguments
                    .get(index + 1)
                    .and_then(|value| value.parse().ok())
                    .ok_or_else(|| "--rounds wants a number".to_string())?;
                index += 1;
            }
            "--lights-colour-walk" => {
                // The word after the flag is optional: no argument means the whole panel, which is how a button that
                // ignores colour is spotted — it stays as it is while everything around it changes.
                let what = match arguments.get(index + 1).map(String::as_str) {
                    Some(value)
                        if value == "left"
                            || value == "right"
                            || value == "all"
                            || value.parse::<u8>().is_ok() =>
                    {
                        index += 1;
                        value.to_string()
                    }
                    _ => "all".to_string(),
                };
                mode = Mode::LightsColourWalk(what);
            }
            "--shift" => shifted = true,
            "--lights-pick" => {
                let what = match arguments.get(index + 1).map(String::as_str) {
                    Some(value)
                        if value == "left"
                            || value == "right"
                            || value == "all"
                            || value.parse::<u8>().is_ok() =>
                    {
                        index += 1;
                        value.to_string()
                    }
                    _ => "all".to_string(),
                };
                mode = Mode::LightsPick(what);
            }
            "--out" => {
                let Some(path) = arguments.get(index + 1) else {
                    return Err(format!("--out wants a file name. {USAGE}"));
                };
                pick_out = Some(path.clone());
                index += 1;
            }
            "--lights-bright" => {
                let Some(byte) = arguments.get(index + 1) else {
                    return Err(format!("--lights-bright wants a byte number. {USAGE}"));
                };
                let byte: u8 = byte
                    .parse()
                    .map_err(|_| format!("--lights-bright wants a byte number, got {byte}"))?;
                mode = Mode::LightsBright(byte);
                index += 1;
            }
            "--lights-fill" => {
                let Some(code) = arguments.get(index + 1) else {
                    return Err(format!("--lights-fill wants a code. {USAGE}"));
                };
                let code: u8 = code
                    .parse()
                    .map_err(|_| format!("--lights-fill wants one byte, got {code}"))?;
                mode = Mode::LightsFill(code);
                index += 1;
            }
            "--lights-map" => {
                let Some(path) = arguments.get(index + 1) else {
                    return Err(format!("--lights-map wants a file name. {USAGE}"));
                };
                mode = Mode::LightsMap(path.clone());
                index += 1;
            }
            "--lights-colours" => {
                let Some(byte) = arguments.get(index + 1) else {
                    return Err(format!("--lights-colours wants a byte number. {USAGE}"));
                };
                let byte: usize = byte
                    .parse()
                    .map_err(|_| format!("--lights-colours wants a byte number, got {byte}"))?;
                mode = Mode::LightsColours(byte);
                index += 1;
            }
            "--colours" => {
                let Some(list) = arguments.get(index + 1) else {
                    return Err(format!("--colours wants a list like 4,8,12. {USAGE}"));
                };
                sweep_colours = dj_controller_s4mk3::lights::colours(list)?;
                index += 1;
            }
            // The sweep's own parameters are flags of their own: a person types them in whatever order reads well, and
            // an option that only works after one particular command is a trap (`--lights-map map.txt --from 0` used to
            // fail on `--from`).
            "--from" => {
                sweep_from = arguments
                    .get(index + 1)
                    .and_then(|value| value.parse().ok())
                    .ok_or_else(|| "--from wants a byte number".to_string())?;
                index += 1;
            }
            "--to" => {
                sweep_to = arguments
                    .get(index + 1)
                    .and_then(|value| value.parse().ok())
                    .ok_or_else(|| "--to wants a byte number".to_string())?;
                index += 1;
            }
            "--colour" => {
                sweep_colour = arguments
                    .get(index + 1)
                    .and_then(|value| value.parse().ok())
                    .ok_or_else(|| "--colour wants a colour code".to_string())?;
                index += 1;
            }
            "--interval" => {
                sweep_interval_ms = arguments
                    .get(index + 1)
                    .and_then(|value| value.parse().ok())
                    .ok_or_else(|| "--interval wants milliseconds".to_string())?;
                index += 1;
            }
            "--body-only" => sweep_body_only = true,
            "--send" => sweep_send = true,
            "--lights-sweep" => mode = Mode::LightsSweep,
            "--screen-preview" => {
                let Some(path) = arguments.get(index + 1) else {
                    return Err(format!("--screen-preview wants a file name. {USAGE}"));
                };
                let mut panel = dj_controller_s4mk3::Screen::Left;
                let mut step = 1;
                if arguments.get(index + 2).map(String::as_str) == Some("--panel") {
                    panel = match arguments.get(index + 3).map(String::as_str) {
                        Some("right") => dj_controller_s4mk3::Screen::Right,
                        Some("left") | None => dj_controller_s4mk3::Screen::Left,
                        Some(other) => {
                            return Err(format!("--panel wants left or right, got {other}"));
                        }
                    };
                    step = 3;
                }
                mode = Mode::ScreenPreview(path.clone(), panel);
                index += step;
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

    // Drawing a screen needs neither the device nor the switch: it is how the design is looked at before anything is
    // sent, so it answers even while controller support is off.
    if let Mode::ScreenPreview(path, panel) = &mode {
        return screen_preview(path, *panel);
    }
    // A dry sweep prints the frames it would send: that needs neither the switch nor a node, and it is how the sweep is
    // tested. Sending is a write to a device, so that half waits for the switch like everything else.
    let dry_sweep = Sweep {
        from: sweep_from,
        to: sweep_to,
        colour: sweep_colour,
        colours: sweep_colours.clone(),
        interval_ms: sweep_interval_ms,
        body_only: sweep_body_only,
    };
    if matches!(mode, Mode::LightsSweep) && !sweep_send {
        return lights_sweep(&[], &dry_sweep, false);
    }
    // The map has a dry half too: it prints the bytes it would light, which is how it is checked without a device.
    if let Mode::LightsFill(code) = mode
        && !sweep_send
    {
        return lights_fill(&[], code, false, sweep_body_only);
    }
    if let Mode::LightsPick(what) = mode.clone()
        && !sweep_send
    {
        return lights_pick(
            &[],
            &what,
            pick_out.as_deref(),
            sweep_interval_ms,
            false,
            sweep_body_only,
        );
    }
    if let Mode::LightsColourWalk(what) = mode.clone()
        && !sweep_send
    {
        return lights_colour_walk(
            &[],
            &what,
            shifted,
            sweep_interval_ms,
            false,
            sweep_body_only,
        );
    }
    if let Mode::LightsBright(byte) = mode
        && !sweep_send
    {
        return lights_bright(
            &[],
            byte,
            sweep_colour,
            rounds,
            sweep_interval_ms,
            false,
            sweep_body_only,
        );
    }
    if matches!(mode, Mode::LightsDemo) && !sweep_send {
        return lights_demo(&[], &dry_sweep, false);
    }
    if let Mode::LightsMap(path) = &mode
        && !sweep_send
    {
        return lights_map(&[], path, &dry_sweep);
    }

    let sweep = Sweep {
        from: sweep_from,
        to: sweep_to,
        colour: sweep_colour,
        colours: sweep_colours.clone(),
        interval_ms: sweep_interval_ms,
        body_only: sweep_body_only,
    };

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

    // The colours the panel shows are settings: the file decides, and `--rest` overrides one of them for a night of
    // trying colours out.
    let mut palette = palette_from(&config.controller.lights);
    if let Some(resting) = rest_colour {
        palette.resting = resting;
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
        Mode::ScreenPreview(path, panel) => screen_preview(&path, panel),
        Mode::LightsMap(path) => lights_map(&devices, &path, &sweep),
        Mode::LightsColours(byte) => lights_colours(&devices, byte, &sweep),
        Mode::LightsDemo => lights_demo(&devices, &sweep, sweep_send),
        Mode::LightsShow => lights_show(
            &devices,
            engine.as_deref(),
            sweep_body_only,
            night || config.controller.lights.night,
            &palette,
        ),
        Mode::LightsFill(code) => lights_fill(&devices, code, sweep_send, sweep_body_only),
        Mode::LightsPick(what) => lights_pick(
            &devices,
            &what,
            pick_out.as_deref(),
            sweep_interval_ms,
            sweep_send,
            sweep_body_only,
        ),
        Mode::LightsColourWalk(what) => lights_colour_walk(
            &devices,
            &what,
            shifted,
            sweep_interval_ms,
            sweep_send,
            sweep_body_only,
        ),
        Mode::LightsBright(byte) => lights_bright(
            &devices,
            byte,
            sweep_colour,
            rounds,
            sweep_interval_ms,
            sweep_send,
            sweep_body_only,
        ),
        Mode::LightsSweep => lights_sweep(&devices, &sweep, sweep_send),
    }
}

#[derive(Debug, Clone)]
enum Mode {
    Status,
    List,
    Capture(String),
    Replay(String),
    ScreenPreview(String, dj_controller_s4mk3::Screen),
    LightsSweep,
    LightsMap(String),
    LightsColours(usize),
    LightsDemo,
    LightsShow,
    LightsFill(u8),
    LightsBright(u8),
    LightsColourWalk(String),
    LightsPick(String),
}

/// Write one screen as a picture a person can open, and say how big the frame would be.
///
/// The design has to be approved by an eye before it goes to a device: a screen verified only by sending bytes is a
/// screen nobody has seen.
fn screen_preview(path: &str, panel: dj_controller_s4mk3::Screen) -> Result<(), String> {
    let (canvas, frame) = dj_controller_s4mk3::screen::sample_frame(panel)
        .map_err(|error| format!("the frame does not fit the panel: {error:?}"))?;
    std::fs::write(path, canvas.to_ppm()).map_err(|error| format!("{path}: {error}"))?;
    let side = match panel {
        dj_controller_s4mk3::Screen::Left => "left",
        dj_controller_s4mk3::Screen::Right => "right",
    };
    println!(
        "dj-controller: {path}: {}x{} pixels; the same picture is {} bytes as a frame for the {side} panel",
        canvas.width(),
        canvas.height(),
        frame.len()
    );
    Ok(())
}

/// Ask the controller which control each byte of the lights report lights up.
///
/// A sweep is the only honest way to learn the map: the protocol notes name the report and the colours but not the
/// per-control bytes, and a copied table would be somebody else's table. One byte at a time, one control at a time, and
/// a person watches and writes down what lit.
///
/// Without `--send` nothing is sent: the frames are printed, which is what makes this testable without a device and
/// without a `udev` rule.
fn lights_sweep(devices: &[DeviceInfo], sweep: &Sweep, send: bool) -> Result<(), String> {
    let (from, to, colour, interval_ms) = (sweep.from, sweep.to, sweep.colour, sweep.interval_ms);
    let reports = dj_controller_s4mk3::lights::sweep(from, to, colour);
    if reports.is_empty() {
        return Err(format!(
            "nothing to sweep: bytes {from}..{to} of a {} byte report",
            dj_controller_s4mk3::lights::LIGHTS_LENGTH
        ));
    }
    let with_id = !sweep.body_only;
    if !send {
        println!(
            "dj-controller: {} reports, {} bytes each on the wire; nothing is sent without --send",
            reports.len(),
            reports[0].wire(with_id).len()
        );
        for (byte, report) in (from..).zip(reports.iter()) {
            println!(
                "dj-controller: byte {byte}: report {} body {} byte {byte} = {colour}",
                report.id,
                report.body.len()
            );
        }
        return Ok(());
    }

    let ours = devices
        .iter()
        .find(|device| device.is(VENDOR, PRODUCT))
        .ok_or_else(|| format!("no {NAME} on this machine"))?;
    let mut sink = dj_hid::write::NodeSink::open(&ours.path)
        .map_err(|error| format!("the lights need a writable node: {error}"))?;
    println!(
        "dj-controller: sending {} reports to {}, {} ms apart; watch which control lights",
        reports.len(),
        ours.path.display(),
        interval_ms
    );
    for (byte, report) in (from..).zip(reports.iter()) {
        sink.send(&report.wire(with_id))
            .map_err(|error| format!("byte {byte}: {error}"))?;
        println!("dj-controller: byte {byte}");
        std::thread::sleep(std::time::Duration::from_millis(interval_ms));
    }
    // Leave the panel dark rather than with one light on.
    let _ = sink.send(&dj_controller_s4mk3::lights::LightsReport::dark().wire(with_id));
    println!(
        "dj-controller: done; the sweep lights nothing on its own, so a dark panel is the end of it"
    );
    Ok(())
}

/// What a sweep is: which bytes, in which colour, how fast, and whether the identifier goes first.
///
/// A struct rather than a dozen arguments: the callers differ in one or two fields and would otherwise read as a wall of
/// identical positions.
struct Sweep {
    from: usize,
    to: usize,
    colour: u8,
    colours: Vec<u8>,
    interval_ms: u64,
    body_only: bool,
}

/// Walk the bytes and write down what a person sees.
///
/// The map is not something a document can give: it is what the controller shows when one byte is lit. So the sweep
/// asks, one byte at a time, and appends the answer to a file. An empty answer means "nothing lit" and is not written:
/// an absence in the file is how a missing light is recorded.
fn lights_map(devices: &[DeviceInfo], path: &str, sweep: &Sweep) -> Result<(), String> {
    let (from, to, colour, interval_ms) = (sweep.from, sweep.to, sweep.colour, sweep.interval_ms);
    if from > to {
        return Err(format!("the sweep starts at {from} and ends at {to}"));
    }
    let with_id = !sweep.body_only;
    if devices.is_empty() {
        // The dry half: the bytes, in order, and nothing sent. This is what makes the map tool testable without the
        // controller, and what a person can run to see the range before committing to a session.
        println!(
            "dj-controller: would write the map to {path}: bytes {from}..{to} in colour {colour}, {} bytes on the wire; nothing is sent",
            from_to_width(sweep)
        );
        for byte in from..=to {
            println!("dj-controller: byte {byte} — would ask what lit up");
        }
        return Ok(());
    }
    let mut sink = open_lights(devices)?;
    println!(
        "dj-controller: writing the map to {path}. For each byte type what lit up (empty means nothing); \"done\" stops"
    );
    let mut stdin = std::io::BufReader::new(std::io::stdin());
    for byte in from..=to {
        let Some(report) = dj_controller_s4mk3::lights::LightsReport::one(byte, colour) else {
            break;
        };
        sink.send(&report.wire(with_id))
            .map_err(|error| format!("byte {byte}: {error}"))?;
        println!("dj-controller: byte {byte} — what lit up?");
        let mut answer = String::new();
        std::io::BufRead::read_line(&mut stdin, &mut answer)
            .map_err(|error| format!("reading the answer: {error}"))?;
        let answer = answer.trim();
        if answer == "done" {
            break;
        }
        if let Some(line) = dj_controller_s4mk3::lights::map_line(byte, answer) {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .map_err(|error| format!("{path}: {error}"))?;
            file.write_all(line.as_bytes())
                .map_err(|error| format!("{path}: {error}"))?;
        }
        std::thread::sleep(std::time::Duration::from_millis(interval_ms));
    }
    let _ = sink.send(&dj_controller_s4mk3::lights::LightsReport::dark().wire(with_id));
    println!("dj-controller: done; the panel is dark again");
    Ok(())
}

/// One byte in several colours: the question the first sweep cannot answer.
///
/// A byte that looks the same in every colour sets "on", not a colour; a byte whose body changes with the code sets the
/// colour itself; a byte that only lights at some codes is a brightness. A person watching says which it was.
fn lights_colours(devices: &[DeviceInfo], byte: usize, sweep: &Sweep) -> Result<(), String> {
    let with_id = !sweep.body_only;
    let mut sink = open_lights(devices)?;
    for colour in &sweep.colours {
        let report =
            dj_controller_s4mk3::lights::LightsReport::one(byte, *colour).ok_or_else(|| {
                format!(
                    "byte {byte} is outside a {} byte report",
                    dj_controller_s4mk3::lights::LIGHTS_LENGTH
                )
            })?;
        sink.send(&report.wire(with_id))
            .map_err(|error| format!("colour {colour}: {error}"))?;
        println!("dj-controller: byte {byte} = colour {colour}");
        std::thread::sleep(std::time::Duration::from_millis(sweep.interval_ms));
    }
    let _ = sink.send(&dj_controller_s4mk3::lights::LightsReport::dark().wire(with_id));
    println!("dj-controller: done; the panel is dark again");
    Ok(())
}

/// Walk every base colour on one control, in both states.
///
/// A green that changes brightness says nothing about blue: each colour is its own pair of codes, and the table is only
/// worth trusting once a person has seen all eighteen.
fn lights_colour_walk(
    devices: &[DeviceInfo],
    what: &str,
    shifted: bool,
    interval_ms: u64,
    send: bool,
    body_only: bool,
) -> Result<(), String> {
    let bytes = dj_controller_s4mk3::lights::bytes_for(what)?;
    let steps = dj_controller_s4mk3::lights::colour_walk(shifted);
    let with_id = !body_only;
    if !send || devices.is_empty() {
        println!(
            "dj-controller: would walk {} steps over {} bytes ({what}):",
            steps.len(),
            bytes.len()
        );
        for step in steps.iter().take(6) {
            println!(
                "dj-controller: code {} — {} {}",
                step.code, step.colour, step.state
            );
        }
        println!("dj-controller: … и так далее по всей таблице цветов");
        return Ok(());
    }
    let mut sink = open_lights(devices)?;
    for step in &steps {
        let report = dj_controller_s4mk3::lights::fill_only(step.code, &bytes);
        sink.send(&report.wire(with_id))
            .map_err(|error| format!("code {}: {error}", step.code))?;
        println!(
            "dj-controller: code {} — {} {}, {} bytes",
            step.code,
            step.colour,
            step.state,
            bytes.len()
        );
        std::thread::sleep(std::time::Duration::from_millis(interval_ms));
    }
    let _ = sink.send(&dj_controller_s4mk3::lights::LightsReport::dark().wire(with_id));
    println!("dj-controller: done; the panel is dark again");
    Ok(())
}

/// Show every colour in both states and let the person say which one the panel should rest at.
///
/// Choosing a resting colour by looking is the only way that produces a panel somebody wants: the table says "white" and
/// "honey", but whether white is too much in a dark booth is not in the table. Each colour is shown faint and bright, then
/// the walk asks; "y" records it, and the walk carries on so a second candidate can be compared against the first.
fn lights_pick(
    devices: &[DeviceInfo],
    what: &str,
    out: Option<&str>,
    interval_ms: u64,
    send: bool,
    body_only: bool,
) -> Result<(), String> {
    let bytes = dj_controller_s4mk3::lights::bytes_for(what)?;
    let steps = dj_controller_s4mk3::lights::colour_walk(false);
    if !send || devices.is_empty() {
        println!(
            "dj-controller: would show {} colours over {} bytes and ask after each; nothing is sent",
            steps.len() / 2,
            bytes.len()
        );
        for step in steps.iter().step_by(2) {
            println!(
                "dj-controller: {} — faint {} / bright {}",
                step.colour,
                step.code,
                step.code + 2
            );
        }
        return Ok(());
    }

    let mut sink = open_lights(devices)?;
    let with_id = !body_only;
    let mut stdin = std::io::BufReader::new(std::io::stdin());
    let mut chosen: Vec<(u8, &'static str)> = Vec::new();
    for pair in steps.chunks(2) {
        let (faint, bright_step) = (&pair[0], &pair[1]);
        sink.send(&dj_controller_s4mk3::lights::fill_only(faint.code, &bytes).wire(with_id))
            .map_err(|error| format!("{}: {error}", faint.colour))?;
        std::thread::sleep(std::time::Duration::from_millis(interval_ms));
        sink.send(&dj_controller_s4mk3::lights::fill_only(bright_step.code, &bytes).wire(with_id))
            .map_err(|error| format!("{}: {error}", bright_step.colour))?;
        println!(
            "dj-controller: {} (faint {}, bright {}) — взять в фон? [y/N]",
            faint.colour, faint.code, bright_step.code
        );
        let mut answer = String::new();
        std::io::BufRead::read_line(&mut stdin, &mut answer)
            .map_err(|error| format!("reading the answer: {error}"))?;
        if answer.trim().eq_ignore_ascii_case("y") {
            chosen.push((faint.code, faint.colour));
        }
        std::thread::sleep(std::time::Duration::from_millis(interval_ms));
    }
    let _ = sink.send(&dj_controller_s4mk3::lights::LightsReport::dark().wire(with_id));

    if chosen.is_empty() {
        println!("dj-controller: nothing chosen; the panel is dark again");
        return Ok(());
    }
    println!("dj-controller: выбрано:");
    let mut text = String::new();
    for (code, name) in &chosen {
        println!("dj-controller: {code} — {name}");
        if let Some(line) = dj_controller_s4mk3::lights::pick_line(*code, name) {
            text.push_str(&line);
        }
    }
    if let Some(path) = out {
        std::fs::write(path, text).map_err(|error| format!("{path}: {error}"))?;
        println!("dj-controller: записано в {path}");
    } else {
        println!("dj-controller: чтобы это не потерялось, добавьте --out <file>");
    }
    Ok(())
}

/// Check on the device that brightness is an offset to the colour code.
///
/// One control, the same colour twice: base and base + 2. If the panel shows one colour changing its brightness, the
/// encoding is right; if it shows two colours, brightness lives elsewhere and nothing should depend on it yet.
fn lights_bright(
    devices: &[DeviceInfo],
    byte: u8,
    colour: u8,
    rounds: usize,
    interval_ms: u64,
    send: bool,
    body_only: bool,
) -> Result<(), String> {
    let reports = dj_controller_s4mk3::lights::brightness_probe(byte, colour, rounds);
    let with_id = !body_only;
    let faint = dj_controller_s4mk3::lights::dim(colour);
    let strong = dj_controller_s4mk3::lights::bright(colour);
    if !send || devices.is_empty() {
        println!(
            "dj-controller: would alternate byte {byte} between code {faint} and {strong}, {} times; nothing is sent",
            reports.len() / 2
        );
        return Ok(());
    }
    let mut sink = open_lights(devices)?;
    for (index, report) in reports.iter().enumerate() {
        sink.send(&report.wire(with_id))
            .map_err(|error| format!("step {index}: {error}"))?;
        println!(
            "dj-controller: {}",
            if index % 2 == 0 { faint } else { strong }
        );
        std::thread::sleep(std::time::Duration::from_millis(interval_ms));
    }
    let _ = sink.send(&dj_controller_s4mk3::lights::LightsReport::dark().wire(with_id));
    Ok(())
}

/// Put one colour code into every byte of the lights report.
///
/// This is the question about the dark: the owner wants the panel to glow faintly everywhere and light the active
/// control brighter, and the only way to find a faint code is to fill the whole panel with it and look. One report, all
/// ninety-four bytes, one code — a code that is invisible in a lit room may be exactly right in a booth.
fn lights_fill(
    devices: &[DeviceInfo],
    code: u8,
    send: bool,
    body_only: bool,
) -> Result<(), String> {
    let mut report = dj_controller_s4mk3::lights::LightsReport::dark();
    report.body.iter_mut().for_each(|byte| *byte = code);
    let with_id = !body_only;
    if !send || devices.is_empty() {
        println!(
            "dj-controller: would fill all {} bytes with code {code}; {} bytes on the wire; nothing is sent",
            report.body.len(),
            report.wire(with_id).len()
        );
        return Ok(());
    }
    let mut sink = open_lights(devices)?;
    sink.send(&report.wire(with_id))
        .map_err(|error| format!("the panel did not take the fill: {error}"))?;
    println!(
        "dj-controller: all {} bytes are code {code}; look at the panel, then 'dj-controller --lights-fill 0 --send' to clear",
        report.body.len()
    );
    Ok(())
}

/// The colour of a settings value in the driver's own vocabulary.
///
/// One match, in one place: the settings crate names the colours as the device does, and this is where the two meet.
fn colour_of(colour: dj_config::LightColour) -> dj_controller_s4mk3::Colour {
    use dj_config::LightColour as Wanted;
    match colour {
        Wanted::Off => dj_controller_s4mk3::Colour::Off,
        Wanted::Red => dj_controller_s4mk3::Colour::Red,
        Wanted::Carrot => dj_controller_s4mk3::Colour::Carrot,
        Wanted::Orange => dj_controller_s4mk3::Colour::Orange,
        Wanted::Honey => dj_controller_s4mk3::Colour::Honey,
        Wanted::Yellow => dj_controller_s4mk3::Colour::Yellow,
        Wanted::Lime => dj_controller_s4mk3::Colour::Lime,
        Wanted::Green => dj_controller_s4mk3::Colour::Green,
        Wanted::Aqua => dj_controller_s4mk3::Colour::Aqua,
        Wanted::Celeste => dj_controller_s4mk3::Colour::Celeste,
        Wanted::Sky => dj_controller_s4mk3::Colour::Sky,
        Wanted::Blue => dj_controller_s4mk3::Colour::Blue,
        Wanted::Purple => dj_controller_s4mk3::Colour::Purple,
        Wanted::Fuscia => dj_controller_s4mk3::Colour::Fuscia,
        Wanted::Magenta => dj_controller_s4mk3::Colour::Magenta,
        Wanted::Azalea => dj_controller_s4mk3::Colour::Azalea,
        Wanted::Salmon => dj_controller_s4mk3::Colour::Salmon,
        Wanted::White => dj_controller_s4mk3::Colour::White,
    }
}

/// The palette the settings file asks for.
fn palette_from(lights: &dj_config::ControllerLights) -> dj_controller_s4mk3::lights::Palette {
    dj_controller_s4mk3::lights::Palette {
        resting: colour_of(lights.resting),
        play: colour_of(lights.play),
        cue: colour_of(lights.cue),
        sync: colour_of(lights.sync),
        hotcue: colour_of(lights.hotcue),
        fx: colour_of(lights.fx),
    }
}

/// What one deck says about itself, as far as the panel cares.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct DeckFacts {
    playing: bool,
    monitoring: bool,
    sync: bool,
    fx: bool,
    hotcues: [bool; 8],
}

/// The panel's state from four decks' facts.
///
/// Two halves again: hotcues, `PLAY`, `CUE` and sync belong to the decks with platters — the left and right pairs — while
/// the mixer section has an FX button for each of the four decks the program runs. Decks C and D have no platter on this
/// controller, so nothing else of theirs can be lit.
fn show_from_facts(decks: &[DeckFacts; 4]) -> dj_controller_s4mk3::lights::Show {
    dj_controller_s4mk3::lights::Show {
        hotcues: [decks[0].hotcues, decks[1].hotcues],
        playing: [decks[0].playing, decks[1].playing],
        monitoring: [decks[0].monitoring, decks[1].monitoring],
        sync: [decks[0].sync, decks[1].sync],
        fx: [decks[0].fx, decks[1].fx, decks[2].fx, decks[3].fx],
    }
}

/// The same facts, taken from a published snapshot.
fn facts_from(snapshot: &dj_domain::Snapshot) -> [DeckFacts; 4] {
    let mut facts = [DeckFacts::default(); 4];
    for deck in &snapshot.decks {
        let index = deck.deck.index();
        if index >= facts.len() {
            continue;
        }
        let mut hotcues = [false; 8];
        for (slot, cue) in deck.hotcues.iter().take(8).enumerate() {
            hotcues[slot] = cue.is_some();
        }
        facts[index] = DeckFacts {
            playing: deck.playing,
            monitoring: deck.cue,
            sync: !deck.sync.is_off(),
            fx: deck.fx.enabled,
            hotcues,
        };
    }
    facts
}

/// Light the panel from what the engine publishes, as long as the engine is there.
///
/// This is the live half of the lights: the same report the tests build, sent whenever the state actually changed. A
/// report every frame would be a stream of identical bytes; the panel does not need to be told twice that nothing moved.
fn lights_show(
    devices: &[DeviceInfo],
    engine: Option<&str>,
    body_only: bool,
    night: bool,
    palette: &dj_controller_s4mk3::lights::Palette,
) -> Result<(), String> {
    let Some(endpoint) = engine else {
        return Err(format!("--lights-show wants --engine <endpoint>. {USAGE}"));
    };
    // Without a device the show says what it would light: that is the half a person can check without the controller,
    // and the only half a machine with no panel can test at all.
    let mut sink = if devices.is_empty() {
        None
    } else {
        Some(open_lights(devices)?)
    };
    let (mut client, welcome) = dj_bridge::connect_with_timeout(
        std::path::Path::new(endpoint),
        Some(std::time::Duration::from_secs(2)),
    )
    .map_err(|error| format!("no engine at {endpoint}: {error}"))?;
    let _ = welcome;
    let with_id = !body_only;
    let mut last: Option<Vec<u8>> = None;
    println!("dj-controller: lighting the panel from {endpoint}; Ctrl-C stops");
    loop {
        let message = client
            .recv_server()
            .map_err(|error| format!("the engine stopped answering: {error}"))?;
        let dj_bridge::ServerMessage::Snapshot(snapshot) = message else {
            continue;
        };
        let show = show_from_facts(&facts_from(&snapshot));
        // In a dark booth everything glows faintly and the active controls stand out; outside it the panel is dark
        // where nothing happens.
        let report = if night {
            dj_controller_s4mk3::lights::report_at_rest(&show, palette)
        } else {
            dj_controller_s4mk3::lights::report_with(&show, palette)
        };
        let wire = report.wire(with_id);
        if last.as_ref() == Some(&wire) {
            continue;
        }
        match sink.as_mut() {
            Some(sink) => sink
                .send(&wire)
                .map_err(|error| format!("the panel did not take the report: {error}"))?,
            None => {
                let lit: Vec<String> = report
                    .body
                    .iter()
                    .enumerate()
                    .filter(|(_, colour)| **colour != 0)
                    .map(|(byte, colour)| {
                        let name = dj_controller_s4mk3::lights::name_of(byte as u8)
                            .unwrap_or("a byte the map does not name");
                        format!("{byte} {name} ({colour})")
                    })
                    .collect();
                println!("dj-controller: would light {}", lit.join(", "));
            }
        }
        last = Some(wire);
    }
}

/// Light every control the map knows, one at a time.
///
/// This is the check of the map itself: if a control the owner named does not light during the walk, the map is wrong
/// for that byte, and a panel that lies about its own buttons is worse than a dark one. Nothing here needs the engine.
fn lights_demo(devices: &[DeviceInfo], sweep: &Sweep, send: bool) -> Result<(), String> {
    let reports = dj_controller_s4mk3::lights::walk();
    let with_id = !sweep.body_only;
    if !send || devices.is_empty() {
        println!(
            "dj-controller: would walk {} controls, {} ms apart; nothing is sent",
            reports.len(),
            sweep.interval_ms
        );
        for (byte, name) in dj_controller_s4mk3::lights::map() {
            println!("dj-controller: byte {byte} — {name}");
        }
        return Ok(());
    }
    let mut sink = open_lights(devices)?;
    println!(
        "dj-controller: walking {} controls; watch the panel",
        reports.len()
    );
    for ((byte, name), report) in dj_controller_s4mk3::lights::map()
        .iter()
        .zip(reports.iter())
    {
        sink.send(&report.wire(with_id))
            .map_err(|error| format!("byte {byte}: {error}"))?;
        println!("dj-controller: byte {byte} — {name}");
        std::thread::sleep(std::time::Duration::from_millis(sweep.interval_ms));
    }
    let _ = sink.send(&dj_controller_s4mk3::lights::LightsReport::dark().wire(with_id));
    println!("dj-controller: done; the panel is dark again");
    Ok(())
}

/// How many bytes one report takes on the wire.
fn from_to_width(sweep: &Sweep) -> usize {
    dj_controller_s4mk3::lights::LIGHTS_LENGTH + usize::from(!sweep.body_only)
}

/// The node the lights go to, with the message a person needs when the rules do not allow it.
fn open_lights(devices: &[DeviceInfo]) -> Result<dj_hid::write::NodeSink, String> {
    let ours = devices
        .iter()
        .find(|device| device.is(VENDOR, PRODUCT))
        .ok_or_else(|| format!("no {NAME} on this machine"))?;
    dj_hid::write::NodeSink::open(&ours.path).map_err(|error| {
        format!(
            "the lights need a writable node {error}; on Linux that is a udev rule for this device"
        )
    })
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
    fn the_left_and_right_decks_carry_the_platter_lights() {
        // Hotcues, play, monitoring and sync belong to the two decks with platters: the mixer section has no room for
        // them, and lighting a button that is not under that hand would be a lie about which deck it belongs to.
        let decks = [
            DeckFacts {
                playing: true,
                monitoring: true,
                sync: false,
                fx: false,
                hotcues: [true; 8],
            },
            DeckFacts::default(),
            DeckFacts::default(),
            DeckFacts::default(),
        ];
        let show = show_from_facts(&decks);
        assert!(show.playing[0] && !show.playing[1]);
        assert!(show.monitoring[0] && !show.monitoring[1]);
        assert!(!show.sync[0] && !show.sync[1]);
        assert!(show.hotcues[0].iter().all(|lit| *lit));
        assert!(show.hotcues[1].iter().all(|lit| !*lit));
    }

    #[test]
    fn the_fx_buttons_follow_all_four_decks() {
        // The program runs four decks and the controller has four FX buttons, so C and D can light even though they have
        // no platter: that is the one part of the panel that can show them.
        let decks = [
            DeckFacts::default(),
            DeckFacts::default(),
            DeckFacts {
                fx: true,
                ..DeckFacts::default()
            },
            DeckFacts {
                fx: true,
                ..DeckFacts::default()
            },
        ];
        let show = show_from_facts(&decks);
        assert_eq!(show.fx, [false, false, true, true]);
    }

    #[test]
    fn the_live_lights_want_an_endpoint() {
        // A show without an engine would light nothing and say nothing; the message has to name the missing half.
        match lights_show(
            &[],
            None,
            false,
            false,
            &dj_controller_s4mk3::lights::Palette::default(),
        ) {
            Ok(()) => panic!("a show without an endpoint lit something"),
            Err(refused) => assert!(refused.contains("--engine"), "{refused}"),
        }
    }

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
