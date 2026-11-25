//! The lights: what can be said to the controller, and how to find out where.
//!
//! The colour vocabulary came with the protocol notes: an LED report of 94 bytes, colours numbered in steps of four, and
//! eighteen named colours up to 68. What did **not** come with them is the map from a byte to a control — that table was
//! deliberately not copied, and inventing it would produce a controller that lights "on average right", which is another
//! way of saying wrong.
//!
//! So the map is learned on the device, by the only honest method: send one byte at a time and watch which control
//! lights up. This module builds those reports. It is safe by construction — a light cannot move a hand or a platter —
//! and it touches nothing but the lights report: the motor reports are behind a decision that has not been made.

use crate::Colour;

/// The output report that carries the lights.
pub const REPORT_LIGHTS: u8 = 128;
/// The output report that carries the level meters: four channel columns of VU segments.
pub const REPORT_METERS: u8 = 129;
/// Its length: four columns of 15 bytes (thirteen segments, one partial, one clip), from the device's public script.
pub const METERS_LENGTH: usize = 78;
/// Its length, as the protocol notes have it. Whether this count includes the report identifier is exactly the kind of
/// question the device answers and a document does not, so both readings can be sent (`--body-only`).
pub const LIGHTS_LENGTH: usize = 94;

/// Brightness is not a separate field: it is an offset added to the colour code.
///
/// The colour table steps by four — red is 4, green 28, white 68 — and the two low bits carry the level. Source: the
/// public controller script for this device, where a button sends `this.color + this.brightnessOn`, with
/// `brightnessOff = 0` and `brightnessOn = 2`. So a base colour on its own is the dim state, and the same colour plus
/// two is the bright one. A code with `+1` is a slightly different shade, which that script uses to squeeze extra
/// colours out of sixteen base hues; it is not a brightness level.
pub const BRIGHTNESS_OFF: u8 = 0;
/// The offset that makes a control bright.
pub const BRIGHTNESS_ON: u8 = 2;

/// The same colour, dim: what an inactive control shows in a dark booth.
pub fn dim(colour: u8) -> u8 {
    colour.saturating_add(BRIGHTNESS_OFF)
}

/// The same colour, bright: what the DJ is looking for.
pub fn bright(colour: u8) -> u8 {
    colour.saturating_add(BRIGHTNESS_ON)
}

/// One lights report: the identifier and the body, kept apart so a caller can decide what goes on the wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LightsReport {
    /// Report identifier, as the protocol notes name it.
    pub id: u8,
    /// The body: one byte per light, or per group of lights.
    pub body: Vec<u8>,
}

impl LightsReport {
    /// A report of `LIGHTS_LENGTH` bytes, all dark.
    pub fn dark() -> Self {
        Self {
            id: REPORT_LIGHTS,
            body: vec![0u8; LIGHTS_LENGTH],
        }
    }

    /// One byte on, the rest dark: the report that asks "which control is this one?".
    pub fn one(byte: usize, value: u8) -> Option<Self> {
        if byte >= LIGHTS_LENGTH {
            return None;
        }
        let mut report = Self::dark();
        report.body[byte] = value;
        Some(report)
    }

    /// The bytes for the wire, identifier first, or the body alone when `with_id` is false.
    pub fn wire(&self, with_id: bool) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.body.len() + 1);
        if with_id {
            out.push(self.id);
        }
        out.extend_from_slice(&self.body);
        out
    }
}

/// The reports for a sweep from `from` up to and including `to`.
///
/// A sweep exists to be watched: one byte at a time, one control at a time. Two bytes at once would light two controls
/// and leave the observer guessing which of them answered.
pub fn sweep(from: usize, to: usize, value: u8) -> Vec<LightsReport> {
    let last = to.min(LIGHTS_LENGTH.saturating_sub(1));
    if from > last {
        return Vec::new();
    }
    (from..=last)
        .filter_map(|byte| LightsReport::one(byte, value))
        .collect()
}

