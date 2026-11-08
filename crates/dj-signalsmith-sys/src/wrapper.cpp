// C ABI over the vendored Signalsmith DSP headers.
//
// Only what the platform actually uses is exposed: a meter that rises to a peak and falls back at a
// steady rate. Each new consumer adds its own functions here rather than a general "call any header"
// surface, because every function in this file is a promise to the Rust side about lifetimes.

#include <cstddef>
#include <vector>

#include "signalsmith-basics/include/signalsmith-basics/limiter.h"
#include "signalsmith-dsp/envelopes.h"

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

} // extern "C"
