#include "Waveform.h"

#include <QFile>
#include <QFileInfo>
#include <QMap>

#include <cmath>
#include <cstring>

namespace {

// Mirrors the header written by crates/dj-analysis: magic (8), size (8), modified (16, u128),
// sample rate (8), frames per bucket (8), column count (8), three band references (3), and three
// band levels (3).
constexpr int kHeaderBytes = 56 + 3 + 3;
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
}

void Waveform::clear()
{
    m_columns.clear();
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

    if (bytes.size() < kHeaderBytes || std::memcmp(bytes.constData(), "DJWAVE09", 8) != 0) {
        m_error = QStringLiteral("%1 is not a waveform file").arg(QFileInfo(path).fileName());
        emit changed();
        return false;
    }

    const quint64 sampleRate = readU64(bytes.constData() + 32);
    const quint64 framesPerBucket = readU64(bytes.constData() + 40);
    const quint64 count = readU64(bytes.constData() + 48);
    // Typical level of each band across the track, stored right after the count. The colour is
    // decided by how a column compares with these, not by raw energy: the mid band spans four
    // octaves and therefore holds the most energy in almost any music, which is why colouring by
    // raw levels painted the whole waveform the mid colour.
    for (int index = 0; index < 3; ++index) {
        const double stored = static_cast<uchar>(bytes.at(56 + index)) / 255.0;
        m_bandReference[index] = stored * stored;
        const double loud = static_cast<uchar>(bytes.at(59 + index)) / 255.0;
        m_bandLevel[index] = qMax(1e-6, loud * loud);
    }
    if (m_bandReference[0] <= 0.0 && m_bandReference[1] <= 0.0 && m_bandReference[2] <= 0.0) {
        m_error = QStringLiteral("%1 has no band references").arg(QFileInfo(path).fileName());
        emit changed();
        return false;
    }
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
        // Bands are stored with a square-root curve, so undo it here: linear levels are what a
        // colour ramp expects.
        const double bass = (column[4] / 255.0) * (column[4] / 255.0);
        const double mids = (column[5] / 255.0) * (column[5] / 255.0);
        const double treble = (column[6] / 255.0) * (column[6] / 255.0);
        columns.append(QVariantMap{{QStringLiteral("min"), static_cast<double>(low) / 32767.0},
                                   {QStringLiteral("max"), static_cast<double>(high) / 32767.0},
                                   {QStringLiteral("low"), bass},
                                   {QStringLiteral("mid"), mids},
                                   {QStringLiteral("high"), treble}});
    }

    double loudest = 0.0;
    for (const QVariant &entry : columns) {
        const QVariantMap column = entry.toMap();
        loudest = qMax(loudest, qMax(qAbs(column.value(QStringLiteral("min")).toDouble()),
                                     qAbs(column.value(QStringLiteral("max")).toDouble())));
    }
    m_loudest = qMax(loudest, 1e-6);
    m_columns = columns;
    m_framesPerBucket = static_cast<int>(framesPerBucket);
    m_sampleRate = static_cast<int>(sampleRate);
    m_source = path;
    m_error.clear();
    emit changed();
    return true;
}

QVariantList Waveform::profile(int width, double windowStart, double span) const
{
    QVariantList pixels;
    m_lastCovered = 0;
    if (width <= 0 || m_columns.isEmpty() || span <= 0.0) {
        return pixels;
    }
    const int total = m_columns.size();
    pixels.reserve(width);
    // Distinct buckets the window covers. Pixels can repeat a bucket when there are fewer buckets
    // than pixels, so counting per pixel would report the width instead of the envelope.
    const int coveredFrom = qBound(0, int(std::floor(windowStart * total)), total);
    const int coveredTo = qBound(0, int(std::ceil((windowStart + span) * total)), total);
    m_lastCovered = qMax(0, coveredTo - coveredFrom);

    // Integer bucket boundaries per pixel: pixel p owns [first, next), and next is the first of
    // pixel p+1, so the ranges tile by construction rather than by luck.
    int previous = qBound(0, int(std::floor(windowStart * total)), total);
    for (int x = 0; x < width; ++x) {
        const double edge = windowStart + span * double(x + 1) / double(width);
        int next = qBound(0, int(std::floor(edge * total)), total);
        if (next < previous) {
            next = previous;
        }
        int first = previous;
        int last = next - 1;
        if (last < first) {
            // Fewer buckets than pixels: repeat the nearest one instead of leaving a hole.
            first = qBound(0, previous, total - 1);
            last = first;
        }
        previous = next;

        double low = 0.0;
        double high = 0.0;
        double bass = 0.0;
        double mids = 0.0;
        double treble = 0.0;
        int seen = 0;
        for (int index = first; index <= last && index < total; ++index) {
            const QVariantMap column = m_columns.at(index).toMap();
            low = qMin(low, column.value(QStringLiteral("min")).toDouble());
            high = qMax(high, column.value(QStringLiteral("max")).toDouble());
            bass += column.value(QStringLiteral("low")).toDouble();
            mids += column.value(QStringLiteral("mid")).toDouble();
            treble += column.value(QStringLiteral("high")).toDouble();
            ++seen;
        }
        const double divisor = seen > 0 ? seen : 1;
        // Loudness of each band in this column, as the owner asked: the loudest one sets the hue
        // at full brightness and the other two tint it in proportion. Nothing is normalised
        // against the track, so the picture follows the music at this moment, not its average.
        const double heights[3] = {bass / divisor, mids / divisor, treble / divisor};
        const QColor colour = pixelColor(heights[0], heights[1], heights[2]);
        pixels.append(QVariantMap{{QStringLiteral("min"), low},
                                  {QStringLiteral("max"), high},
                                  {QStringLiteral("low"), bass / divisor},
                                  {QStringLiteral("mid"), mids / divisor},
                                  {QStringLiteral("high"), treble / divisor},
                                  {QStringLiteral("h0"), heights[0]},
                                  {QStringLiteral("h1"), heights[1]},
                                  {QStringLiteral("h2"), heights[2]},
                                  {QStringLiteral("rgb"), colour.name()}});
    }
    // A column whose bands are nearly equal has no dominant colour, and tiny measurement noise
    // would then flip it between red, green and violet from one pixel to the next. Averaging the
    // heights with the neighbours settles those columns and leaves real changes sharp.
    if (pixels.size() > 5) {
        QVariantList smoothed;
        smoothed.reserve(pixels.size());
        for (int x = 0; x < pixels.size(); ++x) {
            QVariantMap pixel = pixels.at(x).toMap();
            double heights[3] = {0.0, 0.0, 0.0};
            int weight = 0;
            for (int offset = -2; offset <= 2; ++offset) {
                const int neighbour = x + offset;
                if (neighbour < 0 || neighbour >= pixels.size()) {
                    continue;
                }
                const QVariantMap around = pixels.at(neighbour).toMap();
                const double kernel = 3 - std::abs(offset);
                heights[0] += around.value(QStringLiteral("h0")).toDouble() * kernel;
                heights[1] += around.value(QStringLiteral("h1")).toDouble() * kernel;
                heights[2] += around.value(QStringLiteral("h2")).toDouble() * kernel;
                weight += kernel;
            }
            if (weight > 0) {
                for (int band = 0; band < 3; ++band) {
                    pixel[QStringLiteral("h%1").arg(band)] = heights[band] / weight;
                }
            }
            smoothed.append(pixel);
        }
        return smoothed;
    }
    return pixels;
}