/// One line of the map as it is written down: the byte, a tab, what it lights.
///
/// A tab rather than a comma because a name will contain one sooner or later ("CUE, deck A"), and a map whose columns
/// shift is a map that lies. Empty names are not written: a byte that lights nothing is an absence in the file, and an
/// absence is honest.
pub fn map_line(byte: usize, name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    Some(format!("{byte}\t{name}\n"))
}

/// The map as records: byte and name, in the order the file gives them.
pub fn parse_map(text: &str) -> Vec<(usize, String)> {
    text.lines()
        .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
        .filter_map(|line| {
            let (byte, name) = line.split_once('\t')?;
            let byte = byte.trim().parse().ok()?;
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            Some((byte, name.to_string()))
        })
        .collect()
}

/// The colour codes a probe walks through, from `4,8,12` or with spaces.
///
/// The point of more than one code on the same byte is the question the first sweep could not answer: does this byte
/// set a colour, a brightness, or merely "on"? One byte in three colours answers it for that byte, and a person
/// watching says which of the three it was.
pub fn colours(list: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for part in list
        .split([',', ' '])
        .filter(|part| !part.trim().is_empty())
    {
        let code: u8 = part
            .trim()
            .parse()
            .map_err(|_| format!("a colour is one byte, and {part:?} is not"))?;
        out.push(code);
    }
    if out.is_empty() {
        return Err("no colours given".to_string());
    }
    Ok(out)
}

/// The map, as the controller showed it: one byte of the lights report to what lit up.
///
/// The bytes are in `data/lights-map.txt`, taken down by the owner with `--lights-map`, one byte at a time, and that
/// file — not this code — is the record. It is data for the same reason the binding table is: a map in code cannot be
/// argued with, corrected after a second look, or replaced by a different controller.
pub const MAP_SOURCE: &str = include_str!("../data/lights-map.txt");

/// The map, parsed once.
pub fn map() -> &'static [(u8, &'static str)] {
    static PARSED: std::sync::OnceLock<Vec<(u8, &'static str)>> = std::sync::OnceLock::new();
    PARSED.get_or_init(|| {
        let mut out: Vec<(u8, &'static str)> = Vec::new();
        for line in MAP_SOURCE.lines() {
            let line = line.trim_end();
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            let Some((byte, name)) = line.split_once('\t') else {
                continue;
            };
            let Ok(byte) = byte.trim().parse::<u8>() else {
                continue;
            };
            let name = name.trim();
            if !name.is_empty() {
                out.push((byte, name));
            }
        }
        out
    })
}

/// What a byte of the lights report lights up, as the owner saw it.
pub fn name_of(byte: u8) -> Option<&'static str> {
    map()
        .iter()
        .find(|(candidate, _)| *candidate == byte)
        .map(|(_, name)| *name)
}

/// The byte that lights a control whose name contains `needle`, ignoring case.
///
/// A name in the map is a person's words ("Left CUE3", "Deck select A"), so a lookup by part of those words is what a
/// caller can do without owning a table of byte numbers.
pub fn byte_of(needle: &str) -> Option<u8> {
    let needle = needle.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return None;
    }
    map()
        .iter()
        .find(|(_, name)| name.to_ascii_lowercase().contains(&needle))
        .map(|(byte, _)| *byte)
}

/// The bytes the map has heard about, in order, for a caller that wants to light everything the owner named.
pub fn known_bytes() -> Vec<u8> {
    map().iter().map(|(byte, _)| *byte).collect()
}

/// What the panel shows right now.
///
/// What one hotcue pad shows: a colour, and whether something is stored in that slot.
///
/// The owner asked for the difference (24.11): both kinds of pad are visible in the dark, but only a stored cue is at
/// full brightness — an empty pad that shines as brightly as a full one is a pad that promises a cue it does not have.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pad {
    /// The colour of the pad: the marker's own colour when the slot holds a cue, the palette's otherwise.
    pub colour: Colour,
    /// True when the slot holds a cue.
    pub occupied: bool,
}

