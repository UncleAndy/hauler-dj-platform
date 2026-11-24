//! What the panel's own buttons say: a report from the device becomes control events.
//!
//! The bytes are data, not code (the same rule as the lights map): which bit of which report carries `deck_a.play` is a
//! fact about this device, learned by pressing the button, and it lives in `data/inputs-map.txt` where it can be read,
//! corrected and argued with. A program that hard-coded the layout would have to be rebuilt to fix a wrong byte.
//!
//! - Backlog task: A09 (the path), A10 (the routing)
//! - Requirements: DJP-CTRL-002

use std::collections::HashMap;

use dj_controller_core::{ControlEvent, report};

/// The map as it ships: learned on the panel, one line per control.
pub const INPUTS_MAP: &str = include_str!("../data/inputs-map.txt");

/// One control's place in a report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    /// The control's name, the words the binding table uses (`deck_a.play`).
    pub control: String,
    /// Which report carries it.
    pub report: u8,
    /// Which byte of the payload.
    pub byte: usize,
    /// Which bit of that byte, for a field narrower than the byte.
    pub bit: usize,
    /// How many bits wide: 1 for a button, 12 for an encoder, 16 for a fader.
    pub width: usize,
    /// Whether the field counts turns rather than states: a browse encoder says "one step right", not "row seven".
    pub relative: bool,
}

/// Every control the map knows, and where it lives.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InputMap {
    fields: Vec<Field>,
}

impl InputMap {
    /// The map that ships with the program.
    #[must_use]
    pub fn built_in() -> Self {
        parse_inputs(INPUTS_MAP).unwrap_or_default()
    }

    /// The controls, in file order.
    #[must_use]
    pub fn fields(&self) -> &[Field] {
        &self.fields
    }

    /// The field a control name stands for, when the map knows it.
    #[must_use]
    pub fn field_of(&self, control: &str) -> Option<&Field> {
        self.fields.iter().find(|field| field.control == control)
    }

    /// The raw value a report carries for one field: what the device says, before any scaling.
    ///
    /// A hand asks "how much of the travel is left?" and the answer is a number of counts: a knob that reaches 16383 of
    /// 65535 is a fourteen-bit knob, and the map is wrong, not the hand.
    #[must_use]
    pub fn raw_of(&self, control: &str, report_id: u8, payload: &[u8]) -> Option<u16> {
        let field = self.field_of(control)?;
        if field.report != report_id {
            return None;
        }
        report::unsigned(payload, field.byte, field.bit, field.width)
    }

    /// True while nothing has been learned yet: a panel whose presses ask for nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// The events one report carries, for the fields whose value changed.
    ///
    /// `previous` is the caller's memory of the last value of each control: a device that reports the whole panel every
    /// few milliseconds says the same thing most of the time, and a command for a button nobody touched is a command
    /// that would start a deck by itself.
    pub fn decode(
        &self,
        report_id: u8,
        payload: &[u8],
        previous: &mut HashMap<String, f32>,
    ) -> Vec<ControlEvent> {
        let mut events = Vec::new();
        for field in self.fields.iter().filter(|field| field.report == report_id) {
            let Some(raw) = report::unsigned(payload, field.byte, field.bit, field.width) else {
                continue;
            };
            let value = value_of(raw, field.width);
            if field.relative {
                // A turn is a direction, not a position: the encoder's byte counts steps and runs round, so comparing
                // two values would report a jump from the top to the bottom as a huge move in one direction. The rule is
                // the one Mixxx's own encoder uses: a step up is "right", a step down is "left", and a wrapped step is
                // the direction the wrap makes it. The first report of a session only teaches the resting value.
                let known = previous.get(&field.control).copied();
                previous.insert(field.control.clone(), value);
                let Some(known) = known else {
                    continue;
                };
                let right = if known == 1.0 && value == 0.0 {
                    true
                } else if known == 0.0 && value == 1.0 {
                    false
                } else if (known - value).abs() <= f32::EPSILON {
                    continue;
                } else {
                    value > known
                };
                events.push(ControlEvent::level(
                    field.control.clone(),
                    if right { 1.0 } else { 0.0 },
                ));
                continue;
            }
            let known = previous.get(&field.control).copied();
            // A button is exact; a knob is not. Sixteen bits of travel jitter by a step or two when nothing is touched,
            // and every one of those steps would be a command to the engine: what looks like a jump is the program
            // obeying noise. Two steps of the field's own resolution is the deadband.
            let deadband = if field.width == 1 {
                0.0
            } else {
                2.0 / f32::from(
                    u16::try_from((1_u32 << field.width.min(16)) - 1).unwrap_or(u16::MAX),
                )
            };
            if known.is_some_and(|known| (known - value).abs() <= deadband) {
                continue;
            }
            previous.insert(field.control.clone(), value);
            // A button already held when the program starts is not a press: the first report only teaches the resting
            // state.
            if known.is_none() && value == 0.0 && field.width == 1 {
                continue;
            }
            events.push(ControlEvent::level(field.control.clone(), value));
        }
        events
    }
}