QVariantList Waveform::reference() const
{
    return {m_bandReference[0], m_bandReference[1], m_bandReference[2]};
}

void Waveform::setPalette(const QColor &low, const QColor &mid, const QColor &high)
{
    m_palette[0] = low;
    m_palette[1] = mid;
    m_palette[2] = high;
}

QColor Waveform::pixelColor(double loud, double mid, double high) const
{
    const double levels[3] = {qMax(0.0, loud) * m_bandGain[0], qMax(0.0, mid) * m_bandGain[1],
                              qMax(0.0, high) * m_bandGain[2]};
    // Each band against its own loud level in this track. Absolute loudness is not enough: treble
    // always carries a fraction of the bass energy, so the scale never reached the cool end and a
    // hi-hat could not look blue. Against its own peak, every band can fill its part of the axis.
    double relative[3] = {0.0, 0.0, 0.0};
    double total = 0.0;
    for (int band = 0; band < 3; ++band) {
        relative[band] = levels[band] / qMax(m_bandLevel[band], 1e-6);
        total += relative[band];
    }
    if (total <= 1e-6) {
        return QColor(0, 0, 0);
    }
    // Raised to 1.5 before normalising: without it the three shares stay close, every column lands
    // in the middle of the colour scale, and the track reads as one long orange-yellow-green smear.
    // With it the leading band pulls the hue to its own part of the circle without hiding the others.
    double shaped[3] = {0.0, 0.0, 0.0};
    double shapedTotal = 0.0;
    for (int band = 0; band < 3; ++band) {
        shaped[band] = std::pow(relative[band] / total, 1.5);
        shapedTotal += shaped[band];
    }
    const double share[3] = {shaped[0] / shapedTotal, shaped[1] / shapedTotal,
                             shaped[2] / shapedTotal};
    // Where this column sits on a log-frequency axis, using the centre of each band: bass around
    // 70 Hz, mids around 1 kHz, treble around 9 kHz, on the 20 Hz - 20 kHz span.
    static const double centre[3] = {0.140, 0.520, 0.900};
    const double position =
        qBound(0.0, share[0] * centre[0] + share[1] * centre[1] + share[2] * centre[2], 1.0);

    // One hue per column, running round the colour circle from red through yellow and green to blue
    // and violet — the way the reference waveform does it. Measured from the owner's screenshots:
    // a column there is a single colour from top to bottom, and neighbouring columns differ.
    struct Stop {
        double at;
        double hue;
    };
    // Calibrated against the reference screenshot, where bass is red and orange rather than yellow:
    // a kick lands near 0.27 of the axis, and that has to come out red-orange, not yellow.
    static const Stop stops[] = {{0.00, 3.0},   {0.30, 14.0}, {0.45, 50.0}, {0.60, 120.0},
                                 {0.75, 178.0}, {0.88, 232.0}, {1.00, 275.0}};
    double hue = stops[0].hue;
    for (std::size_t index = 1; index < sizeof(stops) / sizeof(stops[0]); ++index) {
        if (position <= stops[index].at) {
            const double span = stops[index].at - stops[index - 1].at;
            const double part = span > 0.0 ? (position - stops[index - 1].at) / span : 0.0;
            hue = stops[index - 1].hue + (stops[index].hue - stops[index - 1].hue) * part;
            break;
        }
        hue = stops[index].hue;
    }
    // Mixed spectrum with no clear winner washes out, as the reference describes; a column where one
    // band dominates stays saturated.
    const double strongest = qMax(share[0], qMax(share[1], share[2]));
    const double saturation = qBound(0.25, 0.35 + (strongest - 1.0 / 3.0) * 1.65, 1.0);
    return QColor::fromHsvF(hue / 360.0, saturation, 1.0);
}

void Waveform::setBandGain(double low, double mid, double high)
{
    m_bandGain[0] = low;
    m_bandGain[1] = mid;
    m_bandGain[2] = high;
}