/// Two halves, because the controller has two: the decks with their platters and hotcues are the left and the right
/// pair, while the mixer section has an FX button for each of the four decks the program runs. The struct says exactly
/// that and nothing more — no field for a light the map has not taught us.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Show {
    /// Hotcue slots, for the left and the right deck, each in the colour of its marker in the application.
    ///
    /// A pad's colour is the colour the DJ painted the cue with: a hand looks for the pad it painted, not for a pad the
    /// program chose. `None` means the deck is empty and the pads say nothing at all.
    pub hotcues: [[Option<Pad>; 8]; 2],
    /// Playing, for the left and the right deck.
    pub playing: [bool; 2],
    /// Monitoring in the headphones (the CUE button), one per channel: the panel has a headphone button on each of the
    /// four strips, and it lights its own, not the pad next to it (owner's report, 24.11).
    pub monitoring: [bool; 4],
    /// Sync engaged, for the left and the right deck.
    pub sync: [bool; 2],
    /// The deck is the master the others follow, for the left and the right deck. A lamp of its own, not the sync lamp
    /// again: with both lit the DJ has to be able to tell the leader from the follower at a glance.
    pub master: [bool; 2],
    /// FX on, for decks A, B, C and D: the mixer section has a button for each.
    pub fx: [bool; 4],
    /// The effect each deck runs, by `FxKind::index()`: its "FX on" button wears the effect's own colour, not a single
    /// one for every effect.
    pub fx_kind: [u8; 4],
}

/// The byte that lights a hotcue slot, found by the name the owner wrote down.
/// Every hotcue pad of the panel, both sides, in map order.
///
/// Read from the map rather than written down twice: the pads are the bytes named `Left CUE1…CUE8` and
/// `Right CUE1…CUE8`, and a map that renames one of them moves the rule with it.
pub fn hotcue_bytes() -> Vec<u8> {
    map()
        .iter()
        .filter(|(_, name)| {
            let trimmed = name
                .trim_start_matches("Left ")
                .trim_start_matches("Right ");
            trimmed
                .strip_prefix("CUE")
                .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
        })
        .map(|(byte, _)| *byte)
        .collect()
}

fn hotcue_byte(side: usize, slot: usize) -> Option<u8> {
    let side = if side == 0 { "Left" } else { "Right" };
    byte_of(&format!("{side} CUE{}", slot + 1))
}

/// The byte for a named control, or nothing when the map has never heard of it.
fn control(name: &str) -> Option<u8> {
    byte_of(name)
}

/// Which colour each state of the panel uses.
///
/// The colours are settings, not constants: the panel is in the owner's hands, and whether white is too much in a dark
/// booth is not a question the hardware answers. This struct is the whole vocabulary — one field per state — so adding a
/// state means adding a field and a line where it is painted, not hunting through matches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    /// The colour of everything inactive, when the panel rests instead of going dark.
    pub resting: Colour,
    /// The marker on a playing deck's platter.
    pub jog: Colour,
    /// A deck that is playing.
    pub play: Colour,
    /// A channel in the headphones.
    pub cue: Colour,
    /// A deck that follows another.
    pub sync: Colour,
    /// The deck the others follow: the MASTER lamp.
    pub master: Colour,
    /// A stored hotcue slot.
    pub hotcue: Colour,
    /// A pad that holds a loop: the colour the screen draws a loop with.
    pub loop_: Colour,
    /// A deck whose effect is switched in.
    pub fx: Colour,
    /// The four FX select keys in order: what each loads (`FxKind::index()`) and the colour that key wears. Key 1 is
    /// first, key 4 last; the dedicated Filter key beside them is not part of this table.
    pub fx_keys: [(u8, Colour); 4],
}

impl Default for Palette {
    fn default() -> Self {
        // The colours the first build drew, so a panel without a settings section looks the way it did.
        Self {
            resting: Colour::White,
            jog: Colour::White,
            play: Colour::Green,
            cue: Colour::Blue,
            sync: Colour::Yellow,
            master: Colour::White,
            hotcue: Colour::Green,
            loop_: Colour::Yellow,
            fx: Colour::Magenta,
            fx_keys: [
                (1, Colour::Blue),
                (2, Colour::Green),
                (3, Colour::Purple),
                (4, Colour::Aqua),
            ],
        }
    }
}

