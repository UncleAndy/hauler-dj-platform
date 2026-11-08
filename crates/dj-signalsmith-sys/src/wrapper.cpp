// C ABI over the vendored Signalsmith DSP headers.
//
// Only what the platform actually uses is exposed: a meter that rises to a peak and falls back at a
// steady rate. Each new consumer adds its own functions here rather than a general "call any header"
// surface, because every function in this file is a promise to the Rust side about lifetimes.

#include <cstddef>
#include <vector>

#include "signalsmith-basics/include/signalsmith-basics/limiter.h"
#include "signalsmith-dsp/envelopes.h"
#include "signalsmith-dsp/filters.h"

namespace {

struct Handle {
    explicit Handle(int length)
        : decay(length) {
    }

    signalsmith::envelopes::PeakDecayLinear<float> decay;
};

// A lookahead limiter on two channels. The library works on arrays of channel buffers, so the handle
// keeps the planar copies the interleaved audio is unpacked into: allocated once, here, and never in
// the audio callback.
struct LimiterHandle {
    LimiterHandle(int channels, int max_block, double sample_rate, float threshold, float attack_ms,
                  float release_ms)
        : channels(channels)
        , planar(static_cast<size_t>(channels) * static_cast<size_t>(max_block), 0.0f)
        , produced(static_cast<size_t>(channels) * static_cast<size_t>(max_block), 0.0f)
        , inputs(static_cast<size_t>(channels))
        , outputs(static_cast<size_t>(channels)) {
        for (int channel = 0; channel < channels; ++channel) {
            inputs[static_cast<size_t>(channel)] =
                planar.data() + static_cast<size_t>(channel) * static_cast<size_t>(max_block);
            outputs[static_cast<size_t>(channel)] =
                produced.data() + static_cast<size_t>(channel) * static_cast<size_t>(max_block);
        }
        effect.outputLimit = threshold;
        effect.attackMs = attack_ms;
        effect.holdMs = 0.0;
        effect.releaseMs = release_ms;
        effect.inputGain = 1.0;
        // Both channels share one gain: an unlinked limiter moves the stereo image when one side peaks.
        effect.linkChannels = 1.0;
        effect.configure(sample_rate, static_cast<size_t>(max_block),
                         static_cast<size_t>(channels));
    }

    int channels;
    std::vector<float> planar;
    std::vector<float> produced;
    std::vector<float *> inputs;
    std::vector<float *> outputs;
    signalsmith::basics::LimiterFloat effect;
};

// A fractional-delay interpolator. The deck resamples with it: the file's rate against the device's,
// and the tempo on top of that.
struct ResampleHandle {
    enum class Mode { linear, cubic, kaiser16 };

    explicit ResampleHandle(int mode)
        : mode(static_cast<Mode>(mode)) {
    }

    Mode mode;
    signalsmith::delay::InterpolatorKaiserSincN<float, 16> kaiser{};
};

// A three-way crossover with a gain on each band, which is what a mixer's three-band EQ is: two
// Linkwitz-Riley fourth-order splits (each two Butterworth second-order sections), the lowpass below
// the low crossover, everything above it split again into the middle and the top. A Linkwitz-Riley
// pair adds to one in phase, so the three bands add to the dry signal exactly — with every gain at one
// the signal comes back bit for bit, and with every gain at zero nothing comes back at all. A gain
// below one takes its own band down and nothing else; at zero it removes the band, which is what the
// floor of a knob means.
struct IsolatorHandle {
    IsolatorHandle(int channels, double sample_rate, double low_hz, double high_hz)
        : channels(channels) {
        glideSamples = sample_rate > 0.0 ? static_cast<int>(sample_rate * 0.005) : 240;
        if (glideSamples < 1) {
            glideSamples = 1;
        }
        const double q = 0.7071067811865476; // sqrt(0.5): two of these make a Linkwitz-Riley fourth order
        const double low = low_hz / sample_rate;
        const double high = high_hz / sample_rate;
        for (int channel = 0; channel < channels && channel < 2; ++channel) {
            for (int section = 0; section < 2; ++section) {
                lowLowpass[channel][section].lowpassQ(low, q);
                lowHighpass[channel][section].highpassQ(low, q);
                midLowpass[channel][section].lowpassQ(high, q);
                midHighpass[channel][section].highpassQ(high, q);
            }
        }
    }

