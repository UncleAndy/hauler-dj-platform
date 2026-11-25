//! Native Instruments Traktor Kontrol S4 MK3.
//!
//! Written from `docs/hardware/s4mk3/protocol-notes.md` and nothing else: the notes say which byte holds what and how
//! sure we are of it. Facts marked `public-source` there are pointers until a capture on the device confirms them, so
//! this driver parses only what the notes state outright and answers nothing for the rest — a wrong control is worse
//! than a quiet one.
//!
//! Three directions, in the order they become safe:
//!
//! 1. **Reports in.** The jog wheel report (`3`) carries two 32-bit positions and a 32-bit timer; its offsets are the
//!    ones the notes give. The button and knob reports (`1`, `2`) have no verified map yet, so they produce nothing.
//! 2. **Screens out.** A frame is a rectangle of RGB565, big-endian, in a 16-byte header and a 4-byte footer. Building
//!    the bytes is safe and testable; sending them needs a bus the program does not open yet.
//! 3. **Motors out.** Refused on purpose: ADR-0006 keeps the motors behind its own safety decision, so every motor
//!    call here returns [`MotorsNotPermitted`] rather than a packet.
//!
//! - Backlog task: A09
//! - Requirements: DJP-CTRL-002, DJP-CTRL-003, DJP-CTRL-005
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

use std::fmt;

use dj_controller_core::{ControlEvent, Driver, WheelCounter};

/// USB vendor id.
pub mod inputs;
pub mod lights;
pub mod panels;
pub mod screen;
pub mod text;
pub mod waveform;

pub const VENDOR: u16 = 0x17cc;
/// USB product id.
pub const PRODUCT: u16 = 0x1720;
/// The name the kernel reports.
pub const NAME: &str = "Traktor Kontrol S4 MK3";

/// Width of one screen, in pixels.
pub const SCREEN_WIDTH: u16 = 320;
/// Height of one screen, in pixels.
pub const SCREEN_HEIGHT: u16 = 240;

/// The report that carries the jog wheels: two positions and a timer.
pub const REPORT_WHEELS: u8 = 3;
/// Where the timer starts in that report.
const WHEEL_TIMER: usize = 8;
/// Where the left wheel's position starts.
const WHEEL_LEFT: usize = 12;
/// Where the right wheel's position starts.
const WHEEL_RIGHT: usize = 40;

/// Size of a frame's header, in bytes.
const FRAME_HEADER: usize = 16;
/// Size of a frame's footer, in bytes.
const FRAME_FOOTER: usize = 4;

/// Which screen a frame is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// The left pair of decks.
    Left = 0,
    /// The right pair of decks.
    Right = 1,
}

/// A rectangle of a screen: the whole thing, or a part that changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    /// Left edge, in pixels.
    pub x: u16,
    /// Top edge, in pixels.
    pub y: u16,
    /// Width, in pixels.
    pub width: u16,
    /// Height, in pixels.
    pub height: u16,
}

impl Rect {
    /// The whole screen.
    #[must_use]
    pub fn full() -> Self {
        Self {
            x: 0,
            y: 0,
            width: SCREEN_WIDTH,
            height: SCREEN_HEIGHT,
        }
    }

    /// How many pixels the rectangle holds.
    #[must_use]
    pub fn pixels(self) -> usize {
        usize::from(self.width) * usize::from(self.height)
    }
}

/// Why a frame cannot be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    /// The pixel count and the rectangle disagree; a frame shorter than its own header would show garbage.
    PixelCount { expected: usize, got: usize },
    /// The rectangle reaches outside the screen.
    OutsideScreen,
}

impl fmt::Display for FrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PixelCount { expected, got } => {
                write!(
                    formatter,
                    "the rectangle wants {expected} pixels, not {got}"
                )
            }
            Self::OutsideScreen => write!(formatter, "the rectangle does not fit a 320x240 screen"),
        }
    }
}

impl std::error::Error for FrameError {}

/// The motors are not ours to drive yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MotorsNotPermitted;

impl fmt::Display for MotorsNotPermitted {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "the motors stay off until the safety decision ADR-0006 asks for: no motor packet is ever built"
        )
    }
}

impl std::error::Error for MotorsNotPermitted {}

/// The eighteen colours the LEDs take, in the order the device numbers them.
///
/// The number is the byte the device wants; the step of four between neighbours is the device's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Colour {
    /// No light.
    Off,
    /// Red.
    Red,
    /// Carrot.
    Carrot,
    /// Orange.
    Orange,
    /// Honey.
    Honey,
    /// Yellow.
    Yellow,
    /// Lime.
    Lime,
    /// Green.
    Green,
    /// Aqua.
    Aqua,
    /// Celeste.
    Celeste,
    /// Sky.
    Sky,
    /// Blue.
    Blue,
    /// Purple.
    Purple,
    /// Fuscia, spelled as the device names it.
    Fuscia,
    /// Magenta.
    Magenta,
    /// Azalea.
    Azalea,
    /// Salmon.
    Salmon,
    /// White.
    White,
}

