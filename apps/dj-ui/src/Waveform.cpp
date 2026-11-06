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

    if (bytes.size() < kHeaderBytes || std::memcmp(bytes.constData(), "DJWAVE06", 8) != 0) {
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
    const double levels[3] = {qMax(0.0, loud), qMax(0.0, mid), qMax(0.0, high)};
    // Compare each band with its own usual level for this track, not with zero. A band that simply
    // spans more octaves is loud everywhere, so measuring against zero makes whichever band is
    // widest win the whole track: first the mids, then the bass, both times one flat colour.
    // What makes a kick red is that the bass is louder THAN ITSELF usually is.
    double deviations[3] = {0.0, 0.0, 0.0};
    double rawTotal = 0.0;
    for (int band = 0; band < 3; ++band) {
        rawTotal += levels[band];
    }
    if (rawTotal <= 1e-6) {
        return m_palette[1];
    }
    for (int band = 0; band < 3; ++band) {
        const double usual = qMax(m_bandReference[band], 1e-6);
        // Gain 1.0 keeps the measurement honest; the ceiling stops one huge hit from pinning a band.
        const double relative = levels[band] * m_bandGain[band] / usual;
        deviations[band] = qMax(0.0, qMin(8.0, relative - 1.0));
    }
    double leader = qMax(deviations[0], qMax(deviations[1], deviations[2]));
    if (leader <= 1e-6) {
        // Nothing stands out from the track's own average: fall back to the plain loudness mix so
        // the column still carries information instead of turning grey.
        for (int band = 0; band < 3; ++band) {
            deviations[band] = levels[band];
        }
        leader = qMax(deviations[0], qMax(deviations[1], deviations[2]));
    }
    double weights[3] = {0.0, 0.0, 0.0};
    double total = 0.0;
    for (int band = 0; band < 3; ++band) {
        // The leader contributes 1 — full brightness of its colour — the others in proportion to
        // how much they rise above their own usual level against the leader, as the owner asked.
        weights[band] = deviations[band] / leader;
        total += weights[band];
    }
    double red = 0.0;
    double green = 0.0;
    double blue = 0.0;
    for (int band = 0; band < 3; ++band) {
        const QColor &colour = m_palette[band];
        const double share = weights[band] / total;
        red += colour.redF() * share;
        green += colour.greenF() * share;
        blue += colour.blueF() * share;
    }
    return QColor::fromRgbF(qBound(0.0, red, 1.0), qBound(0.0, green, 1.0), qBound(0.0, blue, 1.0));
}

void Waveform::setBandGain(double low, double mid, double high)
{
    m_bandGain[0] = low;
    m_bandGain[1] = mid;
    m_bandGain[2] = high;
}
