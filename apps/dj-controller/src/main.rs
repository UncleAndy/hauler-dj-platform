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

const USAGE: &str = "dj-controller --status | --list | --capture <file> [--reports <n>] | --replay <file> [--engine <endpoint>] | --screen-preview <file.ppm> [--panel left|right] | --lights-sweep [--from N] [--to M] [--colour C] [--interval MS] [--body-only] [--send] | --lights-map <file> [--from N] [--to M] [--colour C] [--send] | --lights-colours <byte> --colours 4,8,12 [--interval MS] [--send] | --lights-demo [--interval MS] [--send] | --lights-show --engine <endpoint> | --lights-fill <code> [--send] | --lights-night [--rest <code>] [--send] | --lights-bright <byte> [--colour N] [--rounds N] [--send] | --lights-colour-walk [<byte>|left|right|all] [--shift] [--interval MS] [--send] | --lights-pick [<byte>|left|right|all] [--out <file>] [--interval MS] [--send] | --screen-from --engine <endpoint> --out <file.ppm> [--panel left|right] | --lights-off | --serve --engine <endpoint> | --inputs-watch [--seconds N] | --inputs-learn [--out <file>] [--inputs <file>]";

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
    let mut screen_panel = dj_controller_s4mk3::Screen::Left;
    let mut inputs_map: Option<String> = None;
    let mut watch_seconds = 30.0_f32;
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
            "--lights-off" => mode = Mode::LightsOff,
            "--serve" => mode = Mode::Serve,
            "--inputs-watch" => mode = Mode::InputsWatch,
            "--inputs-learn" => mode = Mode::InputsLearn,
            "--inputs" => {
                let Some(path) = arguments.get(index + 1) else {
                    return Err(format!("--inputs wants a map file. {USAGE}"));
                };
                inputs_map = Some(path.clone());
                index += 1;
            }
            "--seconds" => {
                let Some(text) = arguments.get(index + 1) else {
                    return Err(format!("--seconds wants a number. {USAGE}"));
                };
                watch_seconds = text
                    .parse::<f32>()
                    .map_err(|_| format!("--seconds wants a number of seconds, got {text}"))?;
                index += 1;
            }
            "--screen-from" => mode = Mode::ScreenFrom,
            "--panel" => {
                let Some(side) = arguments.get(index + 1) else {
                    return Err(format!("--panel wants left or right. {USAGE}"));
                };
                screen_panel = match side.as_str() {
                    "left" => dj_controller_s4mk3::Screen::Left,
                    "right" => dj_controller_s4mk3::Screen::Right,
                    other => return Err(format!("--panel wants left or right, got {other}")),
                };
                index += 1;
            }
            "--out" => {
                let Some(path) = arguments.get(index + 1) else {
                    return Err(format!("--out wants a file name. {USAGE}"));
                };
                pick_out = Some(path.clone());
                index += 1;
            }
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
    let mut palette = palette_from(&config.controller.lights, &config.controller.fx);
    println!(
        "dj-controller: palette hotcue {:?}, loop {:?}, resting {:?}, night {}",
        palette.hotcue, palette.loop_, palette.resting, config.controller.lights.night
    );
    if let Some(resting) = rest_colour {
        palette.resting = resting;
    }
    // The night mode is a setting, and `--lights-night` only forces it on for one run: a service started by the
    // application has no flags, and the panel it lights must be the panel the settings describe.
    if config.controller.lights.night {
        night = true;
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
            None,
        ),
        Mode::LightsFill(code) => lights_fill(&devices, code, sweep_send, sweep_body_only),
        Mode::LightsOff => lights_off(&devices),
        Mode::Serve => serve(
            &devices,
            engine.as_deref(),
            sweep_body_only,
            night,
            &palette,
            inputs_map.as_deref(),
            config.controller.pfl,
        ),
        Mode::InputsWatch => inputs_watch(&devices, watch_seconds),
        Mode::InputsLearn => inputs_learn(&devices, pick_out.as_deref()),
        Mode::ScreenFrom => screen_from(engine.as_deref(), pick_out.as_deref(), screen_panel),
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
    ScreenFrom,
    LightsOff,
    Serve,
    InputsWatch,
    InputsLearn,
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

/// The service the application runs: the panel's lights and the panel's buttons, in one process.
///
/// Two jobs and one device: the lights come from the engine's snapshots, the presses go back to the engine as commands.
/// Reading and writing the same HID node from two processes would split the reports between them, so the reading happens
/// on a thread of this process and the writing stays where it was.
fn serve(
    devices: &[DeviceInfo],
    engine: Option<&str>,
    body_only: bool,
    night: bool,
    palette: &dj_controller_s4mk3::lights::Palette,
    inputs_file: Option<&str>,
    pfl_mode: dj_config::PflMode,
) -> Result<(), String> {
    let Some(endpoint) = engine else {
        return Err(format!("--serve wants --engine <endpoint>. {USAGE}"));
    };
    let map = load_inputs(inputs_file)?;
    if map.is_empty() {
        println!(
            "dj-controller: no control has been learned yet, so presses ask for nothing; \
             learn them with --inputs-learn"
        );
    }
    let playing: Playing = std::sync::Arc::new(std::sync::Mutex::new(DeckFactsShared::default()));
    // The effect each select key loads, distilled from the palette's (index, colour) pairs: the press handler needs the
    // effect, the lights need the colour.
    let fx_effects = palette
        .fx_keys
        .map(|(kind, _)| dj_domain::FxKind::from_index(kind).unwrap_or(dj_domain::FxKind::Filter));
    if !map.is_empty() {
        // Opening the nodes takes a moment, and the panel must light at once: the search happens on the same thread that
        // will then read the presses.
        let devices = devices.to_vec();
        let endpoint = endpoint.to_string();
        let state = std::sync::Arc::clone(&playing);
        std::thread::spawn(move || match pick_talkative(&devices) {
            Ok((nodes, reports)) => {
                println!(
                    "dj-controller: presses are read from {}",
                    nodes
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                inputs_loop(reports, &endpoint, map, state, pfl_mode, fx_effects);
            }
            Err(reason) => println!("dj-controller: {reason}; presses ask for nothing"),
        });
    }
    lights_show(
        devices,
        Some(endpoint),
        body_only,
        night,
        palette,
        Some(&playing),
    )
}

/// The map the presses are read with: the file named on the command line, else the one learned on this machine, else
/// the map the program ships with.
///
/// The learned one lives beside the settings rather than inside the binary: learning a button must not need a rebuild,
/// and a DJ who moves a control in their own file must be able to.
fn load_inputs(named: Option<&str>) -> Result<dj_controller_s4mk3::inputs::InputMap, String> {
    if let Some(path) = named {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("{path} cannot be read: {error}"))?;
        return dj_controller_s4mk3::inputs::parse_inputs(&text);
    }
    let shipped = dj_controller_s4mk3::inputs::InputMap::built_in();
    if let Some(path) = learned_inputs_path()
        && path.is_file()
    {
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("{} cannot be read: {error}", path.display()))?;
        let learned = dj_controller_s4mk3::inputs::parse_inputs(&text)?;
        let (merged, notes) = dj_controller_s4mk3::inputs::merge(&shipped, &learned);
        for note in notes {
            println!("dj-controller: {note}");
        }
        return Ok(merged);
    }
    Ok(shipped)
}

/// Where a learned map is kept: `$XDG_DATA_HOME/hauler-dj/inputs-map.txt`, beside the logs.
fn learned_inputs_path() -> Option<std::path::PathBuf> {
    dj_paths::data_dir().map(|folder| folder.join("hauler-dj").join("inputs-map.txt"))
}