/// The report for a state: every control that should be lit, in its colour, and everything else dark.
///
/// Colours are a choice, not a measurement: a lit hotcue is green, a playing deck green, monitoring blue, sync yellow,
/// FX magenta. The byte sets the colour — that is what the probe on byte 8 showed — so what is left is deciding which
/// colour means what, and that is written here in one place a person can argue with.
pub fn report_for(show: &Show) -> LightsReport {
    report_with(show, &Palette::default())
}

/// The same report in the colours the settings name.
pub fn report_with(show: &Show, palette: &Palette) -> LightsReport {
    let mut report = LightsReport::dark();
    let mut set = |byte: Option<u8>, colour: u8| {
        if let Some(byte) = byte
            && let Some(slot) = report.body.get_mut(usize::from(byte))
        {
            *slot = colour;
        }
    };

    for (side, slots) in show.hotcues.iter().enumerate() {
        for (slot, pad) in slots.iter().enumerate() {
            // A stored cue is at full brightness; an empty slot glows faintly, so the pad can be found in the dark without
            // claiming a cue nobody stored.
            if let Some(pad) = pad {
                let code = pad.colour.code();
                let level = if pad.occupied {
                    bright(code)
                } else {
                    dim(code)
                };
                set(hotcue_byte(side, slot), level);
            }
        }
    }
    for side in 0..2 {
        // Both names carry their side: a lookup is a substring, and "Master/Reset" alone is in the map twice — the left
        // lamp answered for the right one, so the leader's lamp appeared on the wrong deck and the right one stayed dark.
        // The right pair was named "Rick SYNC/LOCK" in the map (a typo of the owner's), which no lookup could find.
        let names = if side == 0 {
            ("Left PLAY button", "Left Sync/Lock", "Left Master/Reset")
        } else {
            ("Right PLAY button", "Right SYNC/LOCK", "Right MASTER/RESET")
        };
        if show.playing[side] {
            set(control(names.0), bright(palette.play.code()));
        }
        if show.sync[side] {
            set(control(names.1), bright(palette.sync.code()));
        }
        if show.master[side] {
            set(control(names.2), bright(palette.master.code()));
        }
    }
    // Monitoring is per channel, not per platter: the button lives on the strip and carries the channel's own CUE byte.
    // `byte_of("Left CUE")` would find the pad `Left CUE1` first, which is exactly the wrong light (owner's report, 24.11).
    for (deck, lit) in show.monitoring.iter().enumerate() {
        if *lit {
            let name = match deck {
                0 => "Deck A CUE (headphones)",
                1 => "Deck B CUE",
                2 => "Deck C CUE",
                _ => "Deck D CUE",
            };
            set(control(name), bright(palette.cue.code()));
        }
    }
    // A deck's "FX on" button shows the colour of the effect that deck is running, so each effect reads as its own
    // lamp rather than one lamp copied four times (owner's request, 25.11). The select keys themselves are labels: they
    // glow in their effect's colour from `report_at_rest`, not from the active layer.
    for (deck, lit) in show.fx.iter().enumerate() {
        if *lit {
            let name = match deck {
                0 => "Deck A FX on",
                1 => "Deck B FX on button",
                2 => "Deck C FX on button",
                _ => "Deck D FX on button",
            };
            set(
                control(name),
                bright(effect_colour(show.fx_kind[deck], &palette.fx_keys).code()),
            );
        }
    }
    report
}

