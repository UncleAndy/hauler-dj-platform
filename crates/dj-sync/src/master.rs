//! Who is the master: the deck the others follow (DJP-SYNC-001).
//!
//! Two decks cannot both lead, and the choice has to be explainable: a DJ who presses `master` on a
//! deck expects the others to follow that deck and only that deck, while a DJ who has chosen nothing
//! expects the one already playing to lead rather than silence.

use dj_domain::DeckId;

/// How the master is decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MasterPolicy {
    /// The DJ has chosen a deck; it leads even when it is paused, because a paused deck is a decision
    /// about the next beat, not an absence of one.
    Explicit(DeckId),
    /// Nobody has chosen: the first deck that is playing leads, and a deck with a track is the fallback.
    Auto,
}

/// A deck as the choice sees it: whether it holds a track, and whether it is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeckCandidate {
    /// Which deck this is.
    pub deck: DeckId,
    /// Whether a track is loaded.
    pub loaded: bool,
    /// Whether the deck is playing.
    pub playing: bool,
}

/// The deck the others should follow, if any.
///
/// The order of `decks` is the order of preference for [`MasterPolicy::Auto`], so a caller that lists
/// the decks A, B, C, D gets a stable answer for the same state — a master that changes between two
/// identical readings would make the phase meter flicker.
pub fn choose_master(policy: MasterPolicy, decks: &[DeckCandidate]) -> Option<DeckId> {
    match policy {
        MasterPolicy::Explicit(deck) => decks
            .iter()
            .find(|candidate| candidate.deck == deck && candidate.loaded)
            .map(|candidate| candidate.deck),
        MasterPolicy::Auto => decks
            .iter()
            .find(|candidate| candidate.loaded && candidate.playing)
            .or_else(|| decks.iter().find(|candidate| candidate.loaded))
            .map(|candidate| candidate.deck),
    }
}