/// Reads the panel and sends what a hand asked for.
///
/// Errors are said out loud once and then swallowed: a device that goes away must not take the lights with it, and a
/// program that stops answering because a USB cable moved is worse than one that says "the panel is gone".
fn inputs_loop(
    reports: std::sync::mpsc::Receiver<Vec<u8>>,
    endpoint: &str,
    map: dj_controller_s4mk3::inputs::InputMap,
    playing: Playing,
    pfl_mode: dj_config::PflMode,
    fx: [dj_domain::FxKind; 4],
) {
    let mapping = Mapping::new();
    let mut previous = std::collections::HashMap::new();
    let mut last_press: std::collections::HashMap<String, u128> = std::collections::HashMap::new();
    let started = std::time::Instant::now();
    let mut link: Option<EngineLink> = None;
    // The newest position of every knob and fader, sent on a short timer rather than once per report: a fast hand
    // produces more positions than the engine needs, and what matters is where the hand stopped.
    let mut pending: Vec<(String, dj_domain::DjCommand)> = Vec::new();
    let mut last_flush = std::time::Instant::now();
    let mut queued: Option<Vec<u8>> = None;
    // What the panel said first, once: the owner's complaint is that the window starts with the faders open while the panel
    // has them closed, and this line says which half is telling the truth.
    // The last position of every knob and fader the panel reported: what the engine is told again whenever the link is
    // re-made, so the window can never keep the engine's defaults while the panel stands somewhere else.
    let mut last_positions: std::collections::HashMap<String, dj_domain::DjCommand> =
        std::collections::HashMap::new();
    let mut first_positions = 0_usize;
    // What the panel actually said in the first seconds: report ids seen, events decoded, positions sent. One line, and
    // "the window starts with the faders open" stops being a guess about which half is wrong.
    let mut seen_reports = 0_usize;
    let mut seen_events = 0_usize;
    let mut seen_reports_ids: Vec<u8> = Vec::new();
    let mut counted = false;
    let counting_from = std::time::Instant::now();
    let mut waiting_said = false;
    loop {
        let report_bytes = match queued.take() {
            Some(report) => report,
            None => match reports.recv() {
                Ok(report) => report,
                Err(_) => {
                    println!("dj-controller: the panel stopped answering; no more presses");
                    flush_pending(&mut pending, &mut link, endpoint, &mut waiting_said);
                    return;
                }
            },
        };
        if report_bytes.len() < 2 {
            continue;
        }
        seen_reports += 1;
        if !seen_reports_ids.contains(&report_bytes[0]) {
            seen_reports_ids.push(report_bytes[0]);
        }
        let report = report_bytes[0];
        let payload = &report_bytes[1..];
        let _ = &pending;
        for event in map.decode(report, payload, &mut previous) {
            if first_positions < 24
                && map
                    .field_of(&event.control)
                    .is_some_and(|field| field.width > 1 && !field.relative)
            {
                if first_positions == 0 {
                    println!("dj-controller: the panel's starting positions:");
                }
                first_positions += 1;
                println!(
                    "dj-controller:   {} = {:.3} (report {report})",
                    event.control, event.value
                );
            }
            // A PLAY button asks for the opposite of what the deck is doing, so the deck's own state is part of the
            // question.
            let (running, pad) = match playing.lock() {
                Ok(state) => match driving_deck(&event.control, &state) {
                    Some(deck) => {
                        let index = deck_index(deck);
                        let slot = match dj_controller_core::intent_for(&event.control) {
                            Some(dj_controller_core::Intent::Hotcue(slot)) => usize::from(slot),
                            _ => 0,
                        };
                        (
                            state.playing.get(index).copied().unwrap_or(false),
                            dj_controller_core::PadState {
                                cue: state
                                    .cues
                                    .get(index)
                                    .and_then(|slots| slots.get(slot))
                                    .copied()
                                    .unwrap_or(false),
                                looped: state
                                    .loops
                                    .get(index)
                                    .and_then(|slots| slots.get(slot))
                                    .copied()
                                    .unwrap_or(false),
                                active_loop: state.active_loop.get(index).copied().unwrap_or(false),
                            },
                        )
                    }
                    None => (false, dj_controller_core::PadState::default()),
                },
                Err(_) => (false, dj_controller_core::PadState::default()),
            };
            // Every event is written down with its time: "the deck started by itself" is answered by reading what came
            // in and when, not by guessing (owner's report, 24.11).
            let now = started.elapsed().as_millis();
            // The browser is not a deck's: one VIEW button and one encoder serve both decks and the window, so they are
            // answered before the binding table, which speaks only about decks.
            if event.control.starts_with("panel.deck_") {
                if event.value > 0.5 {
                    let command = panel_deck_command(&event.control);
                    if let Some((deck, command)) = command {
                        println!(
                            "dj-controller: at {now} ms {} = 1 (report {report}) → the panel drives {deck:?}",
                            event.control
                        );
                        pending.push((event.control.clone(), command));
                        flush_pending(&mut pending, &mut link, endpoint, &mut waiting_said);
                    }
                }
                continue;
            }
            // Shift is a state, not a command: it decides what the next turn of the encoder means.
            if event.control.ends_with(".shift") {
                let index = usize::from(!event.control.starts_with("deck_a"));
                if let Ok(mut state) = playing.lock() {
                    state.shift[index] = event.value > 0.5;
                }
                println!(
                    "dj-controller: at {now} ms {} = {:.0} (report {report})",
                    event.control, event.value
                );
                continue;
            }
            // The headphone (PFL) button: hold while pressed, or a switch, whichever the settings ask for. A mixer
            // command, so it is never re-routed to the deck a side drives — the strip's CUE is the strip's own.
            if event.control.ends_with(".pfl") {
                let Some((deck, _)) =
                    dj_controller_core::BindableDeck::from_control(&event.control)
                else {
                    continue;
                };
                let deck = deck.id();
                let pressed = event.value > 0.5;
                if is_double_touch(
                    false,
                    event.value,
                    last_press.get(&event.control).copied(),
                    now,
                ) {
                    continue;
                }
                if pressed {
                    last_press.insert(event.control.clone(), now);
                }
                let on = match pfl_mode {
                    dj_config::PflMode::Hold => pressed,
                    dj_config::PflMode::Toggle => {
                        if !pressed {
                            continue;
                        }
                        let current = playing
                            .lock()
                            .ok()
                            .and_then(|state| state.monitoring.get(deck_index(deck)).copied())
                            .unwrap_or(false);
                        !current
                    }
                };
                let command = dj_domain::DjCommand::SetChannelCue { deck, on };
                println!(
                    "dj-controller: at {now} ms {} = {:.0} (report {report}) → {command:?}",
                    event.control, event.value
                );
                let asked = vec![(event.control.clone(), Some(deck), command)];
                if let Err(reason) = send_commands(endpoint, &asked) {
                    println!(
                        "dj-controller: {} did not reach the engine: {reason}",
                        event.control
                    );
                }
                continue;
            }
            // FX: the on/off button and the amount knob are per deck, the five select keys are the mixer's own. The patch
            // travels whole (kind + amount + enabled), so each control reads the deck's current patch and flips just its
            // own field (owner's request, 24.11).
            if event.control.ends_with(".fx") || event.control.ends_with(".fx_amount") {
                let Some((deck, _)) =
                    dj_controller_core::BindableDeck::from_control(&event.control)
                else {
                    continue;
                };
                let deck = deck.id();
                let index = deck_index(deck);
                let current = playing
                    .lock()
                    .ok()
                    .and_then(|state| state.fx.get(index).copied())
                    .unwrap_or_default();
                let patch = if event.control.ends_with(".fx_amount") {
                    // The knob has a centre detent that does not land on exactly half the field: a value a hair either
                    // side of the middle is the knob standing at its centre, not a hand asking for a sliver of effect
                    // (owner's report, 25.11). Snap the small neighbourhood to the middle.
                    let amount = if (event.value - 0.5).abs() < 0.015 {
                        0.5
                    } else {
                        event.value.clamp(0.0, 1.0)
                    };
                    dj_domain::FxPatch { amount, ..current }
                } else {
                    if event.value <= 0.5 {
                        continue;
                    }
                    dj_domain::FxPatch {
                        enabled: !current.enabled,
                        ..current
                    }
                };
                let command = dj_domain::DjCommand::SetDeckFx { deck, patch };
                let analog = map
                    .field_of(&event.control)
                    .is_some_and(|field| field.width > 1);
                if is_double_touch(
                    analog,
                    event.value,
                    last_press.get(&event.control).copied(),
                    now,
                ) {
                    continue;
                }
                if !analog && event.value > 0.5 {
                    last_press.insert(event.control.clone(), now);
                }
                println!(
                    "dj-controller: at {now} ms {} = {:.3} (report {report}) → {command:?}",
                    event.control, event.value
                );
                if analog {
                    last_positions.insert(event.control.clone(), command.clone());
                    if let Some(slot) = pending
                        .iter_mut()
                        .find(|(control, _)| *control == event.control)
                    {
                        slot.1 = command;
                    } else {
                        pending.push((event.control.clone(), command));
                    }
                } else {
                    let asked = vec![(event.control.clone(), Some(deck), command)];
                    if let Err(reason) = send_commands(endpoint, &asked) {
                        println!(
                            "dj-controller: {} did not reach the engine: {reason}",
                            event.control
                        );
                    }
                }
                continue;
            }
            if event.control.starts_with("panel.fx_") {
                if event.value <= 0.5 {
                    continue;
                }
                let kind = match event.control.as_str() {
                    "panel.fx_filter" => dj_domain::FxKind::Filter,
                    "panel.fx_1" => fx[0],
                    "panel.fx_2" => fx[1],
                    "panel.fx_3" => fx[2],
                    "panel.fx_4" => fx[3],
                    _ => continue,
                };
                println!(
                    "dj-controller: at {now} ms {} = 1 (report {report}) → sets {kind:?} on every deck",
                    event.control
                );
                // The five keys are the mixer's own: one press sets the effect on all four decks. Bitcrusher and Noise are
                // not on the panel — they stay reachable from the window.
                for (index, deck) in [
                    dj_domain::DeckId::A,
                    dj_domain::DeckId::B,
                    dj_domain::DeckId::C,
                    dj_domain::DeckId::D,
                ]
                .into_iter()
                .enumerate()
                {
                    let current = playing
                        .lock()
                        .ok()
                        .and_then(|state| state.fx.get(index).copied())
                        .unwrap_or_default();
                    // A deck whose effect is switched in keeps it: the select keys only re-tune channels where the effect
                    // is off, so a running effect is never yanked away from under a live mix (owner's report, 25.11).
                    if current.enabled {
                        continue;
                    }
                    let patch = dj_domain::FxPatch { kind, ..current };
                    pending.push((
                        event.control.clone(),
                        dj_domain::DjCommand::SetDeckFx { deck, patch },
                    ));
                }
                flush_pending(&mut pending, &mut link, endpoint, &mut waiting_said);
                continue;
            }
            if event.control.starts_with("panel.") {
                match panel_command(&event, &playing) {
                    Some(command) => {
                        println!(
                            "dj-controller: at {now} ms {} = {:.0} (report {report}) → {command:?}",
                            event.control, event.value
                        );
                        if let Ok(mut state) = playing.lock()
                            && let dj_domain::DjCommand::BrowserShow { open } = &command
                        {
                            // Said here as well as in the engine: a second press must fold the browser back, not ask for
                            // the same thing twice while the snapshot is on its way.
                            state.browser_open = *open;
                        }
                        // Every turn is its own row: nothing is coalesced, or a fast turn would move one row.
                        pending.push((event.control.clone(), command));
                        flush_pending(&mut pending, &mut link, endpoint, &mut waiting_said);
                    }
                    None => {
                        if event.control.starts_with("panel.encoder_press") && event.value > 0.5 {
                            let held = playing
                                .lock()
                                .ok()
                                .and_then(|state| state.browser_cursor.clone());
                            println!(
                                "dj-controller: at {now} ms {} — the browser has no row selected (the engine's cursor: {})",
                                event.control,
                                held.unwrap_or_else(|| "none".to_string())
                            );
                        }
                    }
                }
                continue;
            }
            let Some((deck, command)) = mapping.resolve_control(&event, running, pad) else {
                println!(
                    "dj-controller: at {now} ms {} = {:.3} (report {report}) asks for nothing; deck running {running}",
                    event.control, event.value
                );
                continue;
            };
            let analog = map
                .field_of(&event.control)
                .is_some_and(|field| field.width > 1);
            // A light touch can arrive as two presses a few milliseconds apart, and the second one would undo the first.
            // A hand cannot press the same button twice that fast — but a **fader** passes through the whole range in one
            // gesture, and this debounce swallowed its positions one after another: a fader thrown up stopped where the
            // last accepted step was, half way (owner's report, 24.11). Only buttons are debounced.
            if is_double_touch(
                analog,
                event.value,
                last_press.get(&event.control).copied(),
                now,
            ) {
                let before = last_press.get(&event.control).copied().unwrap_or(now);
                println!(
                    "dj-controller: at {now} ms {} ignored: the last press was {} ms ago",
                    event.control,
                    now.saturating_sub(before)
                );
                continue;
            }
            if !analog && event.value > 0.5 {
                last_press.insert(event.control.clone(), now);
            }
            let raw = map.raw_of(&event.control, report, payload).map(|raw| {
                let top = match map.field_of(&event.control) {
                    Some(field) => (1_u32 << field.width.min(16)) - 1,
                    None => 0,
                };
                format!(", raw {raw} of {top}")
            });
            println!(
                "dj-controller: at {now} ms {} = {:.3} (report {report}{}) → {command:?}",
                event.control,
                event.value,
                raw.unwrap_or_default()
            );
            // The buttons of a side go to the deck that side is switched to: the transport and the pads of the left half of
            // the panel are named `deck_a` in the map, and A stops meaning anything once the side is on C.
            let command = follow_side(command, &event.control, &playing);
            if let dj_domain::DjCommand::SetChannelFader { deck, position } = &command
                && let Ok(mut state) = playing.lock()
            {
                state.fader_asked[deck_index(*deck)] = Some(*position);
            }
            if analog {
                seen_events += 1;
                last_positions.insert(event.control.clone(), command.clone());
                // One entry per control, the newest wins.
                if let Some(slot) = pending
                    .iter_mut()
                    .find(|(control, _)| *control == event.control)
                {
                    slot.1 = command;
                } else {
                    pending.push((event.control.clone(), command));
                }
            } else {
                let asked = vec![(event.control.clone(), deck, command)];
                if let Err(reason) = send_commands(endpoint, &asked) {
                    println!(
                        "dj-controller: {} did not reach the engine: {reason}",
                        event.control
                    );
                }
            }
        }
        // A knob's newest position goes out as soon as the hand stops feeding reports, and never later than ten
        // milliseconds: a fader thrown to the top must end at the top, not where the queue ran out.
        match reports.try_recv() {
            Ok(next) => queued = Some(next),
            Err(_) => {
                flush_pending(&mut pending, &mut link, endpoint, &mut waiting_said);
                last_flush = std::time::Instant::now();
            }
        }
        if !counted && counting_from.elapsed() > std::time::Duration::from_secs(2) {
            counted = true;
            println!(
                "dj-controller: 2 s in: {seen_reports} reports (ids {seen_reports_ids:?}), {seen_events} events, {} positions known",
                last_positions.len()
            );
        }
        if last_flush.elapsed() > std::time::Duration::from_millis(10) {
            // No link: put every known position back in the queue before trying again. The first positions of a session
            // arrive before the engine answers, and a position that is never sent stays wrong in the window until a hand
            // happens to move that very control (owner's report, 24.11).
            if link.is_none() && !last_positions.is_empty() {
                for (control, command) in &last_positions {
                    if !pending.iter().any(|(queued, _)| queued == control) {
                        pending.push((control.clone(), command.clone()));
                    }
                }
            }
            flush_pending(&mut pending, &mut link, endpoint, &mut waiting_said);
            last_flush = std::time::Instant::now();
        }
    }
}