/// The report for a dark booth: every control the map knows glows faintly, and the active ones stand out.
///
/// This is what the owner asked for — "минимально светились все кнопки, а включённые подсвечивались ярче" — and it is
/// only possible because brightness is an offset: the same colour at 0 is the faint state and at 2 is the bright one.
/// `resting` is the colour everything sits at when nothing is happening; the active controls keep their own colours,
/// brighter.
pub fn report_at_rest(show: &Show, palette: &Palette) -> LightsReport {
    let mut report = LightsReport::dark();
    let faint = dim(palette.resting.code());
    for (byte, _) in map() {
        if let Some(slot) = report.body.get_mut(usize::from(*byte)) {
            *slot = faint;
        }
    }
    // The hotcue pads are not part of "the instrument is on": they say what is in the deck, and a deck with no record
    // has nothing to say. The owner's rule: the pads light when a track is loaded, not when the program starts.
    for byte in hotcue_bytes() {
        if let Some(slot) = report.body.get_mut(usize::from(byte)) {
            *slot = 0;
        }
    }
    // The four numbered FX select keys glow in the colour the settings gave each key, so in the dark a hand tells 1
    // from 2 from 3 from 4 at a glance (owner's request, 25.11). They sit at the faint step of the resting layer.
    for (index, byte) in [50_u8, 51, 52, 53].into_iter().enumerate() {
        if let Some(slot) = report.body.get_mut(usize::from(byte)) {
            *slot = dim(palette.fx_keys[index].1.code());
        }
    }
    // The Filter key beside the four numbered ones is its own: it always loads the filter and keeps the filter's colour.
    if let Some(slot) = report.body.get_mut(54) {
        *slot = dim(fx_colour(0).code());
    }
    // The active controls are painted over the resting layer: their own colour, one brightness step up.
    let active = report_with(show, palette);
    for (byte, colour) in report.body.iter_mut().zip(active.body.iter()) {
        if *colour != 0 {
            *byte = *colour;
        }
    }
    report
}

/// The colour a settings file names, by the name the device's own table uses.
pub fn colour_by_name(name: &str) -> Option<Colour> {
    let wanted = name.trim().to_ascii_lowercase();
    Colour::all()
        .into_iter()
        .find(|colour| colour_name(*colour) == wanted)
}

/// The reports that check the encoding on the device: the same colour, dim and bright, a few times each.
///
/// If the panel shows one colour changing brightness rather than two colours, the encoding in `bright`/`dim` is right;
/// if it shows two colours, brightness lives somewhere else and this check says so before any code depends on it.
pub fn brightness_probe(byte: u8, colour: u8, rounds: usize) -> Vec<LightsReport> {
    let mut out = Vec::new();
    for _ in 0..rounds {
        if let Some(report) = LightsReport::one(usize::from(byte), dim(colour)) {
            out.push(report);
        }
        if let Some(report) = LightsReport::one(usize::from(byte), bright(colour)) {
            out.push(report);
        }
    }
    out
}

/// Every control the map knows, in order, for a panel test that lights them one at a time.
pub fn walk() -> Vec<LightsReport> {
    let mut out = Vec::new();
    for (byte, _) in map() {
        if let Some(report) = LightsReport::one(usize::from(*byte), bright(Colour::Green.code())) {
            out.push(report);
        }
    }
    out
}

/// One step of a colour walk: the code to send, the colour it should be, and the state it should be in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColourStep {
    /// The byte that goes into the report.
    pub code: u8,
    /// The colour's name, as the colour table calls it.
    pub colour: &'static str,
    /// What the step should look like: faint, faint shifted, bright, bright shifted.
    pub state: &'static str,
}

/// Every base colour in both states, so a person can check the whole table rather than one green.
///
/// The encoding is an offset: base is faint, base + 2 is bright, and + 1 is a slightly different shade. This walk puts
/// those four variants of every colour on one control in order, which is what "разобраться и с другими цветами" needs —
/// a green that behaves says nothing about blue.
pub fn colour_walk(shifted: bool) -> Vec<ColourStep> {
    let mut out = Vec::new();
    for colour in Colour::all() {
        let base = colour.code();
        let name = colour_name(colour);
        out.push(ColourStep {
            code: dim(base),
            colour: name,
            state: "faint",
        });
        out.push(ColourStep {
            code: bright(base),
            colour: name,
            state: "bright",
        });
        if shifted {
            out.push(ColourStep {
                code: dim(base + 1),
                colour: name,
                state: "faint shifted",
            });
            out.push(ColourStep {
                code: bright(base + 1),
                colour: name,
                state: "bright shifted",
            });
        }
    }
    out
}

