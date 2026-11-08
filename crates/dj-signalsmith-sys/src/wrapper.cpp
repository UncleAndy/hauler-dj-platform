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

// A three-way crossover that removes a band instead of shelving it: two Linkwitz-Riley fourth-order
// splits (each two Butterworth second-order sections), summed back together. Killing the low band is a
// highpass at the low crossover; killing the mid band is the low and high halves added; killing the
// high band is a lowpass. All four chains run on the dry signal and the result is blended in by weight,
// so a weight of zero leaves the signal untouched.
struct IsolatorHandle {
    IsolatorHandle(int channels, double sample_rate, double low_hz, double high_hz)
        : channels(channels) {
        const double q = 0.7071067811865476; // sqrt(0.5): two of these make a Linkwitz-Riley fourth order
        const double low = low_hz / sample_rate;
        const double high = high_hz / sample_rate;
        for (int channel = 0; channel < channels && channel < 2; ++channel) {
            for (int section = 0; section < 2; ++section) {
                lowHighpass[channel][section].highpassQ(low, q);
                lowLowpass[channel][section].lowpassQ(low, q);
                highHighpass[channel][section].highpassQ(high, q);
                highLowpass[channel][section].lowpassQ(high, q);
            }
        }
    }

    static float through(signalsmith::filters::BiquadStatic<float> *sections, float value) {
        return sections[1](sections[0](value));
    }

    void reset() {
        for (int channel = 0; channel < 2; ++channel) {
            for (int section = 0; section < 2; ++section) {
                lowHighpass[channel][section].reset();
                lowLowpass[channel][section].reset();
                highHighpass[channel][section].reset();
                highLowpass[channel][section].reset();
            }
        }
    }

    int channels;
    float weights[3] = {0.0f, 0.0f, 0.0f};
    signalsmith::filters::BiquadStatic<float> lowHighpass[2][2] = {};
    signalsmith::filters::BiquadStatic<float> lowLowpass[2][2] = {};
    signalsmith::filters::BiquadStatic<float> highHighpass[2][2] = {};
    signalsmith::filters::BiquadStatic<float> highLowpass[2][2] = {};
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
    if (auto *handle = static_cast<IsolatorHandle *>(raw)) {
        handle->reset();
    }
}

// 0…1 per band: 1 removes the band entirely, 0 leaves the signal alone.
void dj_isolator_set_weights(void *raw, float low, float mid, float high)
{
    if (auto *handle = static_cast<IsolatorHandle *>(raw)) {
        handle->weights[0] = low < 0.0f ? 0.0f : (low > 1.0f ? 1.0f : low);
        handle->weights[1] = mid < 0.0f ? 0.0f : (mid > 1.0f ? 1.0f : mid);
        handle->weights[2] = high < 0.0f ? 0.0f : (high > 1.0f ? 1.0f : high);
    }
}

void dj_isolator_process(void *raw, float *interleaved, int frames)
{
    auto *handle = static_cast<IsolatorHandle *>(raw);
    if (handle == nullptr || frames <= 0) {
        return;
    }
    const float low = handle->weights[0];
    const float mid = handle->weights[1];
    const float high = handle->weights[2];
    if (low <= 0.0f && mid <= 0.0f && high <= 0.0f) {
        // Nothing killed: not a filter at all, and the signal comes back bit for bit.
        return;
    }
    const int channels = handle->channels < 2 ? handle->channels : 2;
    for (int frame = 0; frame < frames; ++frame) {
        for (int channel = 0; channel < channels; ++channel) {
            const size_t index = static_cast<size_t>(frame) * 2 + static_cast<size_t>(channel);
            const float dry = interleaved[index];
            float value = dry;
            if (low > 0.0f) {
                const float removed = IsolatorHandle::through(handle->lowHighpass[channel], dry);
                value = value * (1.0f - low) + removed * low;
            }
            if (high > 0.0f) {
                const float removed = IsolatorHandle::through(handle->highLowpass[channel], dry);
                value = value * (1.0f - high) + removed * high;
            }
            if (mid > 0.0f) {
                // The mid band is what is left when the low half and the high half are added: a
                // Linkwitz-Riley lowpass and highpass sum to one, so their complement is exact.
                const float removed = IsolatorHandle::through(handle->lowLowpass[channel], dry) +
                                      IsolatorHandle::through(handle->highHighpass[channel], dry);
                value = value * (1.0f - mid) + removed * mid;
            }
            interleaved[index] = value;
        }
    }
}

} // extern "C"