/// One owner of the panel, or none.
///
/// A second `dj-controller` on the same device does not queue behind the first: two readers **split** the report stream
/// between them, and each process sees about half of what the hand does. The owner's symptom was exactly that — the window
/// kept the engine's defaults until one fader happened to move, because its positions had gone to the other reader. Two
/// writers are no better: the panel ends up dark, flickering or both, depending on who wrote last.
fn claim_panel() -> Result<std::path::PathBuf, String> {
    let directory = std::env::var("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    let path = directory.join("hauler-dj-controller.pid");
    for attempt in 0..2_u8 {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                use std::io::Write as _;
                let _ = writeln!(file, "{}", std::process::id());
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let held = std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|text| text.trim().parse::<u32>().ok());
                if let Some(pid) = held
                    && pid != std::process::id()
                    && std::path::Path::new(&format!("/proc/{pid}")).exists()
                {
                    return Err(format!(
                        "another dj-controller already drives the panel (pid {pid}): two readers split the reports \
                         and two writers fight over the lights, so this one stops"
                    ));
                }
                // A leftover from a process that is gone: taken over.
                let _ = std::fs::remove_file(&path);
                if attempt == 1 {
                    return Err(format!("{} cannot be taken", path.display()));
                }
            }
            Err(error) => return Err(format!("{} cannot be written: {error}", path.display())),
        }
    }
    Err(format!("{} cannot be taken", path.display()))
}

/// What the browser's own controls ask of the engine, if anything.
///
/// - **VIEW** folds the browser away or brings it back: the one thing both the panel and the window can do to it.
/// - **The encoder's turn** moves the cursor one row. A relative field says which way it went (1.0 right, 0.0 left), not
///   how far: one step is one row, the same as Mixxx's own encoder handling.
/// - **The encoder's press** loads the row the cursor stands on, into a deck that is standing still. A playing deck
///   refuses a load, and a press that cut the track the room is dancing to would be the one mistake the panel must not be
///   able to make; with both decks playing the press names deck B and the engine refuses it, which is said out loud.
fn panel_command(
    event: &dj_controller_core::ControlEvent,
    playing: &Playing,
) -> Option<dj_domain::DjCommand> {
    let pressed = event.value > 0.5;
    match event.control.as_str() {
        "panel.view" | "panel.view_right" => {
            let open = playing.lock().ok()?.browser_open;
            pressed.then_some(dj_domain::DjCommand::BrowserShow { open: !open })
        }
        "panel.encoder" | "panel.encoder_right" => {
            // Shift turns the same knob into a walk through the left block: the collection, the playlists and the files, and
            // the tree under them. The rows of a list stay where they were — a DJ scrolling tracks does not want the rail to
            // move, and the other way round.
            //
            // The panel has **one** browse encoder and two Shift keys, so either key makes the rail: which hand reached for
            // Shift says nothing about what the knob should walk (owner's report, 24.11: the left encoder with the right
            // deck's Shift did nothing).
            let index = usize::from(event.control == "panel.encoder_right");
            let shift = playing.lock().ok().is_some_and(|state| {
                state.shift.first().copied().unwrap_or(false)
                    || state.shift.get(1).copied().unwrap_or(false)
            });
            let delta = if pressed { 1 } else { -1 };
            // Said out loud: a Shift that never arrives looks exactly like a Shift the panel ignores, and this line tells the
            // two apart without another round of guessing (owner's report, 24.11).
            println!(
                "dj-controller: {} turn, shift held: {shift}",
                if index == 0 { "left" } else { "right" }
            );
            Some(if shift {
                dj_domain::DjCommand::BrowserPlaceStep { delta }
            } else {
                dj_domain::DjCommand::BrowserStep { delta }
            })
        }
        "panel.encoder_press" => {
            if !pressed {
                return None;
            }
            let state = playing.lock().ok()?;
            if state.shift.first().copied().unwrap_or(false)
                || state.shift.get(1).copied().unwrap_or(false)
            {
                // Shift + press opens or closes the level the rail's cursor stands on.
                return Some(dj_domain::DjCommand::BrowserPlaceToggle);
            }
            let path = state.browser_cursor.clone()?;
            Some(dj_domain::DjCommand::Load {
                deck: state.panel_left,
                path,
            })
        }
        "panel.encoder_press_right" => {
            if !pressed {
                return None;
            }
            let state = playing.lock().ok()?;
            if state.shift.first().copied().unwrap_or(false)
                || state.shift.get(1).copied().unwrap_or(false)
            {
                return Some(dj_domain::DjCommand::BrowserPlaceToggle);
            }
            let path = state.browser_cursor.clone()?;
            Some(dj_domain::DjCommand::Load {
                deck: state.panel_right,
                path,
            })
        }
        _ => None,
    }
}

/// The deck a control of the panel actually commands.
///
/// The map names the panel's halves `deck_a` and `deck_b`; a side switched to C or D still sends `deck_a.*`, so the state a
/// press is judged by — is the deck playing? does the pad hold a cue? — must come from the deck the side drives. Reading A
/// while the DJ watches C made PLAY start C instead of stopping it, because A was the deck that was standing still
/// (owner's report, 24.11).
fn driving_deck(control: &str, state: &DeckFactsShared) -> Option<dj_domain::DeckId> {
    if control.starts_with("deck_a.") {
        return Some(state.panel_left);
    }
    if control.starts_with("deck_b.") {
        return Some(state.panel_right);
    }
    dj_controller_core::BindableDeck::from_control(control).map(|(deck, _)| deck.id())
}

/// Send a command to the deck the side of the panel is switched to.
///
/// The map names the panel's own halves `deck_a` and `deck_b`, because that is what they were when a session began. Once a
/// side is switched to C or D, every transport and pad command from that half must go to that deck — a PLAY that starts A
/// while the DJ is looking at C is worse than a dark button. Mixer controls are left alone: the four channel faders and
/// the equalisers belong to channels, not to the decks the sides drive.
fn follow_side(
    command: dj_domain::DjCommand,
    control: &str,
    playing: &Playing,
) -> dj_domain::DjCommand {
    let target = if control.starts_with("deck_a.") {
        playing.lock().ok().map(|state| state.panel_left)
    } else if control.starts_with("deck_b.") {
        playing.lock().ok().map(|state| state.panel_right)
    } else {
        None
    };
    let Some(target) = target else {
        return command;
    };
    command_for_deck(command, target)
}

/// The same command, addressed to another deck.
fn command_for_deck(
    command: dj_domain::DjCommand,
    deck: dj_domain::DeckId,
) -> dj_domain::DjCommand {
    use dj_domain::DjCommand;
    match command {
        DjCommand::Play { enabled, .. } => DjCommand::Play { deck, enabled },
        DjCommand::Cue { action, .. } => DjCommand::Cue { deck, action },
        DjCommand::Sync { mode, .. } => DjCommand::Sync { deck, mode },
        DjCommand::Metronome { enabled, .. } => DjCommand::Metronome { deck, enabled },
        DjCommand::SetCueMeta {
            slot, label, color, ..
        } => DjCommand::SetCueMeta {
            deck,
            slot,
            label,
            color,
        },
        DjCommand::SetPadLoop { slot, .. } => DjCommand::SetPadLoop { deck, slot },
        other => other,
    }
}

