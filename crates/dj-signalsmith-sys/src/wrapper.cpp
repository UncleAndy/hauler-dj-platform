// C ABI over the vendored Signalsmith DSP headers.
//
// Only what the platform actually uses is exposed: a meter that rises to a peak and falls back at a
// steady rate. Each new consumer adds its own functions here rather than a general "call any header"
// surface, because every function in this file is a promise to the Rust side about lifetimes.

#include <cstddef>

#include "signalsmith-dsp/envelopes.h"

namespace {

struct Handle {
    explicit Handle(int length)
        : decay(length) {
    }

    signalsmith::envelopes::PeakDecayLinear<float> decay;
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

} // extern "C"