/// What each colour of the panel looks like, as the reference table gives it: `#rrggbb` at three quarters.
///
/// The numbers are not invented here: they come with the public controller script's colour table, one entry per colour
/// name, and they are what the application's cue colours are matched against. A cue marked `#ff0044` has no exact light
/// on this panel; the nearest one is what a hand can find in the dark.
pub fn colour_rgb(colour: Colour) -> u32 {
    match colour {
        Colour::Off => 0x000000,
        Colour::Red => 0xCC0000,
        Colour::Carrot => 0xCC5E00,
        Colour::Orange => 0xCC7800,
        Colour::Honey => 0xCC9200,
        Colour::Yellow => 0xCCCC00,
        Colour::Lime => 0x81CC00,
        Colour::Green => 0x00CC00,
        Colour::Aqua => 0x00CC49,
        Colour::Celeste => 0x00CCCC,
        Colour::Sky => 0x0091CC,
        Colour::Blue => 0x0000CC,
        Colour::Purple => 0xCC00CC,
        Colour::Fuscia => 0xAD65FF,
        Colour::Magenta => 0xCC0079,
        Colour::Azalea => 0xCC477E,
        Colour::Salmon => 0xCC4761,
        Colour::White => 0xCCCCCC,
    }
}

/// The meters report: four channel columns, one per deck, of thirteen segments plus a partial and a clip light.
///
/// The layout is the device's own (read from its public controller script, output report 129): a column is fifteen
/// bytes — thirteen full segments, one partial, one clip — a lit segment is 127 and a partial one 125. The level is the
/// channel's held peak, which is what a meter shows; the clip light is the instantaneous peak at the top of the scale.
#[must_use]
pub fn meter_report(levels: &[(f32, bool); 4]) -> LightsReport {
    let mut body = vec![0u8; METERS_LENGTH];
    let segments = 13;
    for (deck, (level, clipped)) in levels.iter().enumerate() {
        let base = deck * (segments + 2);
        let scaled = level.clamp(0.0, 1.0) * segments as f32;
        let full = (scaled.floor() as usize).min(segments);
        let partial = scaled - full as f32;
        if full > 0 {
            for i in 0..=full {
                if let Some(slot) = body.get_mut(base + i) {
                    *slot = 127;
                }
            }
            if partial > 0.5
                && full < segments
                && let Some(slot) = body.get_mut(base + full + 1)
            {
                *slot = 125;
            }
        }
        if *clipped && let Some(slot) = body.get_mut(base + segments + 1) {
            *slot = 127;
        }
    }
    LightsReport {
        id: REPORT_METERS,
        body,
    }
}

/// The colour of the panel closest to an `#rrggbb` value.
///
/// Distance is the plain sum of squared channel differences, which is what a person's eye does well enough at this
/// saturation: the panel has eighteen colours, and "the closest of eighteen" is the honest answer to a cue marker
/// painted `#33aaff`. `None` for a string that is not a colour at all — a cue without a colour is not a colour to guess
/// at.
pub fn nearest_colour(rgb: &str) -> Option<Colour> {
    let text = rgb.trim().trim_start_matches('#');
    if text.len() != 6 {
        return None;
    }
    let value = u32::from_str_radix(text, 16).ok()?;
    let (red, green, blue) = ((value >> 16) & 0xFF, (value >> 8) & 0xFF, value & 0xFF);
    let mut best: Option<(u64, Colour)> = None;
    for colour in Colour::all() {
        if colour == Colour::Off {
            continue;
        }
        let candidate = colour_rgb(colour);
        let (cr, cg, cb) = (
            (candidate >> 16) & 0xFF,
            (candidate >> 8) & 0xFF,
            candidate & 0xFF,
        );
        let distance = (i64::from(red) - i64::from(cr)).pow(2)
            + (i64::from(green) - i64::from(cg)).pow(2)
            + (i64::from(blue) - i64::from(cb)).pow(2);
        let distance = distance as u64;
        if best.is_none_or(|(current, _)| distance < current) {
            best = Some((distance, colour));
        }
    }
    best.map(|(_, colour)| colour)
}

