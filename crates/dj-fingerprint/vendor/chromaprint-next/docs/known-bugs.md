# Known bugs in the C reference implementation

This port is a **drop-in replacement** for the C library. Applications using the C library's Test3 algorithm have fingerprints stored in databases that were generated with these bugs. Fixing them in the Rust port would produce different fingerprints, breaking compatibility with existing fingerprint databases. The bugs are therefore preserved intentionally and documented here and in `config.rs` so that future maintainers understand the discrepancy between the configuration's declared intent and its actual behavior.

Both bugs affect Algorithm Test3 only.

## Background: how fingerprint generation works

Each algorithm variant is defined by a `FingerprinterConfiguration` that specifies parameters for the pipeline: frame size, frame overlap, classifier weights, and feature flags like interpolation and silence removal. These parameters determine how raw audio is sliced into overlapping frames, how spectral features are extracted, and ultimately which 32-bit sub-fingerprint values are produced.

The **frame overlap** controls how much adjacent FFT frames share input samples. With the default overlap of 2731 (out of 4096 samples), each frame advances by only 1365 samples, creating a dense temporal representation. The Haar-like classifiers then compare rectangular regions across multiple consecutive frames in the integral image, so both the frame cadence and the number of frames directly affect which classifier patterns are seen.

**Interpolation** determines whether FFT bin energy is snapped to the nearest chroma band or smoothly distributed between adjacent bands. When enabled, it reduces quantization noise in the chroma features.

## Bug 1: interpolation is never applied

**Location in C:** [`fingerprinter.cpp`, line 32](https://github.com/acoustid/chromaprint/blob/6b13ce3a81ae931e7477c4856a86bece99157cd8/src/fingerprinter.cpp#L32).

```cpp
m_chroma = new Chroma(MIN_FREQ, MAX_FREQ, config->frame_size(), config->sample_rate(), m_chroma_filter);
//m_chroma->set_interpolate(true);
```

The `Chroma` object is constructed with `m_interpolate = false` (its default). The only line that would enable interpolation based on the configuration is commented out. The `FingerprinterConfiguration::interpolate()` getter and `set_interpolate()` setter exist and are called in the Test3 configuration constructor, but `Fingerprinter` never reads the config's interpolation flag.

**Expected behavior:** Test3's configuration sets `interpolate = true`. The `Chroma` stage should distribute FFT bin energy across adjacent chroma bands using linear interpolation, producing smoother chroma features.

**Actual behavior:** Interpolation is always off for all algorithms. Test3 behaves identically to Test2 at the chroma extraction stage (they share the same classifier weights).

**Rust port:** `config.rs` sets `interpolate: false` for `CONFIG_TEST3`, with a comment explaining the C bug.

## Bug 2: Test3 uses zero frame overlap

**Location in C:** [`fingerprinter_configuration.cpp`, lines 90-96](https://github.com/acoustid/chromaprint/blob/6b13ce3a81ae931e7477c4856a86bece99157cd8/src/fingerprinter_configuration.cpp#L90-L96).

```cpp
FingerprinterConfigurationTest3::FingerprinterConfigurationTest3()
{
    set_classifiers(kClassifiersTest3, 16);
    set_filter_coefficients(kChromaFilterCoefficients, kChromaFilterSize);
    set_interpolate(true);
    set_frame_size(kDefaultFrameSize);
    // set_frame_overlap(kDefaultFrameOverlap);  <-- missing
}
```

The base class constructor initializes `m_frame_overlap = 0`. Test1 and Test2 both explicitly call `set_frame_overlap(kDefaultFrameOverlap)` (= 2731), but Test3's constructor omits this call.

**Expected behavior:** Test3 should use the same frame overlap as Test1/Test2 (2731 samples), producing frames that advance by 1365 samples with substantial temporal overlap for the classifiers.

**Actual behavior:** Frame overlap is 0, so the frame increment equals the full frame size (4096 samples). Each frame is completely independent with no shared samples. This produces roughly 3x fewer frames per second of audio (one every ~371 ms instead of ~124 ms), and the Haar classifiers see a completely different temporal pattern. The combined effect of zero overlap and disabled interpolation means Test3 produces a substantially degraded fingerprint compared to what was likely intended.

For the test audio file (`test_stereo_44100.raw`, ~4 seconds of audio treated as mono), the C implementation produces **0 raw fingerprint items** with Test3. This is because with 0 overlap and the limited audio length, insufficient frames accumulate to exceed the `max_filter_width` threshold (16) required by the classifier loop. The Rust port reproduces this behavior exactly.

**Rust port:** `config.rs` sets `frame_overlap: 0` for `CONFIG_TEST3`, with a comment explaining the C bug.