impl Colour {
    /// The byte to write into an LED's place in the output report.
    #[must_use]
    pub fn code(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::Red => 4,
            Self::Carrot => 8,
            Self::Orange => 12,
            Self::Honey => 16,
            Self::Yellow => 20,
            Self::Lime => 24,
            Self::Green => 28,
            Self::Aqua => 32,
            Self::Celeste => 36,
            Self::Sky => 40,
            Self::Blue => 44,
            Self::Purple => 48,
            Self::Fuscia => 52,
            Self::Magenta => 56,
            Self::Azalea => 60,
            Self::Salmon => 64,
            Self::White => 68,
        }
    }

    /// Every colour, for a test that walks them all.
    #[must_use]
    pub fn all() -> [Self; 18] {
        [
            Self::Off,
            Self::Red,
            Self::Carrot,
            Self::Orange,
            Self::Honey,
            Self::Yellow,
            Self::Lime,
            Self::Green,
            Self::Aqua,
            Self::Celeste,
            Self::Sky,
            Self::Blue,
            Self::Purple,
            Self::Fuscia,
            Self::Magenta,
            Self::Azalea,
            Self::Salmon,
            Self::White,
        ]
    }
}

/// The tempo fader's own scale: `0…4096` across 77 mm of travel.
pub const TEMPO_RANGE: u16 = 4096;
/// Ticks per millimetre of travel.
pub const TEMPO_TICKS_PER_MM: f64 = 4096.0 / 77.0;

/// One deck's tempo fader, with its own centre.
///
/// The two decks do not share a centre, so this is a per-deck calibration and not a constant: a fader at rest in the
/// middle of its travel is not necessarily at zero.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TempoFader {
    centre: f64,
}

impl TempoFader {
    /// A fader whose centre was measured at `centre` ticks.
    #[must_use]
    pub fn new(centre: f64) -> Self {
        Self {
            centre: centre.clamp(0.0, f64::from(TEMPO_RANGE)),
        }
    }

    /// A fader calibrated at the middle of its travel, which is where an uncalibrated deck starts.
    #[must_use]
    pub fn centred() -> Self {
        Self::new(f64::from(TEMPO_RANGE) / 2.0)
    }

    /// The measured centre.
    #[must_use]
    pub fn centre(&self) -> f64 {
        self.centre
    }

    /// Where the fader is, from `-1.0` at the bottom to `1.0` at the top.
    #[must_use]
    pub fn position(&self, raw: u16) -> f64 {
        let half = f64::from(TEMPO_RANGE) / 2.0;
        ((f64::from(raw.min(TEMPO_RANGE)) - self.centre) / half).clamp(-1.0, 1.0)
    }
}

/// The device, as a driver sees it.
#[derive(Debug, Clone, Default)]
pub struct S4Mk3 {
    wheels: [WheelCounter; 2],
}

impl S4Mk3 {
    /// A driver that has seen nothing yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Forgets the wheel counters, for a device that was unplugged and plugged back in.
    pub fn reset(&mut self) {
        for wheel in &mut self.wheels {
            wheel.reset();
        }
    }
}

impl Driver for S4Mk3 {
    fn name(&self) -> &'static str {
        "s4mk3"
    }

    fn feed(&mut self, report: &[u8]) -> Vec<ControlEvent> {
        let Some(&id) = report.first() else {
            return Vec::new();
        };
        if id != REPORT_WHEELS {
            // Reports 1 and 2 hold buttons and knobs whose offsets are not verified yet; parsing them by eye would put
            // the wrong control on the wrong byte, and a DJ would find out on stage.
            return Vec::new();
        }
        let timer = dj_controller_core::report::u32_le(report, WHEEL_TIMER);
        let left = dj_controller_core::report::u32_le(report, WHEEL_LEFT);
        let right = dj_controller_core::report::u32_le(report, WHEEL_RIGHT);
        // The timer is read only to know the report is long enough; the rate comes from the host's clock, because the
        // timer's unit is not written down anywhere and a speed scaled by an unknown constant cannot be calibrated.
        let (Some(_timer), Some(left), Some(right)) = (timer, left, right) else {
            return Vec::new();
        };
        let now = std::time::Instant::now();
        let mut events = Vec::with_capacity(2);
        if let Some(ticks) = self.wheels[0].ticks_per_second(left, now) {
            events.push(ControlEvent::level("wheel_left", ticks as f32));
        }
        if let Some(ticks) = self.wheels[1].ticks_per_second(right, now) {
            events.push(ControlEvent::level("wheel_right", ticks as f32));
        }
        events
    }
}

