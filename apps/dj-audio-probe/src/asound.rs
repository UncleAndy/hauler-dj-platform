//! Parser for the Linux USB-audio descriptors under `/proc/asound`.
//!
//! Why this exists: `cpal` on ALSA reports the *plugin* layer capabilities
//! (`default`, `pipewire`, `hw`), where "up to 32 channels" is what the ALSA API
//! allows, not what the device supports. The authoritative channel count lives in
//! the USB descriptor, which `/proc/asound/card*/stream0` exposes and which is
//! readable even without `/dev/snd`.

/// One interface/altset of a USB audio function.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Altset {
    pub interface: Option<u32>,
    pub altset: Option<u32>,
    pub channels: u16,
    pub format: Option<String>,
    pub rates: Vec<u32>,
    pub channel_map: Option<String>,
    pub endpoint: Option<String>,
}

impl Altset {
    pub fn supports_rate(&self, rate: u32) -> bool {
        self.rates.is_empty() || self.rates.contains(&rate)
    }
}

/// One `/proc/asound/cardN` entry with its playback and capture altsets.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsbCard {
    pub id: String,
    pub name: String,
    pub playback: Vec<Altset>,
    pub capture: Vec<Altset>,
}

impl UsbCard {
    pub fn playback_max_channels(&self) -> u16 {
        self.playback.iter().map(|a| a.channels).max().unwrap_or(0)
    }

    pub fn capture_max_channels(&self) -> u16 {
        self.capture.iter().map(|a| a.channels).max().unwrap_or(0)
    }
}

fn key_value(line: &str) -> Option<(String, String)> {
    let (key, value) = line.split_once(':')?;
    Some((key.trim().to_string(), value.trim().to_string()))
}

fn parse_rates(value: &str) -> Vec<u32> {
    let mut rates: Vec<u32> = value
        .split([',', '-'])
        .filter_map(|piece| piece.trim().parse::<u32>().ok())
        .collect();
    rates.sort_unstable();
    rates.dedup();
    rates
}

fn parse_number(value: &str) -> Option<u32> {
    value
        .split_whitespace()
        .next()
        .and_then(|first| first.parse::<u32>().ok())
}

/// Parse the body of `stream0` into (playback altsets, capture altsets).
pub fn parse_stream(text: &str) -> (Vec<Altset>, Vec<Altset>) {
    let mut playback: Vec<Altset> = Vec::new();
    let mut capture: Vec<Altset> = Vec::new();
    let mut in_playback = false;
    let mut current: Option<Altset> = None;

    let flush = |current: &mut Option<Altset>,
                 playback: &mut Vec<Altset>,
                 capture: &mut Vec<Altset>,
                 in_playback: bool| {
        if let Some(altset) = current.take() {
            if in_playback {
                playback.push(altset);
            } else {
                capture.push(altset);
            }
        }
    };

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == "Playback:" {
            flush(&mut current, &mut playback, &mut capture, in_playback);
            in_playback = true;
            continue;
        }
        if trimmed == "Capture:" {
            flush(&mut current, &mut playback, &mut capture, in_playback);
            in_playback = false;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("Interface ") {
            flush(&mut current, &mut playback, &mut capture, in_playback);
            current = Some(Altset {
                interface: parse_number(rest),
                ..Altset::default()
            });
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("Altset ") {
            // `Interface N` is followed by `Altset M` for the same interface: that is
            // one descriptor, not two. Only a *second* altset starts a new entry.
            let number = parse_number(rest);
            let same_descriptor = current
                .as_ref()
                .is_some_and(|altset| altset.channels == 0 && altset.format.is_none());
            if same_descriptor {
                if let Some(altset) = current.as_mut() {
                    altset.altset = number;
                }
            } else {
                flush(&mut current, &mut playback, &mut capture, in_playback);
                current = Some(Altset {
                    altset: number,
                    ..Altset::default()
                });
            }
            continue;
        }

        let Some((key, value)) = key_value(trimmed) else {
            continue;
        };
        let Some(altset) = current.as_mut() else {
            continue;
        };
        match key.as_str() {
            "Channels" => {
                if let Some(channels) = parse_number(&value) {
                    altset.channels = u16::try_from(channels).unwrap_or(u16::MAX);
                }
            }
            "Format" => altset.format = Some(value),
            "Rates" | "Rate" => altset.rates = parse_rates(&value),
            "Channel map" => altset.channel_map = Some(value),
            "Endpoint" => altset.endpoint = Some(value),
            _ => {}
        }
    }
    flush(&mut current, &mut playback, &mut capture, in_playback);

    (playback, capture)
}

fn card_name(text: &str) -> String {
    let first = text.lines().next().unwrap_or_default().trim();
    // `split(...).next()` would return the whole line when the separator is absent,
    // so the fallback has to be a `split_once` chain rather than `.or_else(next)`.
    let without_bus = first
        .split_once(" at usb-")
        .or_else(|| first.split_once(" at 0x"))
        .map(|(name, _)| name)
        .unwrap_or(first);
    without_bus.trim_end_matches(':').trim().to_string()
}

/// Read every `/proc/asound/cardN/stream0` that the process may access.
pub fn read_cards(root: &str) -> Vec<UsbCard> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    let mut cards: Vec<UsbCard> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("card") {
            continue;
        }
        let path = entry.path().join("stream0");
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if text.trim().is_empty() {
            continue;
        }
        let (playback, capture) = parse_stream(&text);
        if playback.is_empty() && capture.is_empty() {
            continue;
        }
        cards.push(UsbCard {
            id: name,
            name: card_name(&text),
            playback,
            capture,
        });
    }
    cards.sort_by(|a, b| a.id.cmp(&b.id));
    cards
}

