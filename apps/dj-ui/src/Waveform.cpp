#include "Waveform.h"

#include <QFile>
#include <QFileInfo>
#include <QVariantMap>

#include <algorithm>
#include <array>
#include <cmath>
#include <vector>
#include <cstring>

namespace {

// Header written by crates/dj-analysis: magic (8), size (8), modified (16), sample rate (8),
// frames per bucket (8), column count (8).
constexpr int kHeaderBytes = 56;
// Two extremes as i16 and three band levels as u8, per column.
constexpr int kColumnBytes = 7;

quint64 readU64(const char *bytes)
{
    quint64 value = 0;
    for (int index = 0; index < 8; ++index) {
        value |= static_cast<quint64>(static_cast<unsigned char>(bytes[index])) << (8 * index);
    }
    return value;
}

} // namespace

Waveform::Waveform(QObject *parent)
    : QObject(parent)
{
    m_palette[0] = QColor(QStringLiteral("#ff2f1f"));
    m_palette[1] = QColor(QStringLiteral("#35d13a"));
    m_palette[2] = QColor(QStringLiteral("#2f6bff"));
}

void Waveform::clear()
{
    m_columns.clear();
    m_colours.clear();
    m_framesPerBucket = 0;
    m_sampleRate = 0;
    m_source.clear();
    m_error.clear();
    emit changed();
}

bool Waveform::load(const QString &path)
{
    if (path.isEmpty()) {
        clear();
        return false;
    }
    if (path == m_source && !m_columns.isEmpty()) {
        return true; // already holding this one; a snapshot repeats the same reference
    }

    QFile file(path);
    if (!file.open(QIODevice::ReadOnly)) {
        m_error = QStringLiteral("cannot read %1").arg(path);
        emit changed();
        return false;
    }
    const QByteArray bytes = file.readAll();
    file.close();

    if (bytes.size() < kHeaderBytes || std::memcmp(bytes.constData(), "DJWAVE11", 8) != 0) {
        m_error = QStringLiteral("%1 is not a waveform file").arg(QFileInfo(path).fileName());
        emit changed();
        return false;
    }

    const quint64 sampleRate = readU64(bytes.constData() + 32);
    const quint64 framesPerBucket = readU64(bytes.constData() + 40);
    const quint64 count = readU64(bytes.constData() + 48);
    const quint64 available = static_cast<quint64>((bytes.size() - kHeaderBytes) / kColumnBytes);
    if (count == 0 || count > available || framesPerBucket == 0) {
        m_error = QStringLiteral("%1 is truncated: %2 columns announced, %3 present")
                      .arg(QFileInfo(path).fileName())
                      .arg(count)
                      .arg(available);
        emit changed();
        return false;
    }

    QVariantList columns;
    columns.reserve(static_cast<int>(count));
    const char *data = bytes.constData() + kHeaderBytes;
    for (quint64 index = 0; index < count; ++index) {
        const auto *column = reinterpret_cast<const unsigned char *>(data + index * kColumnBytes);
        const auto low = static_cast<qint16>(column[0] | (column[1] << 8));
        const auto high = static_cast<qint16>(column[2] | (column[3] << 8));
        // Band levels are stored with a square-root curve; undo it so the numbers are linear RMS.
        const auto square = [](unsigned char value) {
            const double normalised = static_cast<double>(value) / 255.0;
            return normalised * normalised;
        };
        columns.append(QVariantMap{{QStringLiteral("min"), static_cast<double>(low) / 32767.0},
                                   {QStringLiteral("max"), static_cast<double>(high) / 32767.0},
                                   {QStringLiteral("low"), square(column[4])},
                                   {QStringLiteral("mid"), square(column[5])},
                                   {QStringLiteral("high"), square(column[6])}});
    }

    m_columns = columns;
    m_framesPerBucket = static_cast<int>(framesPerBucket);
    m_sampleRate = static_cast<int>(sampleRate);
    m_source = path;
    m_error.clear();
    measure();
    emit changed();
    return true;
}