/// One screen frame: header, pixels as RGB565 big-endian, footer.
///
/// The device takes rectangles, not pictures, so a clock ticking does not need the whole screen sent again.
pub fn screen_frame(screen: Screen, rect: Rect, pixels: &[u16]) -> Result<Vec<u8>, FrameError> {
    if usize::from(rect.x) + usize::from(rect.width) > usize::from(SCREEN_WIDTH)
        || usize::from(rect.y) + usize::from(rect.height) > usize::from(SCREEN_HEIGHT)
    {
        return Err(FrameError::OutsideScreen);
    }
    let expected = rect.pixels();
    if pixels.len() != expected {
        return Err(FrameError::PixelCount {
            expected,
            got: pixels.len(),
        });
    }
    let mut frame = Vec::with_capacity(FRAME_HEADER + expected * 2 + FRAME_FOOTER);
    frame.extend_from_slice(&[0x84, 0x00, screen as u8, 0x21, 0, 0, 0, 0]);
    frame.extend_from_slice(&rect.x.to_be_bytes());
    frame.extend_from_slice(&rect.y.to_be_bytes());
    frame.extend_from_slice(&rect.width.to_be_bytes());
    frame.extend_from_slice(&rect.height.to_be_bytes());
    for pixel in pixels {
        frame.extend_from_slice(&pixel.to_be_bytes());
    }
    frame.extend_from_slice(&[0x40, 0x00, screen as u8, 0x00]);
    Ok(frame)
}

/// The whole screen at once, which is the case a picture of a waveform falls into.
pub fn full_screen_frame(screen: Screen, pixels: &[u16]) -> Result<Vec<u8>, FrameError> {
    screen_frame(screen, Rect::full(), pixels)
}

/// Turns an RGB colour into the 16 bits a screen takes: five bits of red, six of green, five of blue.
#[must_use]
pub fn rgb565(red: u8, green: u8, blue: u8) -> u16 {
    let red = u16::from(red >> 3);
    let green = u16::from(green >> 2);
    let blue = u16::from(blue >> 3);
    (red << 11) | (green << 5) | blue
}

