#include "Waveform.h"

#include <QFile>
#include <QFileInfo>
#include <QVariantMap>

#include <cmath>
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
    double loudest = 0.0;
    double channels[3] = {0.0, 0.0, 0.0};
    for (const QVariant &entry : m_columns) {
        const QVariantMap column = entry.toMap();
        const double bands[3] = {column.value(QStringLiteral("low")).toDouble(),
                                 column.value(QStringLiteral("mid")).toDouble(),
                                 column.value(QStringLiteral("high")).toDouble()};
        const double amplitude = std::sqrt(bands[0] * bands[0] + bands[1] * bands[1]
                                           + bands[2] * bands[2]);
        loudest = qMax(loudest, amplitude);
        for (int band = 0; band < 3; ++band) {
            channels[band] = qMax(channels[band], bands[band]);
        }
    }
    m_maxAmplitude = qMax(loudest, 1e-6);
    for (int band = 0; band < 3; ++band) {
        m_maxChannel[band] = qMax(channels[band], 1e-6);
    }
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
        const double edge = windowStart + span * double(bar + 1) / double(columns);
        int next = qBound(0, int(std::floor(edge * double(total))), total);
        if (next < previous) {
            next = previous;
        }
        int first = previous;
        int last = next - 1;
        if (last < first) {
            first = qBound(0, coveredFrom, qMax(0, total - 1));
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
        const double amplitude = std::sqrt(bands[0] * bands[0] + bands[1] * bands[1]
                                           + bands[2] * bands[2]);

        // Height against the loudest block of the track, colour against each channel's own peak.
        double channels[3] = {0.0, 0.0, 0.0};
        for (int band = 0; band < 3; ++band) {
            channels[band] = qBound(0.0, bands[band] / m_maxChannel[band], 1.0);
        }
        double red = 0.0;
        double green = 0.0;
        double blue = 0.0;
        for (int band = 0; band < 3; ++band) {
            red += m_palette[band].redF() * channels[band];
            green += m_palette[band].greenF() * channels[band];
            blue += m_palette[band].blueF() * channels[band];
        }
        const QColor colour = QColor::fromRgbF(qBound(0.0, red, 1.0), qBound(0.0, green, 1.0),
                                               qBound(0.0, blue, 1.0));
        bars.append(QVariantMap{
            {QStringLiteral("amplitude"), qBound(0.0, amplitude / m_maxAmplitude, 1.0)},
            {QStringLiteral("rgb"), colour.name()}});
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
