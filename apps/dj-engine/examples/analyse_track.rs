//! Что движок думает о темпе трека и каким путём: для проверки и разбора жалоб на сетку.
//!
//! Запуск: `cargo run -p dj-engine --example analyse_track -- <файл>`

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: analyse_track <file>");
        return;
    };
    println!("источник уровня: {}", dj_engine::beatmodel::source());
    let started = std::time::Instant::now();
    match dj_engine::beatmodel::analyse(std::path::Path::new(&path)) {
        Some(tempo) => println!(
            "{:.3} BPM, участков {}, метрум {}, уверенность {:.2}, за {:?}",
            tempo.bpm(),
            tempo.segments.len(),
            tempo.metre,
            tempo.confidence,
            started.elapsed()
        ),
        None => println!("темпа нет (за {:?})", started.elapsed()),
    }
}