/// The byte of a deck's own select button, by the name the lights map gives it.
fn deck_select_byte(deck: dj_domain::DeckId) -> Option<u8> {
    let name = match deck {
        dj_domain::DeckId::A => "Deck select A",
        dj_domain::DeckId::B => "Deck select B",
        dj_domain::DeckId::C => "Deck select C",
        dj_domain::DeckId::D => "Deck select D",
    };
    dj_controller_s4mk3::lights::byte_of(name)
}

/// Light the select button of the deck each side drives, and let its neighbour sit back.
///
/// The panel has a select button per deck; the one the side is on is bright, the other is faint — the same "on" and
/// "off" brightness as the rest of the panel, so the pair reads as one choice rather than two lamps.
fn paint_deck_select(
    report: &mut dj_controller_s4mk3::lights::LightsReport,
    left: dj_domain::DeckId,
    right: dj_domain::DeckId,
    palette: &dj_controller_s4mk3::lights::Palette,
    night: bool,
) {
    let sides = [
        (left, [dj_domain::DeckId::A, dj_domain::DeckId::C]),
        (right, [dj_domain::DeckId::B, dj_domain::DeckId::D]),
    ];
    for (chosen, pair) in sides {
        for deck in pair {
            let Some(byte) = deck_select_byte(deck) else {
                continue;
            };
            let Some(slot) = report.body.get_mut(usize::from(byte)) else {
                continue;
            };
            *slot = if deck == chosen {
                dj_controller_s4mk3::lights::bright(palette.resting.code())
            } else if night {
                dj_controller_s4mk3::lights::dim(palette.resting.code())
            } else {
                0
            };
        }
    }
}

/// The deck a select button names, and the command that says the side drives it.
///
/// Four buttons, one per deck: A and C sit on the left side, B and D on the right. The engine carries the choice so the
/// window can show it and so a press of the browse encoder on that side knows where to load.
fn panel_deck_command(control: &str) -> Option<(dj_domain::DeckId, dj_domain::DjCommand)> {
    let (deck, side) = match control {
        "panel.deck_a" => (dj_domain::DeckId::A, dj_domain::PanelSide::Left),
        "panel.deck_c" => (dj_domain::DeckId::C, dj_domain::PanelSide::Left),
        "panel.deck_b" => (dj_domain::DeckId::B, dj_domain::PanelSide::Right),
        "panel.deck_d" => (dj_domain::DeckId::D, dj_domain::PanelSide::Right),
        _ => return None,
    };
    Some((deck, dj_domain::DjCommand::PanelDeck { side, deck }))
}

/// Whether a press is the second half of a light touch on the same button.
///
/// Buttons only: a fader crosses half of its travel in every gesture, and treating its positions as presses dropped them
/// one after another until only the first one of each 150 ms stood — a fader thrown to the top stopped half way, while
/// the bottom always arrived because zero is below the threshold (owner's report, 24.11).
fn is_double_touch(analog: bool, value: f32, last: Option<u128>, now: u128) -> bool {
    !analog && value > 0.5 && last.is_some_and(|last| now.saturating_sub(last) < 150)
}

/// Sends the newest position of every knob and fader that has one waiting.
fn flush_pending(
    pending: &mut Vec<(String, dj_domain::DjCommand)>,
    link: &mut Option<EngineLink>,
    endpoint: &str,
    waiting_said: &mut bool,
) {
    if pending.is_empty() {
        return;
    }
    if link.is_none() {
        *link = EngineLink::connect(endpoint).ok();
    }
    let Some(open) = link.as_mut() else {
        if !*waiting_said {
            println!(
                "dj-controller: the engine is not answering yet; {} positions are waiting",
                pending.len()
            );
            *waiting_said = true;
        }
        // Kept, not dropped: the first positions of the session arrive before the engine answers, and dropping them left
        // every fader in the window where the engine's defaults put it until a hand moved it again. Bounded, so a panel
        // turned while the engine is away cannot grow this without limit.
        if pending.len() > 64 {
            let excess = pending.len() - 64;
            pending.drain(..excess);
        }
        return;
    };
    if *waiting_said {
        println!(
            "dj-controller: the engine answered; sending {} waiting positions",
            pending.len()
        );
        *waiting_said = false;
    }
    for (control, command) in pending.drain(..) {
        if let Err(reason) = open.send(&command) {
            println!("dj-controller: {control} did not reach the engine: {reason}");
            // The next flush opens a fresh connection: a broken pipe must not silence the panel for good.
            *link = None;
            return;
        }
    }
}

/// Which deck a slot of the shared state means.
fn deck_index(deck: dj_domain::DeckId) -> usize {
    match deck {
        dj_domain::DeckId::A => 0,
        dj_domain::DeckId::B => 1,
        dj_domain::DeckId::C => 2,
        dj_domain::DeckId::D => 3,
    }
}

/// What the decks are doing, filled from the snapshots the lights loop already receives.
///
/// Two readers: the lights and the presses. A PLAY button asks for the opposite of what the deck is doing, and a hotcue
/// pad jumps to its slot when the slot holds a cue and stores one when it does not — both are questions about the decks,
/// not about the panel.
#[derive(Clone, Debug)]
struct DeckFactsShared {
    playing: [bool; 4],
    cues: [[bool; 8]; 4],
    /// Pads that hold a loop rather than a cue: the two are jumped to the same way and must not be taken for empty.
    loops: [[bool; 8]; 4],
    /// Whether the deck has a loop of its own that an empty pad would take.
    active_loop: [bool; 4],
    /// Whether the channel feeds the headphones: the PFL button toggles against this, not against a guess.
    monitoring: [bool; 4],
    /// Each deck's effect, whole: the patch travels complete, so a knob or a key reads the current kind, amount and
    /// switch and flips just its own field.
    fx: [dj_domain::FxPatch; 4],
    /// What the panel last asked the engine for, per channel: a fader thrown to the top must be seen at the top by the
    /// next snapshot, and a disagreement is said out loud rather than left as "it stopped half way".
    fader_asked: [Option<f32>; 4],
    /// Whether the browser is on the screen: the VIEW button asks for the opposite of this.
    browser_open: bool,
    /// The path the window's browser cursor stands on: what a press of the encoder loads.
    browser_cursor: Option<String>,
    /// Shift of each deck side: with it held, the encoder walks the left block instead of the rows.
    shift: [bool; 2],
    /// The deck each side of the panel drives: the deck switch moves it, and a load lands there. Left is A and right is B
    /// until the switch says otherwise.
    panel_left: dj_domain::DeckId,
    panel_right: dj_domain::DeckId,
}

impl Default for DeckFactsShared {
    /// The browser is open until someone folds it away, and nothing is selected until the window says so.
    fn default() -> Self {
        Self {
            playing: [false; 4],
            cues: [[false; 8]; 4],
            loops: [[false; 8]; 4],
            active_loop: [false; 4],
            monitoring: [false; 4],
            fx: [dj_domain::FxPatch::default(); 4],
            fader_asked: [None; 4],
            browser_open: true,
            browser_cursor: None,
            shift: [false; 2],
            panel_left: dj_domain::DeckId::A,
            panel_right: dj_domain::DeckId::B,
        }
    }
}

type Playing = std::sync::Arc<std::sync::Mutex<DeckFactsShared>>;

/// Reads the panel on a thread and hands the reports over a channel.
///
/// A blocking read on a device that says nothing would hang whoever is listening: the walk would never reach its
/// deadline, and "press the button" would sit there for ever. Over a channel the listener keeps its own clock.
fn spawn_reader(path: &std::path::Path) -> Result<std::sync::mpsc::Receiver<Vec<u8>>, String> {
    let device = DeviceInfo {
        path: path.to_path_buf(),
        vendor: VENDOR,
        product: PRODUCT,
        name: NAME.to_string(),
    };
    let mut source = HidrawTransport::new().open(&device).map_err(|error| {
        format!(
            "{} cannot be opened for reading: {error}",
            device.path.display()
        )
    })?;
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 128];
        loop {
            match source.read_report(&mut buffer) {
                Ok(read) if read >= 1 => {
                    if sender.send(buffer[..read].to_vec()).is_err() {
                        return;
                    }
                }
                Ok(_) => {}
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(100)),
            }
        }
    });
    Ok(receiver)
}

/// The first node of the panel that actually talks.
///
/// One USB device shows up as several HID nodes, and the one that carries the buttons is not necessarily the first the
/// kernel lists. Rather than trusting the order, each matching node is given a moment to say something: the one that
/// does is the panel.
fn pick_talkative(
    devices: &[DeviceInfo],
) -> Result<(Vec<std::path::PathBuf>, std::sync::mpsc::Receiver<Vec<u8>>), String> {
    // Every node of the panel, not one guess: a device shows up as several HID nodes, the buttons and the clock need not
    // live on the same one, and the node that carries the buttons may only speak when one is pressed. Reading them all
    // costs a few threads and removes the guess entirely.
    let (sender, receiver) = std::sync::mpsc::channel();
    let mut opened = Vec::new();
    for device in devices.iter().filter(|device| device.is(VENDOR, PRODUCT)) {
        let Ok(reports) = spawn_reader(&device.path) else {
            println!(
                "dj-controller: {} не открывается — пропускаю этот узел",
                device.path.display()
            );
            continue;
        };
        opened.push(device.path.clone());
        let sender = sender.clone();
        std::thread::spawn(move || {
            while let Ok(report) = reports.recv() {
                if sender.send(report).is_err() {
                    return;
                }
            }
        });
    }
    if opened.is_empty() {
        return Err(format!(
            "no {NAME} node could be opened: the panel is plugged in but unreadable"
        ));
    }
    Ok((opened, receiver))
}

