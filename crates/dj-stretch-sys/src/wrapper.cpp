// C ABI over Signalsmith Stretch, for the deck's key lock.
//
// The library is header-only and templated over the sample type and the buffer accessor; this file
// is the one place that knows about it. Buffers cross the boundary interleaved, because that is what
// the deck speaks, and are turned into per-channel pointers here.

#include "signalsmith-stretch.h"

#include <cstring>
#include <vector>

namespace {

struct Handle {
    Handle(double sample_rate, double block_seconds, double interval_seconds)
        : stretch()
        , channels(2)
    {
        if (block_seconds > 0.0 && interval_seconds > 0.0) {
            // A deck cannot afford the library's default latency: the sound arrives late and a loop
            // marker runs ahead of what is heard. Shortening the block trades a little smoothness for
            // the alignment a DJ mixes with.
            stretch.configure(channels,
                              static_cast<int>(sample_rate * block_seconds),
                              static_cast<int>(sample_rate * interval_seconds));
        } else {
            stretch.presetDefault(channels, static_cast<float>(sample_rate));
        }
        planar.assign(static_cast<size_t>(channels) * capacity, 0.0f);
        produced.assign(static_cast<size_t>(channels) * capacity, 0.0f);
        pointers.assign(channels, nullptr);
        out_pointers.assign(channels, nullptr);
        for (int channel = 0; channel < channels; ++channel) {
            pointers[static_cast<size_t>(channel)] = planar.data() + static_cast<size_t>(channel) * capacity;
            out_pointers[static_cast<size_t>(channel)] =
                produced.data() + static_cast<size_t>(channel) * capacity;
        }
    }

    signalsmith::stretch::SignalsmithStretch<float> stretch;
    int channels;
    // Room for one block of input on each channel. Grown only when a caller asks for something bigger
    // than the engine ever does, and never inside the audio callback of a real run.
    size_t capacity = 8192;
    std::vector<float> planar;
    // The library forbids the same buffers for input and output: it reads a block, writes the
    // previous one, and aliasing would hand it its own output.
    std::vector<float> produced;
    std::vector<float *> pointers;
    std::vector<float *> out_pointers;
};

} // namespace

extern "C" {

void *dj_stretch_new(int channels, double sample_rate)
{
    if (channels != 2) {
        return nullptr;
    }
    return new Handle(sample_rate, 0.0, 0.0);
}

void *dj_stretch_new_custom(int channels, double sample_rate, double block_seconds, double interval_seconds)
{
    if (channels != 2) {
        return nullptr;
    }
    return new Handle(sample_rate, block_seconds, interval_seconds);
}

void dj_stretch_free(void *raw)
{
    delete static_cast<Handle *>(raw);
}

void dj_stretch_reset(void *raw)
{
    if (auto *handle = static_cast<Handle *>(raw)) {
        handle->stretch.reset();
    }
}

int dj_stretch_latency(void *raw)
{
    if (auto *handle = static_cast<Handle *>(raw)) {
        return handle->stretch.outputLatency();
    }
    return 0;
}

int dj_stretch_block(void *raw)
{
    if (auto *handle = static_cast<Handle *>(raw)) {
        return handle->stretch.blockSamples();
    }
    return 0;
}

// Interleaved in, interleaved out, any sizes. The library keeps the history between calls, so the
// caller only has to keep the average ratio right.
void dj_stretch_process(void *raw, const float *input, int input_frames, float *output, int output_frames)
{
    auto *handle = static_cast<Handle *>(raw);
    if (handle == nullptr || input_frames < 0 || output_frames < 0) {
        return;
    }
    const size_t needed = static_cast<size_t>(input_frames) > static_cast<size_t>(output_frames)
                              ? static_cast<size_t>(input_frames)
                              : static_cast<size_t>(output_frames);
    if (needed > handle->capacity) {
        handle->capacity = needed;
        handle->planar.assign(static_cast<size_t>(handle->channels) * needed, 0.0f);
        handle->produced.assign(static_cast<size_t>(handle->channels) * needed, 0.0f);
        for (int channel = 0; channel < handle->channels; ++channel) {
            handle->pointers[static_cast<size_t>(channel)] =
                handle->planar.data() + static_cast<size_t>(channel) * needed;
            handle->out_pointers[static_cast<size_t>(channel)] =
                handle->produced.data() + static_cast<size_t>(channel) * needed;
        }
    }

    for (int channel = 0; channel < handle->channels; ++channel) {
        float *line = handle->pointers[static_cast<size_t>(channel)];
        const size_t from = static_cast<size_t>(channel);
        for (int frame = 0; frame < input_frames; ++frame) {
            line[frame] = input[static_cast<size_t>(frame) * 2 + from];
        }
    }

    handle->stretch.process(handle->pointers, input_frames, handle->out_pointers, output_frames);

    for (int channel = 0; channel < handle->channels; ++channel) {
        const float *line = handle->out_pointers[static_cast<size_t>(channel)];
        const size_t to = static_cast<size_t>(channel);
        for (int frame = 0; frame < output_frames; ++frame) {
            output[static_cast<size_t>(frame) * 2 + to] = line[frame];
        }
    }
}

// Points the internal read position at `input`, so a seek does not leave the stretch replaying the
// material before it.
void dj_stretch_seek(void *raw, const float *input, int input_frames, double playback_rate)
{
    auto *handle = static_cast<Handle *>(raw);
    if (handle == nullptr || input_frames <= 0) {
        return;
    }
    const size_t needed = static_cast<size_t>(input_frames);
    if (needed > handle->capacity) {
        handle->capacity = needed;
        handle->planar.assign(static_cast<size_t>(handle->channels) * needed, 0.0f);
        handle->produced.assign(static_cast<size_t>(handle->channels) * needed, 0.0f);
        for (int channel = 0; channel < handle->channels; ++channel) {
            handle->pointers[static_cast<size_t>(channel)] =
                handle->planar.data() + static_cast<size_t>(channel) * needed;
            handle->out_pointers[static_cast<size_t>(channel)] =
                handle->produced.data() + static_cast<size_t>(channel) * needed;
        }
    }
    for (int channel = 0; channel < handle->channels; ++channel) {
        float *line = handle->pointers[static_cast<size_t>(channel)];
        const size_t from = static_cast<size_t>(channel);
        for (int frame = 0; frame < input_frames; ++frame) {
            line[frame] = input[static_cast<size_t>(frame) * 2 + from];
        }
    }
    handle->stretch.seek(handle->pointers, input_frames, playback_rate);
}

} // extern "C"
