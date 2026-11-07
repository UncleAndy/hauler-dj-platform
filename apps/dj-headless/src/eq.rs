//! The АЧХ stand for the channel EQ (A14): the response that is *measured*, not read off the code.
//!
//! A curve computed from the coefficients and a filter that is actually run are two different
//! claims, and the difference between them was a real defect: a sign in the evaluation made the
//! reported response disagree with the sound by six decibels while every number looked plausible.
//! So this mode runs a sine at each frequency through the real `ChannelEq`, block by block with the
//! same block size the callback uses, measures the settled RMS and prints that next to the reported
//! number.

use std::path::PathBuf;

use dj_domain::{EqBand, EqGains};
use dj_dsp::ChannelEq;
use serde_json::json;

/// Frequencies a sweep reports, in hertz. Chosen to straddle every corner of the three bands and
/// both ends of the audible range.
pub const FREQUENCIES: [f64; 13] = [
    20.0, 40.0, 60.0, 100.0, 200.0, 400.0, 1_000.0, 2_000.0, 4_000.0, 8_000.0, 12_000.0, 16_000.0,
    20_000.0,
];

/// Block size the sweep drives, so the filter is exercised the way the callback exercises it.
const BLOCK: usize = 256;

/// How far a measured response may sit from the reported one, in decibels.
pub const TOLERANCE_DB: f64 = 0.2;

/// How flat a zeroed EQ must measure, in decibels — the A14 acceptance.
pub const FLAT_DB: f64 = 0.1;

/// One sweep: which gains to set, at which rate, and where to write the numbers.
pub struct EqRun {
    /// The three band gains as asked for.
    pub gains: EqGains,
    /// Device sample rate the table is built for.
    pub rate: u32,
    /// Where the JSON report goes; `None` prints only the summary line.
    pub out: Option<PathBuf>,
}

/// One row of the sweep.
struct Point {
    hz: f64,
    measured: f64,
    reported: f64,
}

pub fn parse(args: &[String]) -> Result<EqRun, String> {
    let mut gains = EqGains::FLAT;
    let mut rate: u32 = 48_000;
    let mut out: Option<PathBuf> = None;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].as_str();
        let value = |index: usize| -> Result<&String, String> {
            args.get(index + 1)
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match flag {
            "--low" | "--mid" | "--high" => {
                let text = value(index)?;
                let decibels: f32 = text
                    .parse()
                    .map_err(|_| format!("{flag}: {text:?} is not a number"))?;
                let band = match flag {
                    "--low" => EqBand::Low,
                    "--mid" => EqBand::Mid,
                    _ => EqBand::High,
                };
                gains.set_band(band, decibels);
                index += 2;
            }
            "--rate" => {
                let text = value(index)?;
                rate = text
                    .parse()
                    .map_err(|_| format!("--rate: {text:?} is not a sample rate"))?;
                index += 2;
            }
            "--out" => {
                out = Some(PathBuf::from(value(index)?));
                index += 2;
            }
            other => return Err(format!("unknown flag {other:?} for eq")),
        }
    }
    Ok(EqRun { gains, rate, out })
}

/// Settled gain of a sine through the real filter, in decibels.
///
/// One second of signal, the second half measured: the transient of the sections is gone long
/// before that at any audio frequency, and the RMS of a sine is its amplitude over sqrt(2).
fn measured_db(gains: EqGains, rate: f64, frequency: f64) -> f64 {
    let mut eq = ChannelEq::new(rate);
    eq.set_gains(gains);
    let total = rate.max(1024.0) as usize;
    let mut buffer = vec![0.0f32; BLOCK * 2];
    let mut energy = 0.0f64;
    let mut counted = 0u64;
    let mut frame = 0usize;
    while frame < total {
        let frames = BLOCK.min(total - frame);
        for index in 0..frames {
            let seconds = (frame + index) as f64 / rate;
            let value = (std::f64::consts::TAU * frequency * seconds).sin() as f32;
            buffer[index * 2] = value;
            buffer[index * 2 + 1] = value;
        }
        eq.process(&mut buffer[..frames * 2]);
        if frame >= total / 2 {
            for pair in buffer[..frames * 2].chunks_exact(2) {
                energy += f64::from(pair[0]) * f64::from(pair[0]);
                counted += 1;
            }
        }
        frame += frames;
    }
    let rms = (energy / counted.max(1) as f64).sqrt();
    let gain = (rms * std::f64::consts::SQRT_2).max(1e-12);
    20.0 * gain.log10()
}

