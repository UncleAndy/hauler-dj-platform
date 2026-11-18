//! «Ready» — это слово движка о деке, а не о файле.
//!
//! Загрузчик знает путь и длину трека, а граф прикрепляет источник на границе блока. Между двумя событиями
//! клиент, услышавший «ready», действует раньше деки: его seek достаётся прошлому источнику, а приход нового
//! ставит голову в ноль — работающая дека, которая не слушается. Свидетелем служит не время, а само правило:
//! пока граф не подтвердил публикацию, снапшот обязан говорить `Loading`.

use dj_deck::{Deck, DeckSource};
use dj_domain::{DeckId, DeckLoad};
use dj_engine::DeckStatic;
use dj_engine::StateCells;

/// Источник, которому всё равно, что из него читают.
struct Silence {
    frames: u64,
}

impl DeckSource for Silence {
    fn sample_rate(&self) -> u32 {
        48_000
    }

    fn frames(&self) -> Option<u64> {
        Some(self.frames)
    }

    fn read(&mut self, out: &mut [f32]) -> Result<usize, String> {
        out.fill(0.0);
        Ok(out.len() / 2)
    }

    fn seek(&mut self, frame: u64) -> Result<u64, String> {
        Ok(frame.min(self.frames.saturating_sub(1)))
    }
}

fn published_ready(path: &str) -> DeckStatic {
    DeckStatic {
        deck: DeckId::A,
        load: DeckLoad::Ready,
        track: None,
        total_frames: Some(480_000),
        waveform: None,
        auto_gain: 1.0,
        auto_gain_db: None,
        analyzing: false,
        analysis_lock: false,
        grid: None,
        source_rate: 48_000,
        path: Some(path.to_string()),
    }
}

fn holds_a_track() -> Deck {
    let mut deck = Deck::new(DeckId::A);
    deck.attach(Box::new(Silence { frames: 480_000 }), None);
    deck
}

#[test]
fn a_ready_publication_the_graph_has_not_answered_reads_as_loading() {
    let cells = StateCells::default();
    let deck = holds_a_track();
    // The deck holds the *previous* track: `attached` is true, and that is exactly the lie.
    cells.publish(&[deck], &dj_mixer::Mixer::new(2), 0);
    let statics = [published_ready("/music/next.wav")];
    // The loader published the new track and the graph has not attached it yet.
    cells.note_publication(DeckId::A);
    let snapshot = cells.read(&statics, 48_000, 0, 0);
    assert!(
        matches!(snapshot.decks[0].load, DeckLoad::Loading { .. }),
        "the snapshot called a track ready that the deck does not hold yet: {:?}",
        snapshot.decks[0].load
    );

    // The graph attached it: now the same publication is honest.
    cells.note_applied(DeckId::A);
    let snapshot = cells.read(&statics, 48_000, 0, 0);
    assert!(
        matches!(snapshot.decks[0].load, DeckLoad::Ready),
        "the track is in the deck and the snapshot still says {:?}",
        snapshot.decks[0].load
    );
}
