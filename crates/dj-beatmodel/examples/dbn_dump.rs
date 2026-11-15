//! Дамп долей из активации: для сверки декодера с прототипом.
//!
//! Запуск: `cargo run -p dj-beatmodel --release --example dbn_dump -- <активация.f32> <доли.txt> [lambda]`

fn main() {
    let mut arguments = std::env::args().skip(1);
    let (Some(input), Some(output)) = (arguments.next(), arguments.next()) else {
        eprintln!("usage: dbn_dump <activation.f32> <beats.txt> [lambda]");
        return;
    };
    let lambda = arguments
        .next()
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(30.0);
    let bytes = match std::fs::read(&input) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("{input}: {error}");
            return;
        }
    };
    let activation: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect();
    let config = dj_beatmodel::DbnConfig {
        lambda,
        ..dj_beatmodel::DbnConfig::default()
    };
    let started = std::time::Instant::now();
    let beats = dj_beatmodel::decode_beats(&activation, &config);
    let elapsed = started.elapsed();
    let mut body = String::new();
    for beat in &beats {
        body.push_str(&format!("{beat:.4}\n"));
    }
    match std::fs::write(&output, body) {
        Ok(()) => println!("{} долей за {:?}", beats.len(), elapsed),
        Err(error) => eprintln!("{output}: {error}"),
    }
}
