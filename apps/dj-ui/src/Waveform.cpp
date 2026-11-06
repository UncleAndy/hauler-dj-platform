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
    std::vector<Column> raw;
    raw.reserve(static_cast<std::size_t>(count));
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
        raw.push_back(Column{static_cast<float>(low) / 32767.0F,
                             static_cast<float>(high) / 32767.0F,
                             static_cast<float>(square(column[4])),
                             static_cast<float>(square(column[5])),
                             static_cast<float>(square(column[6]))});
        columns.append(QVariantMap{{QStringLiteral("min"), static_cast<double>(low) / 32767.0},
                                   {QStringLiteral("max"), static_cast<double>(high) / 32767.0},
                                   {QStringLiteral("low"), square(column[4])},
                                   {QStringLiteral("mid"), square(column[5])},
                                   {QStringLiteral("high"), square(column[6])}});
    }

    m_columns = columns;
    m_raw = std::move(raw);
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
    for (const Column &entry : m_raw) {
        const double peak = qMax(qAbs(double(entry.minimum)), qAbs(double(entry.maximum)));
        loudest = qMax(loudest, peak);
    }
    m_maxAmplitude = qMax(loudest, 1e-6);
    // Typical level of each band across the track. Bass carries an order of magnitude more energy
    // than treble in any recording, and without this every bar lands on the red end of the scale no
    // matter what the music does. The perceptual weights of the specification then apply on top.
    std::array<std::vector<double>, 3> samples;
    for (const Column &entry : m_raw) {
        samples[0].push_back(entry.low);
        samples[1].push_back(entry.mid);
        samples[2].push_back(entry.high);
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
    m_peakRamps = peakRamps();
    m_rampLoudest.clear();
    m_rampLoudest.reserve(m_peakRamps.size());
    for (const std::vector<double> &ramp : m_peakRamps) {
        double loudest = 0.0;
        for (const double value : ramp) {
            loudest = qMax(loudest, value);
        }
        m_rampLoudest.push_back(loudest);
    }
}

void Waveform::loadSynthetic(int columns)
{
    const int count = qMax(1, columns);
    m_columns.clear();
    m_raw.clear();
    m_columns.reserve(count);
    m_raw.reserve(std::size_t(count));
    for (int index = 0; index < count; ++index) {
        // A slow swell with a beat on top: the same mixture of near and far detail a track has.
        const double phase = double(index) / double(count);
        const double swell = 0.55 + 0.35 * std::sin(phase * 6.2831853 * 3.0);
        const double beat = 0.15 * std::sin(phase * 6.2831853 * 240.0);
        const auto level = float(qBound(0.0, swell + beat, 1.0));
        const auto band = [&](double weight) {
            return float(qBound(0.0, double(level) * weight, 1.0));
        };
        m_raw.push_back(Column{level, level, band(0.9), band(0.5), band(0.25)});
    }
    m_framesPerBucket = 128;
    m_sampleRate = 48000;
    m_source = QStringLiteral("synthetic");
    m_error.clear();
    measure();
    emit changed();
}