/// Prints what the panel says, one line a change: the tool that turns a hand into a map line.
fn inputs_watch(devices: &[DeviceInfo], seconds: f32) -> Result<(), String> {
    if !devices.iter().any(|device| device.is(VENDOR, PRODUCT)) {
        return Err(format!(
            "no {NAME} is plugged in, so there is nothing to watch"
        ));
    }
    let (nodes, reports) = pick_talkative(devices)?;
    println!(
        "dj-controller: watching {} for {seconds:.0} s — presses and turns are printed as \
         report, byte, bit and value",
        nodes
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let until = std::time::Instant::now() + std::time::Duration::from_secs_f32(seconds.max(1.0));
    let mut last: std::collections::HashMap<u8, Vec<u8>> = std::collections::HashMap::new();
    let mut seen = 0_usize;
    while std::time::Instant::now() < until {
        let Ok(report) = reports.recv_timeout(std::time::Duration::from_millis(100)) else {
            continue;
        };
        if report.len() < 2 {
            continue;
        }
        seen += 1;
        let id = report[0];
        let payload = &report[1..];
        if let Some(previous) = last.get(&id) {
            for byte in 0..payload.len().min(previous.len()) {
                let value = payload[byte];
                let was = previous[byte];
                if value == was {
                    continue;
                }
                for bit in 0..8 {
                    let mask = 1_u8 << bit;
                    if (value ^ was) & mask == 0 {
                        continue;
                    }
                    println!(
                        "dj-controller: report {id} byte {byte} bit {bit} → {}",
                        u8::from(value & mask != 0)
                    );
                }
            }
        }
        last.insert(id, payload.to_vec());
    }
    println!("dj-controller: {seen} отчётов за {seconds:.0} с");
    Ok(())
}

/// Asks for one control at a time and writes down where the press appeared.
///
/// Nothing is guessed: the line written is the byte and bit that changed while that one button was pressed, and the next
/// control is asked for only after the button was let go. Analog controls are not in this walk — a hand cannot hold a
/// knob still enough to say which field it is — and are found with `--inputs-watch`.
fn inputs_learn(devices: &[DeviceInfo], out: Option<&str>) -> Result<(), String> {
    if !devices.iter().any(|device| device.is(VENDOR, PRODUCT)) {
        return Err(format!(
            "no {NAME} is plugged in, so there is nothing to learn"
        ));
    }
    let (nodes, reports) = pick_talkative(devices)?;
    const WALK: &[(&str, &str)] = &[
        ("deck_a.play", "PLAY на деке A"),
        ("deck_a.cue", "CUE на деке A"),
        ("deck_a.sync", "SYNC/LOCK на деке A"),
        ("deck_b.play", "PLAY на деке B"),
        ("deck_b.cue", "CUE на деке B"),
        ("deck_b.sync", "SYNC/LOCK на деке B"),
    ];
    println!(
        "dj-controller: слушаю {}",
        nodes
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );

    // The panel does not send its whole state forever. Report 3 is the clock and the platters, and it comes about four
    // times a second whether anything is touched or not — counters, wheel timestamps, nothing a hand presses. The
    // buttons travel in reports 1 and 2, and those arrive **when something changes**, so a quiet panel sends nothing
    // there at all. That is why a learned "button" was really a counter bit in report 3 (owner's reports, 24.11): the
    // only thing that was moving.
    println!(
        "dj-controller: пробы слышно — report 3 это часы и джоги (счётчики), кнопки приходят в report 1 и 2; \
         обучение слушает только 1 и 2"
    );

    let mut learned: Vec<(String, u8, usize, usize)> = Vec::new();
    for (control, label) in WALK {
        println!(
            "dj-controller: нажмите и ПОДЕРЖИТЕ {label} около секунды, затем отпустите (20 с на кнопку)"
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        // Distinct states of each report, oldest first. The answer is a bit that was down, went up while the finger was
        // on the button, and came back down when it was let go.
        let mut history: std::collections::HashMap<u8, Vec<Vec<u8>>> =
            std::collections::HashMap::new();
        let mut found: Option<(u8, usize, usize, usize)> = None;
        while found.is_none() && std::time::Instant::now() < deadline {
            let Ok(report) = reports.recv_timeout(std::time::Duration::from_millis(100)) else {
                continue;
            };
            if report.len() < 2 {
                continue;
            }
            let id = report[0];
            if id == 3 {
                continue;
            }
            let payload = report[1..].to_vec();
            let states = history.entry(id).or_default();
            if states.last().is_some_and(|last| *last == payload) {
                continue;
            }
            states.push(payload);
            // Wait for enough states to see a whole press: down, up, down.
            if states.len() < 3 {
                continue;
            }
            let mut candidates: Vec<(u8, usize, usize, usize)> = Vec::new();
            let bytes = states.iter().map(Vec::len).min().unwrap_or(0);
            for byte in 0..bytes {
                for bit in 0..8 {
                    let track: Vec<bool> = states
                        .iter()
                        .map(|state| state[byte] & (1_u8 << bit) != 0)
                        .collect();
                    let first_up = track.iter().position(|up| *up);
                    let Some(first_up) = first_up else {
                        continue;
                    };
                    // It must start down, run up, and come back down: a press, not a state that happens to be on.
                    if first_up == 0 || *track.last().unwrap_or(&true) {
                        continue;
                    }
                    let run = track.iter().filter(|up| **up).count();
                    candidates.push((id, byte, bit, run));
                }
            }
            // One finger, one bit. More than one means either a chord or a field that lies, and guessing would put a
            // command on the wrong button.
            if candidates.len() == 1 {
                found = candidates.pop();
            }
        }
        let Some((report, byte, bit, run)) = found else {
            println!(
                "dj-controller: {control}: не увидел нажатия и отпускания в report 1/2 — пропускаю"
            );
            continue;
        };
        if learned.iter().any(|(_, seen_report, seen_byte, seen_bit)| {
            *seen_report == report && *seen_byte == byte && *seen_bit == bit
        }) {
            println!(
                "dj-controller: {control}: report {report} byte {byte} bit {bit} уже занят другой кнопкой — пропускаю"
            );
            continue;
        }
        println!(
            "dj-controller: {control} → report {report} byte {byte} bit {bit} (был поднят {run} отчётов)"
        );
        learned.push((control.to_string(), report, byte, bit));
        // The hand has to be off the button before the next question.
        std::thread::sleep(std::time::Duration::from_millis(500));
        while reports.try_recv().is_ok() {}
    }
    if learned.is_empty() {
        return Err("nothing was learned: no button was pressed and let go".to_string());
    }
    let path = match out {
        Some(path) => path.to_string(),
        None => {
            let path = learned_inputs_path()
                .ok_or("no data folder to write the map into; pass --out <file>")?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("{}: {error}", parent.display()))?;
            }
            path.display().to_string()
        }
    };
    let mut text = String::new();
    for (control, report, byte, bit) in &learned {
        text.push_str(&format!("{control} {report} {byte} {bit} 1\n"));
    }
    std::fs::write(&path, &text).map_err(|error| format!("{path} cannot be written: {error}"))?;
    println!(
        "dj-controller: {} строк записано в {path}; служба прочитает его при следующем запуске",
        learned.len()
    );
    Ok(())
}

/// Put the panel out: every byte of the lights report dark.
///
/// A panel left lit after the program closes is an instrument left on: the buttons stay glowing on a switched-off
/// machine, and the next person cannot tell whether the program is running. Said twice, because the panel has been
/// seen to miss the first report after a burst.
fn lights_off(devices: &[DeviceInfo]) -> Result<(), String> {
    if devices.is_empty() {
        println!("dj-controller: no panel to darken");
        return Ok(());
    }
    let mut sink = open_lights(devices)?;
    let dark = dj_controller_s4mk3::lights::LightsReport::dark();
    sink.send(&dark.wire(true))
        .map_err(|error| format!("the panel did not take the dark report: {error}"))?;
    std::thread::sleep(std::time::Duration::from_millis(60));
    sink.send(&dark.wire(true))
        .map_err(|error| format!("the panel did not take the dark report: {error}"))?;
    println!("dj-controller: the panel is dark");
    Ok(())
}

/// What one screen says, taken from what the engine publishes.
///
/// The panels are two, and the program runs four decks: the left panel belongs to decks A and C, the right one to B and
/// D. Which of the pair is on the panel at a moment is the DJ's business, and this build shows the primary deck of the
/// side — the one the settings name first — rather than guessing from what is playing.
fn panel_from(
    snapshot: &dj_domain::Snapshot,
    side: dj_controller_s4mk3::Screen,
) -> dj_controller_s4mk3::screen::DeckPanel {
    let first = match side {
        dj_controller_s4mk3::Screen::Left => dj_domain::DeckId::A,
        dj_controller_s4mk3::Screen::Right => dj_domain::DeckId::B,
    };
    let Some(deck) = snapshot.decks.iter().find(|deck| deck.deck == first) else {
        return dj_controller_s4mk3::screen::DeckPanel {
            deck: if first == dj_domain::DeckId::A {
                'A'
            } else {
                'B'
            },
            ..dj_controller_s4mk3::screen::DeckPanel::default()
        };
    };
    let rate = f64::from(deck.source_rate.max(1));
    let elapsed = deck.source_frame.0 as f64 / rate;
    let remaining = deck
        .remaining_frames
        .map(|frames| frames as f64 / rate)
        .unwrap_or(0.0);
    let duration = deck
        .remaining_frames
        .map(|frames| (deck.source_frame.0 + frames) as f64 / rate)
        .filter(|seconds| *seconds > 0.0);
    let loop_fraction = match (deck.loop_region, duration) {
        (Some(region), Some(total)) => Some((
            region.start.0 as f64 / rate / total,
            region.end.0 as f64 / rate / total,
        )),
        _ => None,
    };
    dj_controller_s4mk3::screen::DeckPanel {
        deck: match first {
            dj_domain::DeckId::A => 'A',
            dj_domain::DeckId::B => 'B',
            dj_domain::DeckId::C => 'C',
            dj_domain::DeckId::D => 'D',
        },
        title: deck
            .track
            .as_ref()
            .map(|track| track.title.clone())
            .unwrap_or_default(),
        artist: deck
            .track
            .as_ref()
            .map(|track| track.artist.clone())
            .unwrap_or_default(),
        playing: deck.playing,
        loaded: deck.track.is_some(),
        elapsed_seconds: elapsed,
        remaining_seconds: remaining,
        bpm: deck
            .track
            .as_ref()
            .and_then(|track| track.bpm)
            .or(deck.local_bpm),
        pitch_percent: (deck.tempo_ratio - 1.0) * 100.0,
        position: deck.position_ratio.map(f64::from).unwrap_or_else(|| {
            duration
                .map(|total| (elapsed / total).clamp(0.0, 1.0))
                .unwrap_or(0.0)
        }),
        loop_fraction,
        beat_phase: deck
            .bar_error_beats
            .map(|beats| f64::from(beats).rem_euclid(1.0)),
        beats_per_bar: 4,
        key: deck.track.as_ref().and_then(|track| track.key.clone()),
    }
}