/// A field's value on the scale the rest of the program uses: `0.0…1.0`, and exactly `0.0`/`1.0` for a button.
#[must_use]
fn value_of(raw: u16, width: usize) -> f32 {
    if width == 1 {
        return f32::from(raw.min(1));
    }
    let top = (1_u32 << width.min(16)) - 1;
    f32::from(raw) / top as f32
}

/// The shipped map with a learned one laid over it.
///
/// A line from the shipped file wins: it was confirmed by pressing the button. A learned line can add a control the
/// shipped file does not have, and a learned line that contradicts the shipped one is reported and dropped — a file left
/// over from an earlier attempt must not put a command on the wrong button.
/// The report that is never a button: the clock and the platters, with counters that move by themselves.
pub const CLOCK_REPORT: u8 = 3;

#[must_use]
pub fn merge(shipped: &InputMap, learned: &InputMap) -> (InputMap, Vec<String>) {
    let mut merged = shipped.clone();
    let mut notes = Vec::new();
    for field in &learned.fields {
        // A line from a report that only carries counters fires by itself several times a second: learned that way, it
        // started decks with nobody near the panel (owner's report, 24.11: "the deck starts by itself").
        if field.report == CLOCK_REPORT {
            notes.push(format!(
                "{}: report {CLOCK_REPORT} — это часы и джоги, кнопок там нет; строку не беру",
                field.control
            ));
            continue;
        }
        match merged
            .fields
            .iter()
            .find(|known| known.control == field.control)
        {
            Some(known) if known == field => {}
            Some(known) => notes.push(format!(
                "{}: выучено report {} byte {} bit {}, а в поставленной карте report {} byte {} bit {} — оставляю \
                 поставленную",
                field.control, field.report, field.byte, field.bit, known.report, known.byte, known.bit
            )),
            None => {
                if merged
                    .fields
                    .iter()
                    .any(|known| known.report == field.report && known.byte == field.byte && known.bit == field.bit)
                {
                    notes.push(format!(
                        "{}: report {} byte {} bit {} уже занят другим контролем — строку не беру",
                        field.control, field.report, field.byte, field.bit
                    ));
                    continue;
                }
                merged.fields.push(field.clone());
            }
        }
    }
    (merged, notes)
}

