#include "Waveform.h"

#include <QFile>
#include <QFileInfo>
#include <QMap>

#include <cmath>
#include <cstring>

namespace {

// Mirrors the header written by crates/dj-analysis: magic, fingerprint, timebase, columns.
constexpr int kHeaderBytes = 56;
// Two extremes as i16 and two band levels as u8, per column.
constexpr int kColumnBytes = 6;

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

    if (bytes.size() < kHeaderBytes || std::memcmp(bytes.constData(), "DJWAVE02", 8) != 0) {
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
        // Bands are stored with a square-root curve, so undo it here: linear levels are what a
        // colour ramp expects.
        const double bass = (column[4] / 255.0) * (column[4] / 255.0);
        const double treble = (column[5] / 255.0) * (column[5] / 255.0);
        columns.append(QVariantMap{{QStringLiteral("min"), static_cast<double>(low) / 32767.0},
                                   {QStringLiteral("max"), static_cast<double>(high) / 32767.0},
                                   {QStringLiteral("low"), bass},
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
        double treble = 0.0;
        int seen = 0;
        for (int index = first; index <= last && index < total; ++index) {
            const QVariantMap column = m_columns.at(index).toMap();
            low = qMin(low, column.value(QStringLiteral("min")).toDouble());
            high = qMax(high, column.value(QStringLiteral("max")).toDouble());
            bass += column.value(QStringLiteral("low")).toDouble();
            treble += column.value(QStringLiteral("high")).toDouble();
            ++seen;
        }
        const double divisor = seen > 0 ? seen : 1;
        pixels.append(QVariantMap{{QStringLiteral("min"), low},
                                  {QStringLiteral("max"), high},
                                  {QStringLiteral("low"), bass / divisor},
                                  {QStringLiteral("high"), treble / divisor}});
    }
    return pixels;
}