/// Run the sweep, write the report and say whether every criterion held.
///
/// The criteria are the acceptance of A14: a flat EQ is flat inside 0,1 dB, each band reaches its
/// own gain inside its own range, and the curve the engine would report agrees with what the filter
/// does.
pub fn run(run: &EqRun) -> Result<(String, bool), String> {
    let rate = f64::from(run.rate);
    let mut eq = ChannelEq::new(rate);
    eq.set_gains(run.gains);
    // What the engine actually applied: the knob grid can move the request by up to half a step.
    let applied = eq.gains();

    let mut points = Vec::with_capacity(FREQUENCIES.len());
    for frequency in FREQUENCIES {
        points.push(Point {
            hz: frequency,
            measured: measured_db(applied, rate, frequency),
            reported: eq.magnitude_db(frequency),
        });
    }

    let mut failures: Vec<String> = Vec::new();
    for point in &points {
        if (point.measured - point.reported).abs() > TOLERANCE_DB {
            failures.push(format!(
                "{} Hz: measured {:.2} dB, reported {:.2} dB",
                point.hz, point.measured, point.reported
            ));
        }
    }

    let worst_flat = points
        .iter()
        .map(|point| point.measured.abs())
        .fold(0.0f64, f64::max);
    if applied.is_flat() {
        for point in &points {
            if point.measured.abs() > FLAT_DB {
                failures.push(format!(
                    "{} Hz: a flat EQ measured {:.2} dB",
                    point.hz, point.measured
                ));
            }
        }
    } else {
        // Each band has to reach its own gain where the ear expects it: a shelf at 200 Hz is at
        // full lift an octave below, a peak at its centre, a high shelf near the top.
        for (band, hz) in [
            (EqBand::Low, 40.0),
            (EqBand::Mid, 1_000.0),
            (EqBand::High, 16_000.0),
        ] {
            let asked = f64::from(applied.band(band));
            if asked == 0.0 {
                continue;
            }
            let measured = points
                .iter()
                .find(|point| point.hz == hz)
                .map(|point| point.measured)
                .unwrap_or(0.0);
            if (measured - asked).abs() > 1.0 {
                failures.push(format!(
                    "{} band: asked {asked:.2} dB, measured {measured:.2} dB at {hz} Hz",
                    band.as_str()
                ));
            }
        }
    }

    let rows: Vec<serde_json::Value> = points
        .iter()
        .map(|point| {
            json!({
                "hz": point.hz,
                "measured_db": round(point.measured, 2),
                "reported_db": round(point.reported, 2),
                "error_db": round(point.measured - point.reported, 3),
            })
        })
        .collect();

    let report = json!({
        "rate": run.rate,
        "asked": {
            "low_db": run.gains.low_db,
            "mid_db": run.gains.mid_db,
            "high_db": run.gains.high_db,
        },
        "applied": {
            "low_db": applied.low_db,
            "mid_db": applied.mid_db,
            "high_db": applied.high_db,
        },
        "flat_max_db": round(worst_flat, 3),
        "tolerance_db": TOLERANCE_DB,
        "points": rows,
        "failures": failures,
    });

    if let Some(path) = &run.out {
        let text = serde_json::to_string_pretty(&report).map_err(|err| err.to_string())?;
        std::fs::write(path, text).map_err(|err| format!("{}: {err}", path.display()))?;
    }

    let summary = format!(
        "eq: low {:+.2} mid {:+.2} high {:+.2} dB applied; worst |measured - reported| {:.2} dB;          flat max {:.3} dB; {} point(s) swept",
        applied.low_db,
        applied.mid_db,
        applied.high_db,
        points
            .iter()
            .map(|point| (point.measured - point.reported).abs())
            .fold(0.0f64, f64::max),
        worst_flat,
        points.len()
    );
    Ok((summary, failures.is_empty()))
}

fn round(value: f64, places: u32) -> f64 {
    let scale = 10f64.powi(places as i32);
    (value * scale).round() / scale
}