void Waveform::measure()
{
    // The reference algorithm normalises against the loudest block of the whole track, per channel.
    // The height of a bar is the peak of its block, so drum hits become needles instead of humps.
    double loudest = 0.0;
    for (const QVariant &entry : m_columns) {
        const QVariantMap column = entry.toMap();
        const double peak = qMax(qAbs(column.value(QStringLiteral("min")).toDouble()),
                                 qAbs(column.value(QStringLiteral("max")).toDouble()));
        loudest = qMax(loudest, peak);
    }
    m_maxAmplitude = qMax(loudest, 1e-6);
    // Typical level of each band across the track. Bass carries an order of magnitude more energy
    // than treble in any recording, and without this every bar lands on the red end of the scale no
    // matter what the music does. The perceptual weights of the specification then apply on top.
    std::array<std::vector<double>, 3> samples;
    for (const QVariant &entry : m_columns) {
        const QVariantMap column = entry.toMap();
        samples[0].push_back(column.value(QStringLiteral("low")).toDouble());
        samples[1].push_back(column.value(QStringLiteral("mid")).toDouble());
        samples[2].push_back(column.value(QStringLiteral("high")).toDouble());
    }
    for (int band = 0; band < 3; ++band) {
        std::sort(samples[band].begin(), samples[band].end());
        const double typical = samples[band].empty()
            ? 1e-6
            : samples[band][samples[band].size() / 2];
        m_bandTypical[band] = qMax(typical, 1e-6);
    }
    m_maxChannel[0] = m_maxChannel[1] = m_maxChannel[2] = 1.0;
    m_colours = columnColours();
}

QVariantList Waveform::profile(int columns, double windowStart, double span) const
{
    QVariantList bars;
    m_lastCovered = 0;
    if (columns <= 0 || m_columns.isEmpty() || span <= 0.0) {
        return bars;
    }
    const int total = m_columns.size();
    bars.reserve(columns);

    // Integer edges per block: the ranges tile the window by construction, so no column of the
    // envelope is counted twice and none is skipped.
    const int coveredFrom = qBound(0, int(std::floor(windowStart * total)), total);
    const int coveredTo = qBound(0, int(std::ceil((windowStart + span) * total)), total);
    m_lastCovered = qMax(0, coveredTo - coveredFrom);

    int previous = coveredFrom;
    for (int bar = 0; bar < columns; ++bar) {
        // A bar that lies wholly before the start or after the end of the track is not silence in
        // the track, it is outside it. It used to be clamped to the nearest column, so the empty
        // margins carried the edge column's colour and looked like a piece of another waveform.
        const double barStart = windowStart + span * double(bar) / double(columns);
        const double barEnd = windowStart + span * double(bar + 1) / double(columns);
        if (barEnd <= 0.0 || barStart >= 1.0) {
            bars.append(QVariantMap{{QStringLiteral("amplitude"), 0.0},
                                    {QStringLiteral("rgb"), QString()},
                                    {QStringLiteral("outside"), true}});
            continue;
        }
        const double edge = windowStart + span * double(bar + 1) / double(columns);
        int next = qBound(0, int(std::floor(edge * double(total))), total);
        if (next < previous) {
            next = previous;
        }
        int first = previous;
        int last = next - 1;
        if (last < first) {
            // Zoomed in further than one envelope column per bar: take the column under the middle
            // of this bar, not the first column of the window. The old fallback made every bar of a
            // zoomed view show the same column, so the whole strip carried one column's amplitude
            // and pulsed with the music as the playhead moved through it.
            const double middle = windowStart + span * (double(bar) + 0.5) / double(columns);
            first = qBound(0, int(std::floor(middle * double(total))), qMax(0, total - 1));
            last = first;
        }
        previous = next;

        // RMS of each band over the block: sqrt of the mean of the squares, as the reference
        // defines it. The bands are already RMS per envelope column, so their squares average.
        double energy[3] = {0.0, 0.0, 0.0};
        int seen = 0;
        for (int index = first; index <= last && index < total; ++index) {
            const QVariantMap column = m_columns.at(index).toMap();
            energy[0] += column.value(QStringLiteral("low")).toDouble()
                * column.value(QStringLiteral("low")).toDouble();
            energy[1] += column.value(QStringLiteral("mid")).toDouble()
                * column.value(QStringLiteral("mid")).toDouble();
            energy[2] += column.value(QStringLiteral("high")).toDouble()
                * column.value(QStringLiteral("high")).toDouble();
            ++seen;
        }
        const double divisor = seen > 0 ? seen : 1;
        double bands[3] = {0.0, 0.0, 0.0};
        for (int band = 0; band < 3; ++band) {
            bands[band] = std::sqrt(energy[band] / divisor) * m_bandGain[band];
        }
        // The colour of a bar is the average of the columns it covers, taken from the table that
        // was computed once at load. Deriving it here instead made it depend on the visible window:
        // as the waveform scrolled, every frame re-smoothed a different set of neighbours and the
        // whole strip shimmered in time with the music.
        double red = 0.0;
        double green = 0.0;
        double blue = 0.0;
        int counted = 0;
        for (int index = first; index <= last && index < int(m_colours.size()); ++index) {
            red += m_colours[std::size_t(index)].redF();
            green += m_colours[std::size_t(index)].greenF();
            blue += m_colours[std::size_t(index)].blueF();
            ++counted;
        }
        const QColor colour = counted > 0
            ? QColor::fromRgbF(red / counted, green / counted, blue / counted)
            : QColor(0, 0, 0);
        // Height: the peak of the block against the loudest peak in the track.
        double peak = 0.0;
        for (int index = first; index <= last && index < total; ++index) {
            const QVariantMap column = m_columns.at(index).toMap();
            peak = qMax(peak, qMax(qAbs(column.value(QStringLiteral("min")).toDouble()),
                                   qAbs(column.value(QStringLiteral("max")).toDouble())));
        }
        bars.append(QVariantMap{
            {QStringLiteral("amplitude"), qBound(0.0, peak / m_maxAmplitude, 1.0)},
            {QStringLiteral("rgb"), colour.name()},
            {QStringLiteral("outside"), false}});
    }
    return bars;
}