/// Draw one screen from a live snapshot and write it where a person can look at it.
fn screen_from(
    engine: Option<&str>,
    out: Option<&str>,
    side: dj_controller_s4mk3::Screen,
) -> Result<(), String> {
    let (Some(endpoint), Some(path)) = (engine, out) else {
        return Err(format!(
            "--screen-from wants --engine <endpoint> and --out <file>. {USAGE}"
        ));
    };
    let (mut client, _welcome) = dj_bridge::connect_with_timeout(
        std::path::Path::new(endpoint),
        Some(std::time::Duration::from_secs(2)),
    )
    .map_err(|error| format!("no engine at {endpoint}: {error}"))?;
    loop {
        match client
            .recv_server()
            .map_err(|error| format!("the engine stopped answering: {error}"))?
        {
            dj_bridge::ServerMessage::Snapshot(snapshot) => {
                let panel = panel_from(&snapshot, side);
                let canvas = dj_controller_s4mk3::screen::render_deck(&panel);
                std::fs::write(path, canvas.to_ppm())
                    .map_err(|error| format!("{path}: {error}"))?;
                println!(
                    "dj-controller: {path}: {} — {} by {}, {} left",
                    panel.deck,
                    panel.title,
                    panel.artist,
                    dj_controller_s4mk3::screen::clock(panel.remaining_seconds)
                );
                return Ok(());
            }
            _ => continue,
        }
    }
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
fn palette_from(
    lights: &dj_config::ControllerLights,
    fx: &dj_config::ControllerFx,
) -> dj_controller_s4mk3::lights::Palette {
    dj_controller_s4mk3::lights::Palette {
        resting: colour_of(lights.resting),
        play: colour_of(lights.play),
        cue: colour_of(lights.cue),
        sync: colour_of(lights.sync),
        hotcue: colour_of(lights.hotcue),
        loop_: colour_of(lights.loop_),
        fx: colour_of(lights.fx),
        fx_keys: fx_keys_from(fx),
    }
}

/// The four FX select keys as (effect index, colour): what each key glows, and where a deck's effect gets its colour on
/// the "FX on" button.
fn fx_keys_from(fx: &dj_config::ControllerFx) -> [(u8, dj_controller_s4mk3::Colour); 4] {
    [
        (fx.key1_effect.index(), colour_of(fx.key1_colour)),
        (fx.key2_effect.index(), colour_of(fx.key2_colour)),
        (fx.key3_effect.index(), colour_of(fx.key3_colour)),
        (fx.key4_effect.index(), colour_of(fx.key4_colour)),
    ]
}

/// What one deck says about itself, as far as the panel cares.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct DeckFacts {
    playing: bool,
    monitoring: bool,
    sync: bool,
    fx: bool,
    /// The effect the deck runs, as `FxKind::index()`: the "FX on" button wears this effect's colour.
    fx_kind: u8,
    /// What each hotcue pad shows: the marker's colour, and whether the slot holds a cue.
    hotcues: [Option<dj_controller_s4mk3::lights::Pad>; 8],
}

/// The panel's state from four decks' facts.
///
/// Two halves again: hotcues, `PLAY`, `CUE` and sync belong to the decks with platters — the left and right pairs — while
/// the mixer section has an FX button for each of the four decks the program runs. Decks C and D have no platter on this
/// controller, so nothing else of theirs can be lit.
fn show_from_facts(decks: &[DeckFacts; 4], sides: [usize; 2]) -> dj_controller_s4mk3::lights::Show {
    // The panel's two sides show the two decks they are switched to, not always A and B: a side on C must light C's
    // transport, C's pads and C's loop, or the panel lies about the deck the DJ is working on (owner's report, 24.11).
    let left = decks.get(sides[0]).copied().unwrap_or_default();
    let right = decks.get(sides[1]).copied().unwrap_or_default();
    dj_controller_s4mk3::lights::Show {
        hotcues: [left.hotcues, right.hotcues],
        playing: [left.playing, right.playing],
        monitoring: [
            decks[0].monitoring,
            decks[1].monitoring,
            decks[2].monitoring,
            decks[3].monitoring,
        ],
        sync: [left.sync, right.sync],
        fx: [decks[0].fx, decks[1].fx, decks[2].fx, decks[3].fx],
        fx_kind: [
            decks[0].fx_kind,
            decks[1].fx_kind,
            decks[2].fx_kind,
            decks[3].fx_kind,
        ],
    }
}

/// The same facts, taken from a published snapshot.
/// What each hotcue pad shows for one deck.
///
/// The pads speak about the record, not about the instrument: an empty deck says nothing and the pads stay dark, and a
/// loaded deck shows its eight slots — the colour the DJ painted a cue with, or the palette's colour for a slot with no
/// cue yet (owner's rule, 24.11).
fn pad_colours(
    loaded: bool,
    cues: &[Option<dj_domain::SourceFrame>],
    loops: &[Option<dj_domain::FrameRange>],
    cue_colors: &[Option<String>],
    palette: &dj_controller_s4mk3::lights::Palette,
) -> [Option<dj_controller_s4mk3::lights::Pad>; 8] {
    let mut pads = [None; 8];
    if !loaded {
        return pads;
    }
    for (slot, pad) in pads.iter_mut().enumerate() {
        let has_cue = cues.get(slot).is_some_and(Option::is_some);
        let has_loop = loops.get(slot).is_some_and(Option::is_some);
        let painted = cue_colors
            .get(slot)
            .and_then(|colour| colour.as_deref())
            .and_then(dj_controller_s4mk3::lights::nearest_colour);
        // The window's own order, line for line (`DeckPanel.qml`: `pad.loop !== null ? loopColor : …`): a **loop wins
        // over a cue** in the same slot. Both can be there at once — the domain says a pad holds a point or a loop, never
        // both, but a track can carry the two — and putting the cue first made a loop pad go dark whenever the cue colour
        // was set to `off` (owner's log, 24.11: `cues [0, 1, 2] loops [1]`, `hotcue Off`).
        let colour = if has_loop {
            palette.loop_
        } else if has_cue {
            painted.unwrap_or(palette.hotcue)
        } else {
            palette.hotcue
        };
        *pad = Some(dj_controller_s4mk3::lights::Pad {
            colour,
            occupied: has_cue || has_loop,
        });
    }
    pads
}