std::vector<Waveform::Bar> Waveform::bars(int columns, double windowStart, double span) const
{
    std::vector<Bar> result;
    m_lastCovered = 0;
    if (columns <= 0 || m_raw.empty() || span <= 0.0) {
        return result;
    }
    const int total = int(m_raw.size());
    result.reserve(std::size_t(columns));

    // Integer edges per block: the ranges tile the window by construction, so no column of the
    // envelope is counted twice and none is skipped.
    const int coveredFrom = qBound(0, int(std::floor(windowStart * total)), total);
    const int coveredTo = qBound(0, int(std::ceil((windowStart + span) * total)), total);
    m_lastCovered = qMax(0, coveredTo - coveredFrom);
    const double columnsPerBar = double(m_lastCovered) / double(columns);

    // How much of the ramp to mix in, by how wide the window is. Zoomed out, a bar covers a long
    // slice and the outline needs ironing or it grains; zoomed in, a bar is a few milliseconds and
    // ironing would flatten the hits. Between the two the ramp fades in with the window, so every
    // step of the wheel changes the amount of smoothing instead of flipping it on at one zoom.
    // Never zero. At the deepest zoom a little blend is what keeps the strip from turning into a
    // comb of repeated envelope columns; at the whole track it is full.
    const double zoom = span > 0.0 ? 1.0 / span : 1.0;
    const double mediumWeight = qBound(0.5, 0.5 + 0.5 * (span - 0.05) / 0.85, 1.0);
    const double smoothWeight = qMin(1.0, mediumWeight + 0.25 * (smoothingHeat(zoom) - 1.0));
    const int level = rampForZoom(columnsPerBar, zoom);
    const double *ramp = (!m_peakRamps.empty() && smoothWeight > 0.0)
        ? m_peakRamps.at(std::size_t(level)).data()
        : nullptr;
    // The scale of this zoom level: a smoothed outline is shorter than the raw peaks, so it is
    // normalised against its own loudest value — a constant of the level, not of the window.
    const double rampScale = (level < int(m_rampLoudest.size()) && m_rampLoudest[std::size_t(level)] > 0.0)
        ? m_rampLoudest[std::size_t(level)]
        : m_maxAmplitude;
    const double scale = m_maxAmplitude > 0.0
        ? (1.0 - smoothWeight) * m_maxAmplitude + smoothWeight * rampScale
        : 1.0;


    int previous = coveredFrom;
    for (int bar = 0; bar < columns; ++bar) {
        // A bar that lies wholly before the start or after the end of the track is not silence in
        // the track, it is outside it: it gets its own colour instead of the edge column's.
        const double barStart = windowStart + span * double(bar) / double(columns);
        const double barEnd = windowStart + span * double(bar + 1) / double(columns);
        if (barEnd <= 0.0 || barStart >= 1.0) {
            Bar empty;
            empty.outside = true;
            result.push_back(empty);
            continue;
        }
        int next = qBound(0, int(std::floor(barEnd * total)), total);
        if (next < previous) {
            next = previous;
        }
        int first = previous;
        int last = next - 1;
        if (last < first) {
            // Zoomed in further than one envelope column per bar: take the column under the middle
            // of this bar, not the first column of the window.
            const double middle = windowStart + span * (double(bar) + 0.5) / double(columns);
            first = qBound(0, int(std::floor(middle * total)), qMax(0, total - 1));
            last = first;
        }
        previous = next;

        // The colour of a bar is the average of the columns it covers, taken from the table that was
        // computed once at load: deriving it here would make it depend on the visible window, and the
        // strip would shimmer as it scrolled.
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

        // Height: the average of the pre-smoothed peaks over the columns this bar covers. A raw peak
        // picks whichever column happened to be loudest, so with several columns per bar the outline
        // jumps from bar to bar — that is the moiré. The smoothing is prepared once for the whole
        // track, per zoom level, so it cannot depend on where the window sits.
        double height = 0.0;
        if (m_maxAmplitude > 0.0) {
            // Between the whole track and a real zoom, a bar still spans several envelope columns.
            // Taking the loudest of them is what produced the beat pattern: the bar grid and the
            // column grid drift against each other and the winner jumps from bar to bar. Averaging the
            // columns the bar owns is the honest downsample — the bar shows its own slice, not the
            // luckiest column in it. Only when the bar is narrower than a column is there nothing to
            // average and the single column is used.
            double sum = 0.0;
            int seen = 0;
            for (int index = first; index <= last && index < total; ++index) {
                const double peak = qMax(qAbs(double(m_raw[std::size_t(index)].minimum)),
                                         qAbs(double(m_raw[std::size_t(index)].maximum)));
                sum += peak;
                ++seen;
            }
            height = seen > 0 ? (sum / seen) / scale : 0.0;
            if (ramp != nullptr && smoothWeight > 0.0 && seen > 0) {
                double rampSum = 0.0;
                int rampSeen = 0;
                for (int index = first; index <= last && index < total; ++index) {
                    rampSum += ramp[index];
                    ++rampSeen;
                }
                if (rampSeen > 0) {
                    height = (1.0 - smoothWeight) * (sum / seen)
                             + smoothWeight * (rampSum / rampSeen);
                }
            }
        }

        Bar entry;
        entry.amplitude = float(qBound(0.0, height, 1.0));
        entry.colour = colour.rgb();
        result.push_back(entry);
    }
    return result;
}

