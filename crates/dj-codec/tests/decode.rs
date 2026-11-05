//! Decoding and seeking, per format (A02 acceptance).

mod common;

use dj_codec::{MediaError, MediaFormat, StreamDecoder};

use common::{fixture, peak, temp_path, write_wav};

fn read_all(decoder: &mut StreamDecoder) -> Vec<f32> {
    let mut out = Vec::new();
    let mut block = vec![0.0f32; 2048];
    loop {
        match decoder.read_frames(&mut block) {
            Ok(0) => break,
            Ok(frames) => out.extend_from_slice(&block[..frames * 2]),
            Err(err) => panic!("read failed: {err}"),
        }
    }
    out
}

#[test]
fn a_wav_is_fully_read_and_reports_its_shape() {
    let path = temp_path("tone.wav");
    let expected = write_wav(&path, 44_100, 1.0, 0);

    let mut decoder = StreamDecoder::open(&path).unwrap_or_else(|err| panic!("open wav: {err}"));
    let info = decoder.info().clone();
    assert_eq!(info.format, MediaFormat::Wav);
    assert_eq!(info.sample_rate, 44_100);
    assert_eq!(info.channels, 2);
    assert_eq!(info.frames, Some(expected));

    let samples = read_all(&mut decoder);
    assert_eq!(samples.len() as u64, expected * 2);
    assert!(peak(&samples) > 0.5, "the tone must be audible");
    assert_eq!(decoder.stats().corrupt_packets, 0);
}

#[test]
fn flac_decodes_to_the_same_samples_as_the_wav_it_came_from() {
    let wav_path = temp_path("reference.wav");
    write_wav(&wav_path, 44_100, 1.0, 0);

    let mut wav = StreamDecoder::open(&wav_path).unwrap_or_else(|err| panic!("open wav: {err}"));
    let mut flac = StreamDecoder::open(&fixture("tone-1s-44100.flac"))
        .unwrap_or_else(|err| panic!("open flac: {err}"));
    assert_eq!(flac.info().format, MediaFormat::Flac);

    let wav_samples = read_all(&mut wav);
    let flac_samples = read_all(&mut flac);
    let compare = wav_samples.len().min(flac_samples.len());
    assert!(
        compare > 40_000,
        "expected about a second of audio, got {compare}"
    );
    for index in 0..compare {
        let diff = (wav_samples[index] - flac_samples[index]).abs();
        assert!(diff < 1e-3, "sample {index} differs by {diff}");
    }
}

#[test]
fn mp3_arrives_with_the_encoder_delay_already_trimmed() {
    let mut decoder = StreamDecoder::open(&fixture("tone-1s-44100-cbr.mp3"))
        .unwrap_or_else(|err| panic!("open mp3: {err}"));
    let info = decoder.info().clone();
    assert_eq!(info.format, MediaFormat::Mp3);
    assert!(
        info.gapless.is_some(),
        "a LAME-tagged file must report delay/padding"
    );

    let mut block = vec![0.0f32; 256];
    let frames = decoder
        .read_frames(&mut block)
        .unwrap_or_else(|err| panic!("read: {err}"));
    assert!(frames > 0);
    assert!(
        peak(&block[..frames * 2]) > 0.1,
        "the first samples must be audio, not the encoder delay"
    );

    let total = read_all(&mut decoder).len() / 2 + frames;
    let expected = usize::try_from(info.frames.unwrap_or_default()).unwrap_or(0);
    let drift = total.abs_diff(expected);
    assert!(
        drift < 2_000,
        "decoded {total} frames against a declared {expected} (drift {drift})"
    );
}

#[test]
fn a_vbr_mp3_still_reports_a_usable_duration() {
    let decoder = StreamDecoder::open(&fixture("tone-1s-44100-vbr.mp3"))
        .unwrap_or_else(|err| panic!("open vbr mp3: {err}"));
    let info = decoder.info();
    let duration = info
        .duration_seconds()
        .unwrap_or_else(|| panic!("duration"));
    assert!(
        (0.90..1.10).contains(&duration),
        "VBR duration {duration} s is not plausible for a one-second file"
    );
    assert!(info.needs_pcm_cache(), "MP3 belongs to the PCM-cache class");
}