/// A motor packet — which this build never produces.
///
/// The signature exists so that the day the safety decision lands, there is one place to change and a test that says
/// what changed; until then the answer is a refusal, not a packet written into a device we do not fully understand.
pub fn motor_packet(_left: i16, _right: i16) -> Result<[u8; 10], MotorsNotPermitted> {
    Err(MotorsNotPermitted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wheel_report(timer: u32, left: u32, right: u32) -> Vec<u8> {
        let mut report = vec![0_u8; 44];
        report[0] = REPORT_WHEELS;
        report[WHEEL_TIMER..WHEEL_TIMER + 4].copy_from_slice(&timer.to_le_bytes());
        report[WHEEL_LEFT..WHEEL_LEFT + 4].copy_from_slice(&left.to_le_bytes());
        // The report repeats the timer at 36, and the right position follows it.
        report[36..40].copy_from_slice(&timer.to_le_bytes());
        report[WHEEL_RIGHT..WHEEL_RIGHT + 4].copy_from_slice(&right.to_le_bytes());
        report
    }

    #[test]
    fn the_first_wheel_report_moves_nothing() {
        let mut driver = S4Mk3::new();
        assert!(driver.feed(&wheel_report(1000, 500, 500)).is_empty());
    }

    #[test]
    fn a_moving_wheel_reports_its_speed() {
        let mut driver = S4Mk3::new();
        let _ = driver.feed(&wheel_report(1000, 500, 500));
        // A real gap, because the rate is now by the wall clock: two reports in the same instant would be no time at all.
        std::thread::sleep(std::time::Duration::from_millis(2));
        let events = driver.feed(&wheel_report(1100, 620, 400));
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].control, "wheel_left");
        // Forward is positive, backward negative: the sign is what the shell turns into a scratch or a bend.
        assert!(
            events[0].value > 0.0,
            "left went forward: {}",
            events[0].value
        );
        assert_eq!(events[1].control, "wheel_right");
        assert!(
            events[1].value < 0.0,
            "right went back: {}",
            events[1].value
        );
    }

    #[test]
    fn a_short_report_is_not_guessed() {
        let mut driver = S4Mk3::new();
        assert!(driver.feed(&[REPORT_WHEELS, 0, 0]).is_empty());
        assert!(driver.feed(&[]).is_empty());
    }

    #[test]
    fn reports_without_a_verified_map_produce_nothing() {
        let mut driver = S4Mk3::new();
        assert!(driver.feed(&[1, 0xff, 0xff, 0xff, 0xff]).is_empty());
        assert!(driver.feed(&[2, 0xff, 0xff, 0xff, 0xff]).is_empty());
    }

    #[test]
    fn a_reconnect_starts_the_wheels_over() {
        let mut driver = S4Mk3::new();
        let _ = driver.feed(&wheel_report(1000, 500, 500));
        driver.reset();
        assert!(driver.feed(&wheel_report(9000, 5, 5)).is_empty());
    }

    #[test]
    fn a_full_frame_is_a_header_a_rectangle_and_a_footer() {
        let pixels = vec![0x1234_u16; usize::from(SCREEN_WIDTH) * usize::from(SCREEN_HEIGHT)];
        let frame = full_screen_frame(Screen::Left, &pixels);
        let frame = match frame {
            Ok(frame) => frame,
            Err(error) => panic!("a full screen of pixels must build: {error}"),
        };
        assert_eq!(frame.len(), 16 + 320 * 240 * 2 + 4);
        assert_eq!(&frame[..8], &[0x84, 0x00, 0x00, 0x21, 0, 0, 0, 0]);
        // x = 0, y = 0, width = 320, height = 240, all big-endian.
        assert_eq!(
            &frame[8..16],
            &[0x00, 0x00, 0x00, 0x00, 0x01, 0x40, 0x00, 0xf0]
        );
        // The first pixel, high byte first.
        assert_eq!(&frame[16..18], &[0x12, 0x34]);
        assert_eq!(&frame[frame.len() - 4..], &[0x40, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn the_right_screen_says_so_in_both_places() {
        let frame = screen_frame(
            Screen::Right,
            Rect {
                x: 10,
                y: 20,
                width: 2,
                height: 2,
            },
            &[0x0001, 0x0002, 0x0003, 0x0004],
        );
        let frame = match frame {
            Ok(frame) => frame,
            Err(error) => panic!("a small rectangle must build: {error}"),
        };
        assert_eq!(frame.len(), 16 + 8 + 4);
        assert_eq!(frame[2], 1);
        assert_eq!(
            &frame[8..16],
            &[0x00, 0x0a, 0x00, 0x14, 0x00, 0x02, 0x00, 0x02]
        );
        // The footer is `0x40, 0x00, screen, 0x00`, so the screen number sits second from the end.
        assert_eq!(&frame[frame.len() - 4..], &[0x40, 0x00, 0x01, 0x00]);
    }

    #[test]
    fn a_frame_that_does_not_add_up_is_refused() {
        let too_few = screen_frame(
            Screen::Left,
            Rect {
                x: 0,
                y: 0,
                width: 4,
                height: 4,
            },
            &[0; 15],
        );
        assert_eq!(
            too_few,
            Err(FrameError::PixelCount {
                expected: 16,
                got: 15
            })
        );
        let outside = full_screen_frame(Screen::Left, &[0; 4]);
        assert_eq!(
            outside,
            Err(FrameError::PixelCount {
                expected: 76800,
                got: 4
            })
        );
        let past = screen_frame(
            Screen::Left,
            Rect {
                x: 300,
                y: 0,
                width: 40,
                height: 1,
            },
            &[0; 40],
        );
        assert_eq!(past, Err(FrameError::OutsideScreen));
    }

    #[test]
    fn colours_run_in_steps_of_four_and_the_last_is_white() {
        let codes: Vec<u8> = Colour::all().iter().map(|colour| colour.code()).collect();
        assert_eq!(
            codes,
            vec![
                0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 48, 52, 56, 60, 64, 68
            ]
        );
    }

    #[test]
    fn a_colour_becomes_the_bits_a_screen_takes() {
        assert_eq!(rgb565(0xff, 0xff, 0xff), 0xffff);
        assert_eq!(rgb565(0, 0, 0), 0x0000);
        // Pure red: five bits, at the top of the word.
        assert_eq!(rgb565(0xff, 0, 0), 0xf800);
        assert_eq!(rgb565(0, 0xff, 0), 0x07e0);
        assert_eq!(rgb565(0, 0, 0xff), 0x001f);
    }

    #[test]
    fn the_motor_is_refused_by_name() {
        let refused = motor_packet(0, 0);
        assert_eq!(refused, Err(MotorsNotPermitted));
        let text = MotorsNotPermitted.to_string();
        assert!(text.contains("ADR-0006"), "{text}");
    }

    #[test]
    fn the_tempo_fader_has_its_own_centre() {
        let centred = TempoFader::centred();
        assert!((centred.position(0) + 1.0).abs() < 1e-9);
        assert!(centred.position(2048).abs() < 1e-9);
        assert!((centred.position(4096) - 1.0).abs() < 1e-9);
        // A deck whose centre sits 100 ticks high reads +0.05 at the default middle.
        let shifted = TempoFader::new(2148.0);
        assert!((shifted.position(2048) + 100.0 / 2048.0).abs() < 1e-9);
    }
}