void Waveform::setPalette(const QColor &low, const QColor &mid, const QColor &high)
{
    m_palette[0] = low;
    m_palette[1] = mid;
    m_palette[2] = high;
}

void Waveform::setBandGain(double low, double mid, double high)
{
    m_bandGain[0] = low;
    m_bandGain[1] = mid;
    m_bandGain[2] = high;
}

std::vector<QColor> Waveform::columnColours() const
{
    // One colour per envelope column, from that column's own band balance. Nothing here depends on
    // the view, so scrolling cannot change a colour that has already been drawn.
    static const double perception[3] = {1.0, 1.5, 2.5};
    std::vector<QColor> colours;
    colours.reserve(std::size_t(m_columns.size()));
    for (const QVariant &entry : m_columns) {
        const QVariantMap column = entry.toMap();
        const double bands[3] = {column.value(QStringLiteral("low")).toDouble(),
                                 column.value(QStringLiteral("mid")).toDouble(),
                                 column.value(QStringLiteral("high")).toDouble()};
        double raw[3] = {0.0, 0.0, 0.0};
        for (int band = 0; band < 3; ++band) {
            // Against the square root of the band's typical level: full equalisation paints a
            // bass-heavy track blue, none at all paints everything red.
            const double balanced = bands[band] / std::sqrt(qMax(m_bandTypical[band], 1e-6));
            raw[band] = std::pow(balanced * perception[band] * m_bandGain[band], 1.8);
        }
        const double strongest = qMax(raw[0], qMax(raw[1], raw[2]));
        if (strongest <= 1e-12) {
            colours.push_back(QColor(0, 0, 0));
            continue;
        }
        const QColor mixed = QColor::fromRgbF(qBound(0.0, raw[0] / strongest, 1.0),
                                              qBound(0.0, raw[1] / strongest, 1.0),
                                              qBound(0.0, raw[2] / strongest, 1.0));
        colours.push_back(QColor::fromHsvF(mixed.hsvHueF(), 1.0, 1.0));
    }

    // Blend across neighbours once, over the whole track: a colour boundary spans about twenty
    // pixels in the reference, and doing it here keeps that independent of scrolling.
    constexpr int radius = 12;
    std::vector<QColor> smoothed(colours.size());
    for (int index = 0; index < int(colours.size()); ++index) {
        double red = 0.0;
        double green = 0.0;
        double blue = 0.0;
        double weight = 0.0;
        for (int step = -radius; step <= radius; ++step) {
            const int neighbour = index + step;
            if (neighbour < 0 || neighbour >= int(colours.size())) {
                continue;
            }
            const double kernel = std::exp(-0.5 * (step / 5.0) * (step / 5.0));
            red += colours[std::size_t(neighbour)].redF() * kernel;
            green += colours[std::size_t(neighbour)].greenF() * kernel;
            blue += colours[std::size_t(neighbour)].blueF() * kernel;
            weight += kernel;
        }
        smoothed[std::size_t(index)] = weight > 0.0
            ? QColor::fromRgbF(red / weight, green / weight, blue / weight)
            : QColor(0, 0, 0);
    }
    return smoothed;
}
