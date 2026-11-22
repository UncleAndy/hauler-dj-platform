//! What a control means right now: shift, and the layer a deck is in.
//!
//! A DJ controller is a keyboard with fewer keys than commands, so the same button means different things depending on
//! state. That state is decided here, in one place, rather than inside a mapping table or a driver: a driver reports
//! what it saw, this turns it into what the DJ meant.
//!
//! Only the two layers the protocol really has are modelled. Anything richer — a browser mode, an effects focus — is
//! added when a device needs it, not before.

/// Which of a deck's two layers is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeckLayer {
    /// The layer the deck starts in: play, cue, hotcues.
    #[default]
    Primary,
    /// The layer behind a shift: loops, beatjump, stems.
    Secondary,
}

impl DeckLayer {
    /// The other layer.
    #[must_use]
    pub fn flipped(self) -> Self {
        match self {
            Self::Primary => Self::Secondary,
            Self::Secondary => Self::Primary,
        }
    }

    /// A name for the log and the picture.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Secondary => "secondary",
        }
    }
}

/// What changed when a button was pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerChange {
    /// Shift went down or up.
    Shift(bool),
    /// A deck moved to another layer.
    Layer(DeckLayer),
}

/// The layer state of the controller.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Layers {
    shift: bool,
    layer: DeckLayer,
}

impl Layers {
    /// Nothing held, primary layer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether shift is held.
    #[must_use]
    pub fn shift(&self) -> bool {
        self.shift
    }

    /// The current layer.
    #[must_use]
    pub fn layer(&self) -> DeckLayer {
        self.layer
    }

    /// Routes a press. `None` means the control is not a layer control and the caller maps it as it always would.
    ///
    /// Names are the driver's: `shift` and `layer` are the only two this type claims.
    pub fn press(&mut self, control: &str, pressed: bool) -> Option<LayerChange> {
        match control {
            "shift" => {
                if self.shift == pressed {
                    return None;
                }
                self.shift = pressed;
                Some(LayerChange::Shift(pressed))
            }
            "layer" if pressed => {
                self.layer = self.layer.flipped();
                Some(LayerChange::Layer(self.layer))
            }
            _ => None,
        }
    }

    /// Forgets everything held, for a device that was unplugged with shift down.
    pub fn release_all(&mut self) -> LayerChange {
        self.shift = false;
        self.layer = DeckLayer::Primary;
        LayerChange::Layer(self.layer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shift_goes_down_and_comes_back_up() {
        let mut layers = Layers::new();
        assert_eq!(layers.press("shift", true), Some(LayerChange::Shift(true)));
        assert!(layers.shift());
        // A second press of the same state is not a change.
        assert_eq!(layers.press("shift", true), None);
        assert_eq!(
            layers.press("shift", false),
            Some(LayerChange::Shift(false))
        );
        assert!(!layers.shift());
    }

    #[test]
    fn the_layer_button_flips_and_a_release_does_nothing() {
        let mut layers = Layers::new();
        assert_eq!(layers.layer(), DeckLayer::Primary);
        assert_eq!(
            layers.press("layer", true),
            Some(LayerChange::Layer(DeckLayer::Secondary))
        );
        assert_eq!(layers.press("layer", false), None);
        assert_eq!(
            layers.press("layer", true),
            Some(LayerChange::Layer(DeckLayer::Primary))
        );
    }

    #[test]
    fn anything_else_is_not_a_layer_control() {
        let mut layers = Layers::new();
        assert_eq!(layers.press("deck_a.play", true), None);
        assert!(!layers.shift());
    }

    #[test]
    fn an_unplugged_device_leaves_nothing_held() {
        let mut layers = Layers::new();
        let _ = layers.press("shift", true);
        let _ = layers.press("layer", true);
        assert_eq!(layers.release_all(), LayerChange::Layer(DeckLayer::Primary));
        assert!(!layers.shift());
        assert_eq!(layers.layer(), DeckLayer::Primary);
    }
}