QVariantList Waveform::profile(int columns, double windowStart, double span) const
{
    // The QML and the offline renderer read this shape; the live strip uses `bars` and never pays
    // for a QVariantMap per bar.
    QVariantList list;
    const std::vector<Bar> built = bars(columns, windowStart, span);
    list.reserve(int(built.size()));
    for (const Bar &bar : built) {
        list.append(QVariantMap{
            {QStringLiteral("amplitude"), double(bar.amplitude)},
            {QStringLiteral("rgb"), bar.outside ? QString() : QColor(bar.colour).name()},
            {QStringLiteral("outside"), bar.outside}});
    }
    return list;
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
    for (const Column &entry : m_raw) {
        const double bands[3] = {entry.low, entry.mid, entry.high};
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

std::vector<std::vector<double>> Waveform::peakRamps() const
{
    // Peak per column, then the same ramp smoothed with a few fixed radii. A fixed set means the
    // chosen radius depends only on the zoom, never on the scroll position: a radius computed from
    // "how many columns this bar happens to cover" changes as the view moves and makes the whole
    // strip pump.
    std::vector<double> raw;
    raw.reserve(std::size_t(m_columns.size()));
    for (const Column &entry : m_raw) {
        raw.push_back(qMax(qAbs(double(entry.minimum)), qAbs(double(entry.maximum))));
    }

    // Radius 1 is not "no smoothing": it is the smallest blend, and at the deepest zoom it is what
    // turns the envelope's stair steps into a slope instead of leaving the eye a comb.
    static const int radii[] = {1, 4, 8, 16, 32, 64};
    std::vector<std::vector<double>> ramps;
    ramps.reserve(sizeof(radii) / sizeof(radii[0]));
    for (const int radius : radii) {
        std::vector<double> ramp(raw.size(), 0.0);
        for (int index = 0; index < int(raw.size()); ++index) {
            if (radius == 0) {
                ramp[std::size_t(index)] = raw[std::size_t(index)];
                continue;
            }
            double sum = 0.0;
            double weight = 0.0;
            for (int step = -radius; step <= radius; ++step) {
                const int neighbour = index + step;
                if (neighbour < 0 || neighbour >= int(raw.size())) {
                    continue;
                }
                const double kernel = std::exp(-0.5 * (step / (radius * 0.5)) * (step / (radius * 0.5)));
                sum += raw[std::size_t(neighbour)] * kernel;
                weight += kernel;
            }
            ramp[std::size_t(index)] = weight > 0.0 ? sum / weight : raw[std::size_t(index)];
        }
        ramps.push_back(std::move(ramp));
    }
    return ramps;
}

int Waveform::rampForZoom(double columnsPerBar, double zoom) const
{
    // Up to fifteen times the radius is the medium one worked out earlier: eight bars' worth of
    // columns, so a bar reads the trend around it instead of the jagged edge of its own few columns.
    // Past fifteen the strip is showing fewer and fewer envelope columns per bar — the same column
    // repeats across several bars and reads as a comb — so the smoothing grows with the zoom instead
    // of shrinking with the bar.
    static const int radii[] = {1, 4, 8, 16, 32, 64};
    const double wanted = qMax(1.0, columnsPerBar * 8.0 * smoothingHeat(zoom));
    for (int index = 0; index < int(sizeof(radii) / sizeof(radii[0])); ++index) {
        if (radii[index] >= wanted) {
            return index;
        }
    }
    return int(sizeof(radii) / sizeof(radii[0])) - 1;
}

double Waveform::smoothingHeat(double zoom)
{
    // How much stronger than the medium setting the smoothing is: one up to 15x, then rising with the
    // zoom towards a ceiling of 2.5. The rise saturates on purpose — past about forty times the strip
    // shows so little of the track that more ironing only makes the beat look soft. Measured against
    // the zoom, the curve gives 1.2 at 20x, 1.7 at 40x, 2.1 at 100x and 2.4 at 400x: the last decade
    // of zoom adds a tenth of what the first one did.
    if (!(zoom > 15.0)) {
        return 1.0;
    }
    const double past = zoom / 15.0 - 1.0;
    return 1.0 + 1.5 * past / (past + 2.0);
}
