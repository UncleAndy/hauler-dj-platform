//! Что движок думает о темпе трека и каким путём: для проверки и разбора жалоб на сетку.
//!
//! Запуск: `cargo run -p dj-engine --release --example analyse_track -- <файл>`

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: analyse_track <file>");
        return;
    };
    println!("level source: {}", dj_engine::beatmodel::source());
    let started = std::time::Instant::now();
    let Some(tempo) = dj_engine::beatmodel::analyse(std::path::Path::new(&path)) else {
        println!("no tempo (in {:?})", started.elapsed());
        return;
    };
    for segment in &tempo.segments {
        println!(
            "segment: beat {} at frame {}, {:.3} BPM",
            segment.beat, segment.first_beat_frame, segment.bpm
        );
    }
    println!(
        "{:.3} BPM, segments {}, metre {}, confidence {:.2}, in {:?}",
        tempo.bpm(),
        tempo.segments.len(),
        tempo.metre,
        tempo.confidence,
        started.elapsed()
    );
}