#[test]
fn ogg_vorbis_decodes() {
    let mut decoder = StreamDecoder::open(&fixture("tone-1s-44100.ogg"))
        .unwrap_or_else(|err| panic!("open ogg: {err}"));
    assert_eq!(decoder.info().format, MediaFormat::OggVorbis);
    let samples = read_all(&mut decoder);
    assert!(!samples.is_empty());
    assert!(peak(&samples) > 0.1);
}

#[test]
fn a_seek_in_wav_lands_on_the_exact_sample() {
    let path = temp_path("silence-then-tone.wav");
    write_wav(&path, 44_100, 2.0, 44_100);
    let mut decoder = StreamDecoder::open(&path).unwrap_or_else(|err| panic!("open: {err}"));

    let mut block = vec![0.0f32; 512];
    let landed = decoder
        .seek(44_100)
        .unwrap_or_else(|err| panic!("seek: {err}"));
    assert_eq!(landed, 44_100, "WAV seek must be sample-accurate");
    let frames = decoder
        .read_frames(&mut block)
        .unwrap_or_else(|err| panic!("read: {err}"));
    assert!(frames > 0);
    assert!(peak(&block[..frames * 2]) > 0.3, "the tone starts here");

    let landed = decoder
        .seek(0)
        .unwrap_or_else(|err| panic!("seek 0: {err}"));
    assert_eq!(landed, 0);
    let frames = decoder
        .read_frames(&mut block)
        .unwrap_or_else(|err| panic!("read: {err}"));
    assert!(frames > 0);
    assert!(
        peak(&block[..frames * 2]) < 1e-6,
        "the first second is silence"
    );
}

#[test]
fn a_seek_in_flac_lands_on_or_before_the_target() {
    let mut decoder = StreamDecoder::open(&fixture("silence-then-tone-2s-44100.flac"))
        .unwrap_or_else(|err| panic!("open flac: {err}"));
    let landed = decoder
        .seek(44_100)
        .unwrap_or_else(|err| panic!("seek: {err}"));
    assert!(
        landed <= 44_100 && 44_100 - landed < 4_608,
        "FLAC seek landed at {landed}, request was 44100"
    );
    let mut block = vec![0.0f32; 4096];
    let frames = decoder
        .read_frames(&mut block)
        .unwrap_or_else(|err| panic!("read: {err}"));
    assert!(
        peak(&block[..frames * 2]) > 0.3,
        "tone begins at one second"
    );
}

#[test]
fn a_seek_in_mp3_lands_within_one_codec_frame() {
    let mut decoder = StreamDecoder::open(&fixture("tone-1s-44100-cbr.mp3"))
        .unwrap_or_else(|err| panic!("open mp3: {err}"));
    let landed = decoder
        .seek(22_050)
        .unwrap_or_else(|err| panic!("seek: {err}"));
    assert!(
        landed <= 22_050 && 22_050 - landed <= 1_152,
        "MP3 seek landed at {landed}, one frame is 1152 samples"
    );
    let mut block = vec![0.0f32; 256];
    let frames = decoder
        .read_frames(&mut block)
        .unwrap_or_else(|err| panic!("read: {err}"));
    assert!(frames > 0);
}

#[test]
fn garbage_is_an_error_and_not_a_panic() {
    let path = temp_path("broken.mp3");
    std::fs::write(&path, vec![0x7fu8; 4096]).unwrap_or_else(|err| panic!("write: {err}"));
    match StreamDecoder::open(&path) {
        Ok(_) => panic!("a file of noise must not open as MP3"),
        Err(MediaError::UnsupportedFormat(_))
        | Err(MediaError::NoAudioTrack)
        | Err(MediaError::UnsupportedCodec(_)) => {}
        Err(other) => panic!("unexpected error for garbage input: {other}"),
    }
}

#[test]
fn reading_past_the_end_returns_zero_rather_than_padding() {
    let path = temp_path("short.wav");
    write_wav(&path, 44_100, 0.1, 0);
    let mut decoder = StreamDecoder::open(&path).unwrap_or_else(|err| panic!("open: {err}"));
    let mut block = vec![0.0f32; 44_100 * 2];
    let frames = decoder
        .read_frames(&mut block)
        .unwrap_or_else(|err| panic!("read: {err}"));
    assert_eq!(frames, 4_410);
    let again = decoder
        .read_frames(&mut block)
        .unwrap_or_else(|err| panic!("read: {err}"));
    assert_eq!(again, 0);
}