    static float through(signalsmith::filters::BiquadStatic<float> *sections, float value) {
        return sections[1](sections[0](value));
    }

    void reset() {
        for (int channel = 0; channel < 2; ++channel) {
            for (int section = 0; section < 2; ++section) {
                lowLowpass[channel][section].reset();
                lowHighpass[channel][section].reset();
                midLowpass[channel][section].reset();
                midHighpass[channel][section].reset();
            }
        }
    }

    int channels;
    // Linear gains: 1 is unity, 0 removes the band, above 1 is a boost. `gains` is where a knob points,
    // `reached` is where the multiplier currently stands: a gain applied in one sample is a step in the
    // waveform, and a step is a tick in the middle of a track.
    float gains[3] = {1.0f, 1.0f, 1.0f};
    float reached[3] = {1.0f, 1.0f, 1.0f};
    float glide[3] = {0.0f, 0.0f, 0.0f};
    int glideLeft = 0;
    // How much of the split is in the output, 0…1. At unity the split is not heard at all — but it is
    // not the same waveform as the dry signal: a Linkwitz-Riley pair sums to an allpass, so the sum is
    // the dry signal turned by tens of degrees. Switching between the two in one sample is a step of
    // fifty times the material's own, so the split is faded in and out instead.
    float wet = 0.0f;
    float wetStep = 0.0f;
    int wetLeft = 0;
    float wetTarget = 0.0f;
    // Five milliseconds: long enough that a scroll notch is a slope rather than an edge, short enough
    // that a hand still feels the knob answer it.
    int glideSamples = 240;
    signalsmith::filters::BiquadStatic<float> lowLowpass[2][2] = {};
    signalsmith::filters::BiquadStatic<float> lowHighpass[2][2] = {};
    signalsmith::filters::BiquadStatic<float> midLowpass[2][2] = {};
    signalsmith::filters::BiquadStatic<float> midHighpass[2][2] = {};
};

// Views an interleaved buffer as one channel with an index base, which is what the interpolators
// expect from `data[i]`.
struct ChannelView {
    const float *base;
    int channel;

    float operator[](int index) const {
        return base[static_cast<size_t>(index) * 2 + static_cast<size_t>(channel)];
    }
};

} // namespace