fn facts_from(
    snapshot: &dj_domain::Snapshot,
    palette: &dj_controller_s4mk3::lights::Palette,
) -> [DeckFacts; 4] {
    let mut facts = [DeckFacts::default(); 4];
    for deck in &snapshot.decks {
        let index = deck.deck.index();
        if index >= facts.len() {
            continue;
        }
        // The pads say what is in the deck, so they light when a record is loaded and stay dark while the deck is
        // empty (owner's rule, 24.11). An occupied slot shows the colour the DJ painted it with; an empty one shows the
        // palette's hotcue colour, because a pad that cannot be found in the dark is a pad nobody presses.
        let hotcues = pad_colours(
            deck.track.is_some(),
            &deck.hotcues,
            &deck.pad_loops,
            &deck.cue_colors,
            palette,
        );
        facts[index] = DeckFacts {
            playing: deck.playing,
            monitoring: deck.cue,
            sync: !deck.sync.is_off(),
            fx: deck.fx.enabled,
            fx_kind: deck.fx.kind.index(),
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
    playing: Option<&Playing>,
) -> Result<(), String> {
    let Some(endpoint) = engine else {
        return Err(format!("--lights-show wants --engine <endpoint>. {USAGE}"));
    };
    // Without a device the show says what it would light: that is the half a person can check without the controller,
    // and the only half a machine with no panel can test at all.
    let _panel = claim_panel()?;
    let mut sink = if devices.is_empty() {
        None
    } else {
        Some(open_lights(devices)?)
    };
    let with_id = !body_only;
    let night_kept = night;
    // The panel lights up the moment the program starts: every control in the resting colour, so a dark stage shows
    // where the buttons are before any state arrives (owner's instruction, 23.11). The first snapshot then paints the
    // active controls over it.
    let glow = dj_controller_s4mk3::lights::report_at_rest(
        &dj_controller_s4mk3::lights::Show::default(),
        palette,
    );
    if let Some(sink) = sink.as_mut() {
        sink.send(&glow.wire(with_id))
            .map_err(|error| format!("the panel did not take the start-up glow: {error}"))?;
    } else {
        println!(
            "dj-controller: would light the whole panel in the resting colour ({})",
            dj_controller_s4mk3::lights::colour_name(palette.resting)
        );
    }
    let mut last: Option<Vec<u8>> = Some(glow.wire(with_id));
    if night_kept {
        println!(
            "dj-controller: lighting the panel from {endpoint}; the whole panel stays lit; Ctrl-C stops"
        );
    } else {
        // Said out loud, because otherwise the panel glows for a moment and then goes dark for no visible reason.
        println!(
            "dj-controller: lighting the panel from {endpoint}; 'controller.lights.night' is off, so inactive controls go dark on the first snapshot"
        );
    }
    // The engine may not be listening yet — the service is started beside it, and a restart of the engine must not
    // leave a dead service behind a silent log. Waiting and reconnecting is what makes "the buttons are dark" a thing
    // of the past rather than a thing to explain.
    let mut waiting_said = false;
    // One line a second at most: a fader that disagrees would otherwise flood the log the moment it is touched.
    let mut last_fader_note = std::time::Instant::now() - std::time::Duration::from_secs(5);
    let mut last_pads = String::new();
    loop {
        let (mut client, _welcome) = match dj_bridge::connect_with_timeout(
            std::path::Path::new(endpoint),
            Some(std::time::Duration::from_secs(2)),
        ) {
            Ok(pair) => pair,
            Err(error) => {
                if !waiting_said {
                    println!(
                        "dj-controller: no engine at {endpoint} yet ({error}); waiting for it"
                    );
                    waiting_said = true;
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
                continue;
            }
        };
        if waiting_said {
            println!("dj-controller: the engine answered; lighting the panel");
            waiting_said = false;
        }
        loop {
            let message = match client.recv_server() {
                Ok(message) => message,
                Err(error) => {
                    println!("dj-controller: the engine stopped answering ({error}); reconnecting");
                    break;
                }
            };
            let dj_bridge::ServerMessage::Snapshot(snapshot) = message else {
                continue;
            };
            let facts = facts_from(&snapshot, palette);
            // What the transports are doing, for the buttons whose meaning depends on it: a PLAY that only ever starts a
            // deck is a PLAY that cannot stop one.
            if let Some(playing) = playing
                && let Ok(mut state) = playing.lock()
            {
                for (index, deck) in facts.iter().enumerate() {
                    state.playing[index] = deck.playing;
                    state.monitoring[index] = deck.monitoring;
                }
                for (index, deck) in snapshot.decks.iter().enumerate().take(4) {
                    if let Some(asked) = state.fader_asked[index]
                        && (deck.fader - asked).abs() > 0.01
                    {
                        let now = std::time::Instant::now();
                        if now.duration_since(last_fader_note).as_millis() > 1000 {
                            last_fader_note = now;
                            println!(
                                "dj-controller: the engine has {} on the fader of {:?} where the panel asked for {asked:.3}",
                                deck.fader, deck.deck
                            );
                        }
                    }
                }
                for (index, deck) in snapshot.decks.iter().enumerate().take(4) {
                    for slot in 0..8 {
                        state.cues[index][slot] =
                            deck.hotcues.get(slot).is_some_and(Option::is_some);
                        state.loops[index][slot] =
                            deck.pad_loops.get(slot).is_some_and(Option::is_some);
                    }
                    state.active_loop[index] = deck.loop_region.is_some();
                    state.fx[index] = deck.fx;
                }
                state.browser_open = snapshot.browser.open;
                state.panel_left = snapshot.panel.left;
                state.panel_right = snapshot.panel.right;
                state.browser_cursor = snapshot.browser.cursor.clone();
            }
            let (side_left, side_right) = playing
                .and_then(|playing| {
                    playing
                        .lock()
                        .ok()
                        .map(|state| (state.panel_left.index(), state.panel_right.index()))
                })
                .unwrap_or((0, 1));
            let show = show_from_facts(&facts, [side_left, side_right]);
            // What the pads are made of, once per change: a pad that goes dark after a load is answered by looking at what the
            // snapshot actually carried, not by guessing which branch of the colouring was taken.
            let pads_now: Vec<String> = snapshot
                .decks
                .iter()
                .take(2)
                .map(|deck| {
                    let cues: Vec<usize> = (0..8)
                        .filter(|slot| deck.hotcues.get(*slot).is_some_and(Option::is_some))
                        .collect();
                    let loops: Vec<usize> = (0..8)
                        .filter(|slot| deck.pad_loops.get(*slot).is_some_and(Option::is_some))
                        .collect();
                    format!(
                        "{:?} loaded {} cues {cues:?} loops {loops:?}",
                        deck.deck,
                        deck.track.is_some()
                    )
                })
                .collect();
            let pads_line = pads_now.join("; ");
            if pads_line != last_pads {
                println!("dj-controller: pads {pads_line}");
                last_pads = pads_line;
            }
            // In a dark booth everything glows faintly and the active controls stand out; outside it the panel is dark
            // where nothing happens.
            let report = if night {
                dj_controller_s4mk3::lights::report_at_rest(&show, palette)
            } else {
                dj_controller_s4mk3::lights::report_with(&show, palette)
            };
            // The deck each side drives is marked on the panel itself: its select button is the bright one, so a hand can
            // see where a load would land without looking at the screen.
            let (panel_left, panel_right) = playing
                .and_then(|playing| {
                    playing
                        .lock()
                        .ok()
                        .map(|state| (state.panel_left, state.panel_right))
                })
                .unwrap_or((dj_domain::DeckId::A, dj_domain::DeckId::B));
            let mut report = report;
            paint_deck_select(&mut report, panel_left, panel_right, palette, night);
            let wire = report.wire(with_id);
            // Level meters, report 129: the panel's VU column per channel. Sent every snapshot, not only when the lights
            // change — a meter moves while the buttons do not (owner's request, 24.11).
            let mut meter_levels = [(0.0f32, false); 4];
            for (index, deck) in snapshot.decks.iter().enumerate().take(4) {
                let held = deck.levels.held_left.max(deck.levels.held_right);
                let peak = deck.levels.peak_left.max(deck.levels.peak_right);
                meter_levels[index] = (held, peak >= 0.999);
            }
            if let Some(sink) = sink.as_mut() {
                let meter = dj_controller_s4mk3::lights::meter_report(&meter_levels);
                sink.send(&meter.wire(with_id))
                    .map_err(|error| format!("the panel did not take the meter report: {error}"))?;
            }
            if last.as_ref() == Some(&wire) {
                continue;
            }
            match sink.as_mut() {
                Some(sink) => sink
                    .send(&wire)
                    .map_err(|error| format!("the panel did not take the report: {error}"))?,
                None => {
                    let mut lit: Vec<String> = report
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
                    let total = lit.len();
                    lit.truncate(6);
                    let rest = if total > lit.len() {
                        format!(" …и ещё {}", total - lit.len())
                    } else {
                        String::new()
                    };
                    println!(
                        "dj-controller: would light {total}: {}{rest}",
                        lit.join(", ")
                    );
                }
            }
            last = Some(wire);
        }
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
            Some((deck, command)) => {
                commands.push((control.trim().to_string(), Some(deck), command));
            }
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

/// One connection to the engine, kept open for as long as the service lives.
///
/// Opening a socket and shaking hands for every knob step was the bug behind "a fast fader stops before the top": each
/// command travelled on its own connection, the engine saw them in whatever order they arrived, and an older value could
/// land after a newer one. A knob is absolute, so the last value has to be the one that stays.
struct EngineLink {
    stream: std::os::unix::net::UnixStream,
    next_id: u64,
}

impl EngineLink {
    fn connect(endpoint: &str) -> Result<Self, String> {
        let mut stream = dj_bridge::transport::connect(std::path::Path::new(endpoint))
            .map_err(|error| format!("{endpoint} cannot be reached: {error}"))?;
        dj_bridge::write_frame(
            &mut stream,
            &ClientMessage::Hello {
                protocol: dj_domain::COMMAND_PROTOCOL.to_string(),
            },
        )
        .map_err(|error| format!("the engine refused the handshake: {error}"))?;
        Ok(Self { stream, next_id: 1 })
    }

    /// Sends one command and waits for its answer, so a failure is seen by the caller and not by silence.
    fn send(&mut self, command: &dj_domain::DjCommand) -> Result<(), String> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let scheduled = dj_domain::ScheduledCommand {
            id,
            // No clock of our own: the engine applies it at the next block, which is where a hand's command lands too.
            effective_sample: 0,
            command: command.clone(),
        };
        dj_bridge::write_frame(&mut self.stream, &ClientMessage::Command(scheduled))
            .map_err(|error| format!("the command could not be sent: {error}"))?;
        loop {
            let message: ServerMessage = dj_bridge::read_frame(&mut self.stream)
                .map_err(|error| format!("the engine stopped answering: {error}"))?;
            if let ServerMessage::Ack(ack) = message
                && ack.command_id == id
            {
                // A refusal is not a send. The engine says why — a deck that is playing refuses a load, because cutting
                // the track the room is dancing to is the one mistake a DJ never forgives — and swallowing that here
                // would leave the DJ pressing a button that does nothing at all.
                if let dj_domain::AckStatus::Rejected(reason) = ack.status {
                    return Err(format!("the engine refused it: {reason:?}"));
                }
                return Ok(());
            }
        }
    }
}

/// Sends the commands to the engine and waits for their answers.
fn send_commands(
    endpoint: &str,
    commands: &[(String, Option<dj_domain::DeckId>, dj_domain::DjCommand)],
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
    fn cue_at(frame: u64) -> Option<dj_domain::SourceFrame> {
        Some(dj_domain::SourceFrame(frame))
    }

    fn pad(
        colour: dj_controller_s4mk3::Colour,
        occupied: bool,
    ) -> dj_controller_s4mk3::lights::Pad {
        dj_controller_s4mk3::lights::Pad { colour, occupied }
    }

    #[test]
    fn an_empty_deck_leaves_its_pads_dark() {
        // The pads are about the record, not about the instrument: nothing loaded, nothing to say.
        let palette = dj_controller_s4mk3::lights::Palette::default();
        assert_eq!(
            super::pad_colours(false, &[], &[], &[], &palette),
            [None; 8]
        );
    }

    #[test]
    fn a_loaded_deck_lights_every_pad_and_the_stored_ones_brightly() {
        // A pad that cannot be found in the dark is a pad nobody presses, and a pad that shines like a stored cue
        // promises one it does not have (owner's rule, 24.11).
        let palette = dj_controller_s4mk3::lights::Palette::default();
        let cues = vec![cue_at(4800), None];
        let pads = super::pad_colours(true, &cues, &[], &[], &palette);
        assert_eq!(pads[0], Some(pad(palette.hotcue, true)), "a stored cue");
        assert_eq!(pads[1], Some(pad(palette.hotcue, false)), "an empty slot");
        assert_eq!(
            pads[7],
            Some(pad(palette.hotcue, false)),
            "past the cue list"
        );
    }

    #[test]
    fn a_loop_shows_the_colour_the_screen_draws_it_with() {
        // The screen draws a loop amber; the panel's nearest colour is yellow, and the pad says "loop" rather than
        // "cue" (owner's report, 24.11: saved loops did not light at all).
        let palette = dj_controller_s4mk3::lights::Palette::default();
        let loops = vec![Some(dj_domain::FrameRange {
            start: dj_domain::SourceFrame(0),
            end: dj_domain::SourceFrame(4800),
        })];
        let pads = super::pad_colours(true, &[], &loops, &[], &palette);
        assert_eq!(
            pads[0],
            Some(pad(dj_controller_s4mk3::Colour::Yellow, true)),
            "a stored loop is yellow and bright"
        );
        assert_eq!(
            pads[1],
            Some(pad(palette.hotcue, false)),
            "the next slot is still empty"
        );
    }

    #[test]
    fn a_loop_wins_over_a_cue_in_the_same_slot_as_on_the_screen() {
        // The window colours a pad `pad.loop !== null ? loopColor : …`, so a slot holding both shows the loop. With the cue
        // colour set to `off` the panel used to go dark exactly where the screen showed an amber loop.
        let palette = dj_controller_s4mk3::lights::Palette {
            hotcue: dj_controller_s4mk3::Colour::Off,
            ..dj_controller_s4mk3::lights::Palette::default()
        };
        let cues = vec![cue_at(1)];
        let loops = vec![Some(dj_domain::FrameRange {
            start: dj_domain::SourceFrame(0),
            end: dj_domain::SourceFrame(4800),
        })];
        let pads = super::pad_colours(true, &cues, &loops, &[], &palette);
        assert_eq!(
            pads[0],
            Some(pad(dj_controller_s4mk3::Colour::Yellow, true)),
            "the loop is what the pad is about"
        );
    }

    #[test]
    fn a_painted_cue_keeps_its_own_colour_and_the_others_do_not() {
        let palette = dj_controller_s4mk3::lights::Palette::default();
        let cues = vec![cue_at(1), cue_at(2), cue_at(3)];
        let colours = vec![
            Some("#cc0000".to_string()),
            None,
            Some("not a colour".to_string()),
        ];
        let pads = super::pad_colours(true, &cues, &[], &colours, &palette);
        assert_eq!(pads[0], Some(pad(dj_controller_s4mk3::Colour::Red, true)));
        assert_eq!(pads[1], Some(pad(palette.hotcue, true)), "an unpainted cue");
        assert_eq!(
            pads[2],
            Some(pad(palette.hotcue, true)),
            "a broken colour is no colour"
        );
    }

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
                fx_kind: 0,
                hotcues: [Some(dj_controller_s4mk3::lights::Pad {
                    colour: dj_controller_s4mk3::Colour::Green,
                    occupied: true,
                }); 8],
            },
            DeckFacts::default(),
            DeckFacts::default(),
            DeckFacts::default(),
        ];
        let show = show_from_facts(&decks, [0, 1]);
        assert!(show.playing[0] && !show.playing[1]);
        assert!(show.monitoring[0] && !show.monitoring[1]);
        assert!(!show.sync[0] && !show.sync[1]);
        assert!(show.hotcues[0].iter().all(|slot| slot.is_some()));
        assert!(show.hotcues[1].iter().all(|slot| slot.is_none()));
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
        let show = show_from_facts(&decks, [0, 1]);
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
            None,
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
    #[allow(clippy::expect_used)]
    fn shift_sends_the_encoder_to_the_left_block() {
        // Shift held on a deck turns the browse encoder into a walk through the collection, the playlists and the files, and
        // its press into opening or closing the level the rail stands on (owner's report, 24.11: with Shift the cursor still
        // moved through the track list).
        let playing: Playing =
            std::sync::Arc::new(std::sync::Mutex::new(super::DeckFactsShared::default()));
        let right = dj_controller_core::ControlEvent::level("panel.encoder", 1.0);
        let press = dj_controller_core::ControlEvent::button("panel.encoder_press", true);
        {
            let mut state = playing.lock().expect("the state");
            state.browser_cursor = Some("/music/one.flac".to_string());
        }
        assert_eq!(
            super::panel_command(&right, &playing),
            Some(dj_domain::DjCommand::BrowserStep { delta: 1 }),
            "no Shift: the rows of the list"
        );
        {
            let mut state = playing.lock().expect("the state");
            state.shift[0] = true;
        }
        assert_eq!(
            super::panel_command(&right, &playing),
            Some(dj_domain::DjCommand::BrowserPlaceStep { delta: 1 }),
            "Shift: the left block"
        );
        assert_eq!(
            super::panel_command(&press, &playing),
            Some(dj_domain::DjCommand::BrowserPlaceToggle),
            "Shift + press: open or close the level"
        );
        // One browse encoder, two Shift keys: with either held, a press opens or closes the level instead of loading a track
        // under the DJ's hands.
        let right_press =
            dj_controller_core::ControlEvent::button("panel.encoder_press_right", true);
        assert_eq!(
            super::panel_command(&right_press, &playing),
            Some(dj_domain::DjCommand::BrowserPlaceToggle)
        );
        {
            let mut state = playing.lock().expect("the state");
            state.shift[0] = false;
        }
        assert_eq!(
            super::panel_command(&right_press, &playing),
            Some(dj_domain::DjCommand::Load {
                deck: dj_domain::DeckId::B,
                path: "/music/one.flac".to_string()
            }),
            "no Shift: the press loads the row"
        );
    }

    #[test]
    fn a_press_is_judged_by_the_deck_the_side_drives() {
        let state = super::DeckFactsShared {
            panel_left: dj_domain::DeckId::C,
            ..super::DeckFactsShared::default()
        };
        // The left half of the panel still says `deck_a` in the map, but it commands C.
        assert_eq!(
            super::driving_deck("deck_a.play_pause", &state),
            Some(dj_domain::DeckId::C)
        );
        assert_eq!(
            super::driving_deck("deck_b.play_pause", &state),
            Some(dj_domain::DeckId::B)
        );
        // A mixer control keeps its own deck: the channel fader is a channel, not a side.
        assert_eq!(
            super::driving_deck("deck_c.fader", &state),
            Some(dj_domain::DeckId::C)
        );
    }

    #[test]
    fn the_deck_each_side_drives_is_the_bright_one() {
        let palette = dj_controller_s4mk3::lights::Palette::default();
        let mut report = dj_controller_s4mk3::lights::report_at_rest(
            &dj_controller_s4mk3::lights::Show::default(),
            &palette,
        );
        super::paint_deck_select(
            &mut report,
            dj_domain::DeckId::A,
            dj_domain::DeckId::B,
            &palette,
            true,
        );
        let byte = |deck| usize::from(super::deck_select_byte(deck).unwrap_or(0));
        let bright = dj_controller_s4mk3::lights::bright(palette.resting.code());
        let faint = dj_controller_s4mk3::lights::dim(palette.resting.code());
        assert_eq!(
            report.body[byte(dj_domain::DeckId::A)],
            bright,
            "the deck the left side drives is bright"
        );
        assert_eq!(
            report.body[byte(dj_domain::DeckId::C)],
            faint,
            "the other deck of that side sits back"
        );
        // The switch moves the light with it.
        super::paint_deck_select(
            &mut report,
            dj_domain::DeckId::C,
            dj_domain::DeckId::D,
            &palette,
            true,
        );
        assert_eq!(report.body[byte(dj_domain::DeckId::C)], bright);
        assert_eq!(report.body[byte(dj_domain::DeckId::A)], faint);
        assert_eq!(report.body[byte(dj_domain::DeckId::D)], bright);
        assert_eq!(report.body[byte(dj_domain::DeckId::B)], faint);
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn the_browser_controls_say_what_the_panel_asks() {
        let playing: Playing =
            std::sync::Arc::new(std::sync::Mutex::new(super::DeckFactsShared::default()));
        // VIEW asks for the opposite of what is on screen, so one button does both.
        let view = dj_controller_core::ControlEvent::button("panel.view", true);
        assert_eq!(
            super::panel_command(&view, &playing),
            Some(dj_domain::DjCommand::BrowserShow { open: false })
        );
        let release = dj_controller_core::ControlEvent::button("panel.view", false);
        assert_eq!(
            super::panel_command(&release, &playing),
            None,
            "letting go asks for nothing"
        );
        // A turn is one row: right is 1.0, left is 0.0.
        let right = dj_controller_core::ControlEvent::level("panel.encoder", 1.0);
        assert_eq!(
            super::panel_command(&right, &playing),
            Some(dj_domain::DjCommand::BrowserStep { delta: 1 })
        );
        let left = dj_controller_core::ControlEvent::level("panel.encoder", 0.0);
        assert_eq!(
            super::panel_command(&left, &playing),
            Some(dj_domain::DjCommand::BrowserStep { delta: -1 })
        );
        // A press with nothing selected loads nothing, and says so instead of guessing a row.
        let press = dj_controller_core::ControlEvent::button("panel.encoder_press", true);
        assert_eq!(super::panel_command(&press, &playing), None);
        // With a row selected the standing deck takes it.
        {
            let mut state = playing.lock().expect("the state");
            state.browser_cursor = Some("/music/one.flac".to_string());
        }
        assert_eq!(
            super::panel_command(&press, &playing),
            Some(dj_domain::DjCommand::Load {
                deck: dj_domain::DeckId::A,
                path: "/music/one.flac".to_string()
            })
        );
        // The right encoder loads into the right side's deck, whatever the left one is doing.
        let press_right =
            dj_controller_core::ControlEvent::button("panel.encoder_press_right", true);
        assert_eq!(
            super::panel_command(&press_right, &playing),
            Some(dj_domain::DjCommand::Load {
                deck: dj_domain::DeckId::B,
                path: "/music/one.flac".to_string()
            })
        );
        // The deck switch moves a side to its second deck, and a press follows it there.
        let switch = dj_controller_core::ControlEvent::button("panel.deck_c", true);
        assert_eq!(
            super::panel_deck_command("panel.deck_c").map(|(deck, _)| deck),
            Some(dj_domain::DeckId::C),
            "the C button names deck C, it does not toggle"
        );
        assert_eq!(
            super::panel_deck_command("panel.deck_d").map(|(deck, _)| deck),
            Some(dj_domain::DeckId::D)
        );
        {
            let mut state = playing.lock().expect("the state");
            state.panel_left = dj_domain::DeckId::C;
        }
        assert_eq!(
            super::panel_command(&press, &playing),
            Some(dj_domain::DjCommand::Load {
                deck: dj_domain::DeckId::C,
                path: "/music/one.flac".to_string()
            })
        );
        let _ = switch;
    }

    #[test]
    fn a_fader_is_never_mistaken_for_a_second_touch() {
        // The whole travel of a fader is one gesture: every position must be taken, however fast they come.
        for step in 0..=100 {
            let value = step as f32 / 100.0;
            assert!(
                !super::is_double_touch(true, value, Some(0), 10),
                "step {step} was swallowed"
            );
        }
        // A button pressed twice within 150 ms is a light touch, not two gestures.
        assert!(super::is_double_touch(false, 1.0, Some(0), 100));
        assert!(!super::is_double_touch(false, 1.0, Some(0), 400));
        assert!(
            !super::is_double_touch(false, 0.0, Some(0), 100),
            "a release is not a press"
        );
    }

    #[test]
    fn serve_without_an_engine_is_refused() {
        // A service that lights nothing and hears nothing is worse than a refusal with a reason.
        let Err(error) = run(&["--serve".to_string()]) else {
            panic!("a service without an engine must be refused");
        };
        assert!(error.contains("--engine"), "{error}");
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
