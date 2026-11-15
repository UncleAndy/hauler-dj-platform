//! Дамп лог-мел-спектрограммы в сырой f32: для сверки с эталоном из torchaudio.
//!
//! Запуск: `cargo run -p dj-beatmodel --release --example mel_dump -- <вход.f32> <выход.f32> <кадры>`

fn main() {
    let mut arguments = std::env::args().skip(1);
    let (Some(input), Some(output)) = (arguments.next(), arguments.next()) else {
        eprintln!("usage: mel_dump <input.f32> <output.f32> [frames]");
        return;
    };
    let bytes = match std::fs::read(&input) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("{input}: {error}");
            return;
        }
    };
    let samples: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect();
    let config = dj_beatmodel::MelConfig::default();
    let started = std::time::Instant::now();
    let spect = dj_beatmodel::log_mel(&samples, &config);
    let frames = spect.len() / config.n_mels;
    let mut body = Vec::with_capacity(spect.len() * 4);
    for value in &spect {
        body.extend_from_slice(&value.to_le_bytes());
    }
    match std::fs::write(&output, &body) {
        Ok(()) => println!(
            "{} кадров × {} полос за {:?} → {output}",
            frames,
            config.n_mels,
            started.elapsed()
        ),
        Err(error) => eprintln!("{output}: {error}"),
    }
}
