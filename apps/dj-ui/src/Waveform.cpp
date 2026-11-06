#include "Waveform.h"

#include <QFile>
#include <QFileInfo>
#include <QMap>

#include <cmath>
#include <cstring>

namespace {

// Mirrors the header written by crates/dj-analysis: magic (8), size (8), modified (16, u128),
// sample rate (8), frames per bucket (8), column count (8), and three band references (3).
constexpr int kHeaderBytes = 56 + 3;
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

    if (bytes.size() < kHeaderBytes || std::memcmp(bytes.constData(), "DJWAVE08", 8) != 0) {
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
    // The three bands drive the red, green and blue channels directly, each scaled by its own
    // loudest block in this track. That is the scheme the reference describes, and it is the only
    // one of the five I tried that cannot collapse: no band is chosen as "the" colour, so no band
    // can own the picture, and mixtures come out where music has them — bass plus mids is orange,
    // mids plus treble is cyan, everything at once is white.
    const double levels[3] = {qMax(0.0, loud) * m_bandGain[0], qMax(0.0, mid) * m_bandGain[1],
                              qMax(0.0, high) * m_bandGain[2]};
    // Against each band's own usual level in this track, so a band that spans more octaves cannot
    // own a channel by being loud everywhere.
    double relative[3] = {0.0, 0.0, 0.0};
    for (int band = 0; band < 3; ++band) {
        relative[band] = levels[band] / qMax(m_bandReference[band], 1e-6);
    }
    // The strongest band fills its channel completely and the others follow in proportion, so a
    // kick is red with an orange edge, a hi-hat is blue, and dense material — where all three rise
    // together — comes out pale, as the reference says it should.
    const double strongest = qMax(relative[0], qMax(relative[1], relative[2]));
    if (strongest <= 1e-6) {
        return QColor(0, 0, 0);
    }
    // Each band adds its own colour in proportion to its share, so bass plus mids mixes into
    // orange, mids plus highs into cyan, and a passage where all three are up turns pale — the
    // additive channels of the reference, with the three colours still configurable.
    double red = 0.0;
    double green = 0.0;
    double blue = 0.0;
    for (int band = 0; band < 3; ++band) {
        const double share = qBound(0.0, relative[band] / strongest, 1.0);
        red += m_palette[band].redF() * share;
        green += m_palette[band].greenF() * share;
        blue += m_palette[band].blueF() * share;
    }
    return QColor::fromRgbF(qBound(0.0, red, 1.0), qBound(0.0, green, 1.0), qBound(0.0, blue, 1.0));
}

void Waveform::setBandGain(double low, double mid, double high)
{
    m_bandGain[0] = low;
    m_bandGain[1] = mid;
    m_bandGain[2] = high;
}