#[cfg(test)]
mod tests {
    use super::*;

    const S4: &str = "\
Native Instruments Traktor Kontrol S4 MK3 at usb-0000:00:14.0-11.3.1, high spee : USB Audio

Playback:
  Status: Stop
  Interface 1
    Altset 1
    Format: S32_LE
    Channels: 4
    Endpoint: 0x01 (1 OUT) (ASYNC)
    Rates: 44100, 48000, 88200, 96000
    Data packet interval: 125 us
    Bits: 24
    Channel map: FL FR FC LFE

Capture:
  Status: Stop
  Interface 2
    Altset 1
    Format: S32_LE
    Channels: 8
    Endpoint: 0x81 (1 IN) (ASYNC)
    Rates: 44100, 48000, 88200, 96000
    Channel map: FL FR FC LFE RL RR FLC FRC
";

    #[test]
    fn parses_playback_and_capture_channels() {
        let (playback, capture) = parse_stream(S4);
        assert_eq!(playback.len(), 1);
        assert_eq!(capture.len(), 1);
        assert_eq!(playback[0].channels, 4);
        assert_eq!(capture[0].channels, 8);
        assert_eq!(playback[0].format.as_deref(), Some("S32_LE"));
        assert_eq!(playback[0].rates, vec![44_100, 48_000, 88_200, 96_000]);
        assert_eq!(playback[0].channel_map.as_deref(), Some("FL FR FC LFE"));
        assert!(playback[0].supports_rate(48_000));
        assert!(!playback[0].supports_rate(192_000));
    }

    #[test]
    fn card_name_drops_the_pci_address_and_trailing_colon() {
        assert_eq!(card_name(S4), "Native Instruments Traktor Kontrol S4 MK3");
        assert_eq!(
            card_name("HDA Intel PCH at 0x43510000 irq 189\n"),
            "HDA Intel PCH"
        );
    }

    #[test]
    fn a_capture_only_card_has_no_playback_altsets() {
        let text = "\
REDRAGON Live Camera at usb-0000:00:14.0-11.1, high  : USB Audio

Capture:
  Interface 4
    Altset 1
    Format: S16_LE
    Channels: 2
    Rates: 48000
";
        let (playback, capture) = parse_stream(text);
        assert!(playback.is_empty());
        assert_eq!(capture.len(), 1);
        assert_eq!(capture[0].channels, 2);
    }

    #[test]
    fn range_style_rates_are_expanded() {
        assert_eq!(parse_rates("44100 - 96000"), vec![44_100, 96_000]);
    }
}