/// Reads the map file.
///
/// Damage is an error with a line number, never a silent skip: a map with one wrong line puts a command on the wrong
/// button, and the DJ finds out on stage.
pub fn parse_inputs(text: &str) -> Result<InputMap, String> {
    let mut fields: Vec<Field> = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() != 5 && !(parts.len() == 6 && parts[5] == "relative") {
            return Err(format!(
                "line {}: wanted control, report, byte, bit, width, and an optional `relative`; got {line}",
                number + 1
            ));
        }
        let control = parts[0].to_string();
        let parse = |what: &str, text: &str| -> Result<usize, String> {
            text.parse::<usize>()
                .map_err(|_| format!("line {}: {what} is not a number: {text}", number + 1))
        };
        let report_text = parts[1];
        let report = u8::try_from(parse("the report", report_text)?)
            .map_err(|_| format!("line {}: {report_text} is not a report number", number + 1))?;
        let byte = parse("the byte", parts[2])?;
        let bit = parse("the bit", parts[3])?;
        let width = parse("the width", parts[4])?;
        if width == 0 || width > 16 || bit > 7 || (width > 1 && bit + width > 16) {
            return Err(format!(
                "line {}: {width} bits at bit {bit} do not fit the payload",
                number + 1
            ));
        }
        if fields.iter().any(|field| field.control == control) {
            return Err(format!(
                "line {}: {control} is already in the map",
                number + 1
            ));
        }
        fields.push(Field {
            control,
            report,
            byte,
            bit,
            width,
            relative: parts.len() == 6,
        });
    }
    Ok(InputMap { fields })
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# control report byte bit width
deck_a.play 1 4 0 1
deck_b.play 1 4 1 1
deck_a.crossfader 2 12 0 16
";

    #[test]
    fn a_button_becomes_a_press_and_a_release() {
        let map = parse_inputs(SAMPLE).expect("the sample parses");
        let mut previous = HashMap::new();
        let mut payload = vec![0_u8; 20];
        // Nothing pressed yet: the first report teaches the map the resting state and asks for nothing.
        assert!(map.decode(1, &payload, &mut previous).is_empty());
        payload[4] = 0b0000_0001;
        let events = map.decode(1, &payload, &mut previous);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].control, "deck_a.play");
        assert_eq!(events[0].value, 1.0);
        // The same report again is not a second press.
        assert!(map.decode(1, &payload, &mut previous).is_empty());
        payload[4] = 0;
        let events = map.decode(1, &payload, &mut previous);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].value, 0.0);
    }

    #[test]
    fn a_fader_arrives_as_a_fraction() {
        let map = parse_inputs(SAMPLE).expect("the sample parses");
        let mut previous = HashMap::new();
        let mut payload = vec![0_u8; 20];
        payload[12] = 0xFF;
        payload[13] = 0x7F;
        let events = map.decode(2, &payload, &mut previous);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].control, "deck_a.crossfader");
        assert!(
            (events[0].value - 0.5).abs() < 0.001,
            "{:?}",
            events[0].value
        );
    }

    #[test]
    fn a_knob_ignores_the_jitter_of_its_lowest_bits() {
        // Sixteen bits of travel jitter by a step or two on their own; a command per step would look like a fader that
        // jumps while nobody touches it.
        let map = parse_inputs("deck_a.tempo 2 12 0 16\n").expect("the map parses");
        let mut previous = HashMap::new();
        let mut payload = vec![0_u8; 20];
        payload[12] = 0x00;
        payload[13] = 0x80;
        assert_eq!(
            map.decode(2, &payload, &mut previous).len(),
            1,
            "the first value"
        );
        payload[12] = 0x01;
        assert!(
            map.decode(2, &payload, &mut previous).is_empty(),
            "one step is noise"
        );
        payload[12] = 0x02;
        assert!(
            map.decode(2, &payload, &mut previous).is_empty(),
            "two steps are noise"
        );
        payload[12] = 0x40;
        assert_eq!(
            map.decode(2, &payload, &mut previous).len(),
            1,
            "a real move"
        );
    }

    #[test]
    fn a_report_without_fields_of_its_own_says_nothing() {
        let map = parse_inputs(SAMPLE).expect("the sample parses");
        let mut previous = HashMap::new();
        let payload = vec![0xFF_u8; 20];
        assert!(map.decode(3, &payload, &mut previous).is_empty());
    }

    #[test]
    fn a_broken_line_is_an_error_with_its_number() {
        let error = parse_inputs("deck_a.play 1 4 0\n").expect_err("four fields are not five");
        assert!(error.contains("line 1"), "{error}");
        let error = parse_inputs("deck_a.play 1 4 0 x\n").expect_err("the width is not a number");
        assert!(error.contains("the width"), "{error}");
        let error = parse_inputs("deck_a.play 1 4 0 1\ndeck_a.play 1 5 0 1\n")
            .expect_err("one control twice");
        assert!(error.contains("already in the map"), "{error}");
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod browser_tests {
    use super::*;

    #[test]
    fn a_turn_says_which_way_it_went() {
        // A relative field counts steps and runs round: the first report teaches the resting value, a step up is "right",
        // a step down is "left", and a wrap is a step in the direction the wrap makes it.
        let map = parse_inputs("panel.encoder 1 20 0 4 relative").expect("the map");
        let mut previous = HashMap::new();
        let mut payload = [0_u8; 32];
        assert!(
            map.decode(1, &payload, &mut previous).is_empty(),
            "the resting value is not a turn"
        );
        let value = |events: &[ControlEvent]| events.first().map(|event| event.value);
        payload[20] = 3;
        assert_eq!(
            value(&map.decode(1, &payload, &mut previous)),
            Some(1.0),
            "right"
        );
        payload[20] = 1;
        assert_eq!(
            value(&map.decode(1, &payload, &mut previous)),
            Some(0.0),
            "left"
        );
        payload[20] = 15;
        assert_eq!(
            value(&map.decode(1, &payload, &mut previous)),
            Some(1.0),
            "right, up to the top"
        );
        payload[20] = 0;
        assert_eq!(
            value(&map.decode(1, &payload, &mut previous)),
            Some(1.0),
            "round the top is still right, not a jump to the bottom of the range"
        );
        payload[20] = 15;
        assert_eq!(
            value(&map.decode(1, &payload, &mut previous)),
            Some(0.0),
            "and round the bottom is left"
        );
    }

    #[test]
    fn the_map_that_ships_has_the_browser_controls() {
        let map = InputMap::built_in();
        let encoder = map.field_of("panel.encoder").expect("the browse encoder");
        assert!(
            encoder.relative,
            "an encoder counts steps, it does not hold a position"
        );
        assert!(map.field_of("panel.view").is_some(), "the VIEW button");
        assert!(
            map.field_of("panel.encoder_press").is_some(),
            "and the press of the encoder"
        );
    }
}