/// The colour each effect kind wears on the panel, by `FxKind::index()` (0…6).
///
/// Seven kinds, one colour each, so the select keys and the decks' "FX on" buttons read as different effects rather than
/// as one lamp copied four times.
#[must_use]
pub fn fx_colour(index: u8) -> Colour {
    match index {
        0 => Colour::Yellow, // Filter
        1 => Colour::Blue,   // Echo
        2 => Colour::Green,  // Roll
        3 => Colour::Purple, // Flanger
        4 => Colour::Aqua,   // Phaser
        5 => Colour::Red,    // Bitcrusher
        _ => Colour::Salmon, // Noise
    }
}

/// The colour an effect wears, driven by the select keys.
///
/// The earliest key (1..4) that loads the effect owns its colour; an effect not on any key keeps the built-in colour in
/// [`fx_colour`] (Filter, Bitcrusher and Noise are off the keys by default, but the window can still put them on a deck).
#[must_use]
pub fn effect_colour(kind: u8, fx_keys: &[(u8, Colour); 4]) -> Colour {
    for (key_kind, colour) in fx_keys {
        if *key_kind == kind {
            return *colour;
        }
    }
    fx_colour(kind)
}

/// A colour's name for a person watching the panel.
pub fn colour_name(colour: Colour) -> &'static str {
    match colour {
        Colour::Off => "off",
        Colour::Red => "red",
        Colour::Carrot => "carrot",
        Colour::Orange => "orange",
        Colour::Honey => "honey",
        Colour::Yellow => "yellow",
        Colour::Lime => "lime",
        Colour::Green => "green",
        Colour::Aqua => "aqua",
        Colour::Celeste => "celeste",
        Colour::Sky => "sky",
        Colour::Blue => "blue",
        Colour::Purple => "purple",
        Colour::Fuscia => "fuscia",
        Colour::Magenta => "magenta",
        Colour::Azalea => "azalea",
        Colour::Salmon => "salmon",
        Colour::White => "white",
    }
}

/// A report with `code` in every byte of `bytes`, and nothing anywhere else.
///
/// "Is this button colour capable?" is a question about a whole deck, not about one byte: a single button that stays
/// the same may be a button that is single-colour, or one byte that is not what the map says. Painting a group answers
/// it without guessing.
pub fn fill_only(code: u8, bytes: &[u8]) -> LightsReport {
    let mut report = LightsReport::dark();
    for byte in bytes {
        if let Some(slot) = report.body.get_mut(usize::from(*byte)) {
            *slot = code;
        }
    }
    report
}

/// The bytes a person means by a word: a side of the controller, everything, or one named byte number.
///
/// The map is the dictionary here: a byte belongs to a side because the owner wrote "Left CUE1" or "Right CUE1" next to
/// it, so the answer cannot drift away from what was observed.
pub fn bytes_for(what: &str) -> Result<Vec<u8>, String> {
    let what = what.trim().to_ascii_lowercase();
    let bytes: Vec<u8> = match what.as_str() {
        "all" | "" => map().iter().map(|(byte, _)| *byte).collect(),
        "left" => map()
            .iter()
            .filter(|(_, name)| name.to_ascii_lowercase().contains("left"))
            .map(|(byte, _)| *byte)
            .collect(),
        "right" => map()
            .iter()
            .filter(|(_, name)| name.to_ascii_lowercase().contains("right"))
            .map(|(byte, _)| *byte)
            .collect(),
        other => {
            let byte: u8 = other.parse().map_err(|_| {
                format!("wanted a side (left, right), all, or a byte number; got {other:?}")
            })?;
            if byte >= LIGHTS_LENGTH as u8 {
                return Err(format!(
                    "byte {byte} is outside the {LIGHTS_LENGTH} byte report"
                ));
            }
            vec![byte]
        }
    };
    if bytes.is_empty() {
        return Err(format!("nothing in the map belongs to {what:?}"));
    }
    Ok(bytes)
}

/// The line that records a colour the owner picked for the resting panel.
///
/// Same shape as the map — a tab, not a comma — for the same reason: the file is read back by a parser that has no
/// patience for quoted columns, and a colour name is a person's word.
pub fn pick_line(code: u8, name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    Some(format!("{code}\t{name}\n"))
}
