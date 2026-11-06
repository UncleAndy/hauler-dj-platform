// Reads the cached waveform envelope so QML can draw it (A07, DJP-MEDIA-003).
//
// The file is written by the engine (`DJWAVE01`): a fingerprint, the timebase in source frames and
// one min/max pair per column. Reading it here, rather than shipping columns in the snapshot, keeps
// twenty snapshots a second small and gives the UI a picture it can hold on to.

#pragma once

#include <QColor>
#include <QObject>
#include <QVariantList>
#include <QString>

class Waveform : public QObject
{
    Q_OBJECT
    // Columns of the envelope, oldest first; each is {min, max} in [-1, 1].
    Q_PROPERTY(QVariantList columns READ columns NOTIFY changed)
    // Frames each column covers: the timebase the playhead is placed with.
    Q_PROPERTY(int framesPerBucket READ framesPerBucket NOTIFY changed)
    Q_PROPERTY(int buckets READ buckets NOTIFY changed)
    Q_PROPERTY(int sampleRate READ sampleRate NOTIFY changed)
    Q_PROPERTY(QString source READ source NOTIFY changed)
    Q_PROPERTY(QString error READ error NOTIFY changed)

public:
    explicit Waveform(QObject *parent = nullptr);

    QVariantList columns() const { return m_columns; }
    int framesPerBucket() const { return m_framesPerBucket; }
    int buckets() const { return m_columns.size(); }
    int sampleRate() const { return m_sampleRate; }
    QString source() const { return m_source; }
    QString error() const { return m_error; }

    // Load the envelope at `path`; an unreadable or foreign file leaves the previous one alone and
    // reports why.
    Q_INVOKABLE bool load(const QString &path);
    Q_INVOKABLE void clear();

    // Aggregate the envelope into exactly `width` screen columns for the visible window.
    //
    // Every pixel gets one contiguous range of buckets and the ranges tile the window without gaps
    // or overlaps. That is what removes the moiré: drawing per bucket with fractional x positions
    // made neighbouring strands land on the same pixel or skip one, and at some zoom levels the
    // pattern was regular enough to read as texture.
    Q_INVOKABLE QVariantList profile(int width, double windowStart, double span) const;

    // Buckets the last `profile` call consumed; equals `buckets` when the window covers the track.
    Q_INVOKABLE int lastCoveredBuckets() const { return m_lastCovered; }

    // Shares of the column that the three bands claim, in [0, 1]; what the renderer mixes with.
    Q_INVOKABLE QVariantList reference() const;

    // Colors of the three bands, brightest red for bass, yellow for mids, bright blue for highs.
    void setPalette(const QColor &low, const QColor &mid, const QColor &high);

    // The color of one column at its loudness share: the loudest band sets the hue at full
    // brightness, the other two tint it in proportion to how loud they are against that leader.
    QColor pixelColor(double loud, double mid, double high) const;

signals:
    void changed();

private:
    QVariantList m_columns;
    int m_framesPerBucket = 0;
    int m_sampleRate = 0;
    QString m_source;
    QString m_error;
    mutable int m_lastCovered = 0;
    // Typical level of low, mid and high across the loaded track, from the envelope header.
    double m_bandReference[3] = {0.0, 0.0, 0.0};
    QColor m_palette[3];
};