extern "C" {

void *dj_peak_decay_new(int length)
{
    if (length <= 0) {
        return nullptr;
    }
    auto *handle = new Handle(length);
    handle->decay.reset(0.0f);
    return handle;
}

void dj_peak_decay_free(void *raw)
{
    delete static_cast<Handle *>(raw);
}

void dj_peak_decay_reset(void *raw)
{
    if (auto *handle = static_cast<Handle *>(raw)) {
        handle->decay.reset(0.0f);
    }
}

// Feed one block: the peak of the block, held for the length of the block so the decay clock follows
// real time. Returns the value the meter shows at the end of the block.
float dj_peak_decay_block(void *raw, float peak, int frames)
{
    auto *handle = static_cast<Handle *>(raw);
    if (handle == nullptr) {
        return peak;
    }
    if (frames <= 0) {
        return handle->decay(peak);
    }
    float value = peak;
    for (int i = 0; i < frames; ++i) {
        // The same peak across the block: the value rises to it at once and starts falling only when a
        // quieter block arrives.
        value = handle->decay(peak);
    }
    return value;
}

void *dj_limiter_new(int channels, int max_block, double sample_rate, float threshold,
                    float attack_ms, float release_ms)
{
    if (channels <= 0 || max_block <= 0) {
        return nullptr;
    }
    return new LimiterHandle(channels, max_block, sample_rate, threshold, attack_ms, release_ms);
}

void dj_limiter_free(void *raw)
{
    delete static_cast<LimiterHandle *>(raw);
}

void dj_limiter_reset(void *raw)
{
    if (auto *handle = static_cast<LimiterHandle *>(raw)) {
        handle->effect.reset();
    }
}

// Frames the limiter delays the signal by: its attack time, which is what lets the gain come down
// before the peak reaches the output.
int dj_limiter_latency(void *raw)
{
    if (auto *handle = static_cast<LimiterHandle *>(raw)) {
        return handle->effect.latencySamples();
    }
    return 0;
}

// Limit one interleaved stereo block in place.
void dj_limiter_process(void *raw, float *interleaved, int frames)
{
    auto *handle = static_cast<LimiterHandle *>(raw);
    if (handle == nullptr || frames <= 0) {
        return;
    }
    const int channels = handle->channels;
    const int count = frames < static_cast<int>(handle->planar.size() / channels)
                          ? frames
                          : static_cast<int>(handle->planar.size() / channels);
    for (int channel = 0; channel < channels; ++channel) {
        float *line = handle->inputs[static_cast<size_t>(channel)];
        for (int frame = 0; frame < count; ++frame) {
            line[frame] = interleaved[static_cast<size_t>(frame) * channels + channel];
        }
    }
    handle->effect.process(handle->inputs.data(), handle->outputs.data(),
                           static_cast<size_t>(count));
    for (int channel = 0; channel < channels; ++channel) {
        const float *line = handle->outputs[static_cast<size_t>(channel)];
        for (int frame = 0; frame < count; ++frame) {
            interleaved[static_cast<size_t>(frame) * channels + channel] = line[frame];
        }
    }
}

void *dj_resampler_new(int mode)
{
    if (mode < 0 || mode > 2) {
        return nullptr;
    }
    return new ResampleHandle(mode);
}

void dj_resampler_free(void *raw)
{
    delete static_cast<ResampleHandle *>(raw);
}

// Interpolate `out_frames` frames at `phase`, `phase + step`, … measured from frame `base` of
// `input`, and return the position reached. The caller keeps the fractional part as its next phase.
double dj_resample(void *raw, const float *input, int in_frames, float *output, int out_frames,
                   int base, double phase, double step)
{
    auto *handle = static_cast<ResampleHandle *>(raw);
    if (handle == nullptr || in_frames <= 0 || out_frames <= 0) {
        return phase;
    }
    const int taps = (handle->mode == ResampleHandle::Mode::kaiser16)
                         ? signalsmith::delay::InterpolatorKaiserSincN<float, 16>::inputLength
                         : (handle->mode == ResampleHandle::Mode::cubic) ? 4 : 2;
    const int lookback = (handle->mode == ResampleHandle::Mode::kaiser16)
                             ? signalsmith::delay::InterpolatorKaiserSincN<float, 16>::latency
                             : (handle->mode == ResampleHandle::Mode::cubic) ? 1 : 0;

    double position = phase;
    int written = 0;
    for (int frame = 0; frame < out_frames; ++frame) {
        const int index = base + static_cast<int>(position);
        const int start = index - lookback;
        const float fraction = static_cast<float>(position - static_cast<int>(position));
        if (start < 0 || start + taps > in_frames) {
            break;
        }
        ChannelView left{input + static_cast<size_t>(start) * 2, 0};
        ChannelView right{input + static_cast<size_t>(start) * 2, 1};
        float valueLeft = 0.0f;
        float valueRight = 0.0f;
        switch (handle->mode) {
        case ResampleHandle::Mode::linear: {
            signalsmith::delay::InterpolatorLinear<float> interpolator;
            valueLeft = interpolator.fractional(left, fraction);
            valueRight = interpolator.fractional(right, fraction);
            break;
        }
        case ResampleHandle::Mode::cubic: {
            signalsmith::delay::InterpolatorCubic<float> interpolator;
            valueLeft = interpolator.fractional(left, fraction);
            valueRight = interpolator.fractional(right, fraction);
            break;
        }
        case ResampleHandle::Mode::kaiser16:
            valueLeft = handle->kaiser.fractional(left, fraction);
            valueRight = handle->kaiser.fractional(right, fraction);
            break;
        }
        output[static_cast<size_t>(frame) * 2] = valueLeft;
        output[static_cast<size_t>(frame) * 2 + 1] = valueRight;
        written = frame + 1;
        position += step;
    }
    // Anything not written stays silent: the caller sees a short block as the end of the material.
    for (size_t sample = static_cast<size_t>(written) * 2; sample < static_cast<size_t>(out_frames) * 2;
         ++sample) {
        output[sample] = 0.0f;
    }
    return position;
}

void *dj_isolator_new(int channels, double sample_rate, double low_hz, double high_hz)
{
    if (channels <= 0 || sample_rate <= 0.0 || low_hz <= 0.0 || high_hz <= low_hz) {
        return nullptr;
    }
    return new IsolatorHandle(channels, sample_rate, low_hz, high_hz);
}

void dj_isolator_free(void *raw)
{
    delete static_cast<IsolatorHandle *>(raw);
}

void dj_isolator_reset(void *raw)
{
    auto *handle = static_cast<IsolatorHandle *>(raw);
    if (handle == nullptr) {
        return;
    }
    handle->reset();
    // A reset is a new track: the multiplier starts where the knobs point, with nothing to travel.
    for (int band = 0; band < 3; ++band) {
        handle->reached[band] = handle->gains[band];
        handle->glide[band] = 0.0f;
    }
    handle->glideLeft = 0;
    handle->wet = handle->wetTarget;
    handle->wetStep = 0.0f;
    handle->wetLeft = 0;
}

// Linear gain per band: 1 is unity, 0 removes the band. A negative or broken gain is silence for that
// band rather than a sign flip in the mix. The change is travelled to over a few milliseconds rather
// than applied at once.
void dj_isolator_set_gains(void *raw, float low, float mid, float high)
{
    auto *handle = static_cast<IsolatorHandle *>(raw);
    if (handle == nullptr) {
        return;
    }
    const float wanted[3] = {low, mid, high};
    bool changed = false;
    for (int band = 0; band < 3; ++band) {
        const float value = wanted[band] > 0.0f && wanted[band] <= 64.0f ? wanted[band] : 0.0f;
        if (value != handle->gains[band]) {
            handle->gains[band] = value;
            changed = true;
        }
    }
    if (!changed) {
        return;
    }
    // The glide starts from wherever the multiplier stands, so a second notch mid-glide bends the
    // slope instead of jumping onto a new one.
    for (int band = 0; band < 3; ++band) {
        handle->glide[band] = (handle->gains[band] - handle->reached[band]) /
                              static_cast<float>(handle->glideSamples);
    }
    handle->glideLeft = handle->glideSamples;
    const bool unity = handle->gains[0] == 1.0f && handle->gains[1] == 1.0f &&
                       handle->gains[2] == 1.0f;
    const float wantedWet = unity ? 0.0f : 1.0f;
    if (wantedWet != handle->wetTarget) {
        handle->wetTarget = wantedWet;
        handle->wetStep = (wantedWet - handle->wet) / static_cast<float>(handle->glideSamples);
        handle->wetLeft = handle->glideSamples;
    }
}

void dj_isolator_process(void *raw, float *interleaved, int frames)
{
    auto *handle = static_cast<IsolatorHandle *>(raw);
    if (handle == nullptr || frames <= 0) {
        return;
    }
    const int channels = handle->channels < 2 ? handle->channels : 2;
    // The chains always run, even while every band stands at unity: a splitter that stops while it is
    // bypassed holds a state from whenever it last ran, and the first sample after it is switched back
    // in is a step (measured at 51 times the material's own). What the bypass saves is the write, not
    // the arithmetic: at unity the dry sample goes out, so the signal still comes back bit for bit.
    for (int frame = 0; frame < frames; ++frame) {
        if (handle->glideLeft > 0) {
            for (int band = 0; band < 3; ++band) {
                handle->reached[band] += handle->glide[band];
            }
            handle->glideLeft -= 1;
            if (handle->glideLeft == 0) {
                for (int band = 0; band < 3; ++band) {
                    handle->reached[band] = handle->gains[band];
                }
            }
        }
        if (handle->wetLeft > 0) {
            handle->wet += handle->wetStep;
            handle->wetLeft -= 1;
            if (handle->wetLeft == 0) {
                handle->wet = handle->wetTarget;
            }
        }
        const float wet = handle->wet;
        const float low = handle->reached[0];
        const float mid = handle->reached[1];
        const float high = handle->reached[2];
        for (int channel = 0; channel < channels; ++channel) {
            const size_t index = static_cast<size_t>(frame) * 2 + static_cast<size_t>(channel);
            const float dry = interleaved[index];
            // Nested splits, which is the only way the three bands add up to the dry signal: the low
            // crossover separates the bottom from everything above it, and the high crossover then
            // separates that upper part into the middle and the top. A Linkwitz-Riley pair is in phase
            // at all frequencies, so each split is exact — and so is their sum. Subtracting a filter's
            // output from the dry signal instead would not be: away from the crossover the two are not
            // in phase, and the "complement" comes back louder than the dry signal.
            const float lowBand = IsolatorHandle::through(handle->lowLowpass[channel], dry);
            const float upper = IsolatorHandle::through(handle->lowHighpass[channel], dry);
            const float midBand = IsolatorHandle::through(handle->midLowpass[channel], upper);
            const float highBand = IsolatorHandle::through(handle->midHighpass[channel], upper);
            const float sum = lowBand * low + midBand * mid + highBand * high;
            // At a wet of zero this is exactly the dry sample, bit for bit: nothing is multiplied by
            // an almost-one and nothing drifts.
            interleaved[index] = dry + (sum - dry) * wet;
        }
    }
}

// Response of one band, or of the whole split (`band` 0…2, or 3 for everything), in decibels.
//
// Computed from the sections' own coefficients rather than from a formula: the library designs its
// crossings with a bilinear (bandwidth-compensated) method, so textbook Linkwitz-Riley magnitudes are
// a few tenths of a decibel away from what the filter actually does — and a curve drawn from the
// textbook would disagree with the ear.
double dj_isolator_response_db(void *raw, int band, double hertz, double sample_rate)
{
    auto *handle = static_cast<IsolatorHandle *>(raw);
    if (handle == nullptr || sample_rate <= 0.0) {
        return 0.0;
    }
    const float w = static_cast<float>(hertz / sample_rate);
    auto cascaded = [w](const signalsmith::filters::BiquadStatic<float> *sections) {
        return sections[0].response(w) * sections[1].response(w);
    };
    const int channel = 0; // both channels hold the same coefficients
    const auto low = cascaded(handle->lowLowpass[channel]);
    const auto upper = cascaded(handle->lowHighpass[channel]);
    const auto mid = upper * cascaded(handle->midLowpass[channel]);
    const auto high = upper * cascaded(handle->midHighpass[channel]);
    std::complex<float> total = {0.0f, 0.0f};
    if (band == 0) {
        total = low * handle->gains[0];
    } else if (band == 1) {
        total = mid * handle->gains[1];
    } else if (band == 2) {
        total = high * handle->gains[2];
    } else {
        total = low * handle->gains[0] + mid * handle->gains[1] + high * handle->gains[2];
    }
    const float magnitude = std::abs(total);
    return 20.0 * std::log10(magnitude > 1e-20f ? magnitude : 1e-20f);
}

} // extern "C"
