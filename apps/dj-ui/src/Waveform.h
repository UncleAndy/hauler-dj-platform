// Reads the cached waveform envelope and turns it into the bars the deck draws (A07, DJP-MEDIA-003).
//
// The envelope is written by the engine (`DJWAVE11`): a fingerprint, the timebase in source frames,
// and per column the min/max of the mix plus the RMS of three frequency bands. Everything about
// colour and height is decided here, from the whole track, exactly as the reference algorithm
// describes: three bands drive R, G and B, and each channel is normalised by its own maximum.

#pragma once

#include <QColor>
#include <QObject>
#include <QString>
#include <QVariantList>
#include <vector>

class Waveform : public QObject
{
    Q_OBJECT
    // Columns of the envelope, oldest first; each is {min, max, low, mid, high}.
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

    // One entry per bar for the visible window: {amplitude, rgb}.
    //
    // `columns` is the number of bars the caller can fit. The window is split into that many equal
    // time blocks, which is the segmentation step of the reference algorithm; every block gets the
    // RMS of each band, its amplitude, and the colour those three numbers produce.
    Q_INVOKABLE QVariantList profile(int columns, double windowStart, double span) const;

    // Envelope columns the last `profile` call consumed; equals `buckets` for the whole track.
    Q_INVOKABLE int lastCoveredBuckets() const { return m_lastCovered; }

    // Colors of the three channels: bass, mids, treble.
    void setPalette(const QColor &low, const QColor &mid, const QColor &high);

    // Trims for the three channels before normalisation, all 1.0 by default.
    void setBandGain(double low, double mid, double high);

signals:
    void changed();

private:
    // Recompute the whole-track maxima the normalisation needs, and the colour table.
    void measure();

    // One colour per envelope column, computed once; smoothing included. Independent of the view,
    // so scrolling cannot make the strip shimmer.
    std::vector<QColor> columnColours() const;

    QVariantList m_columns;
    int m_framesPerBucket = 0;
    int m_sampleRate = 0;
    QString m_source;
    QString m_error;
    mutable int m_lastCovered = 0;
    QColor m_palette[3];
    double m_bandGain[3] = {1.0, 1.0, 1.0};
    double m_maxAmplitude = 1.0;
    double m_maxChannel[3] = {1.0, 1.0, 1.0};
    // Median level of each band in this track: what the colour balances against.
    double m_bandTypical[3] = {1.0, 1.0, 1.0};
    std::vector<QColor> m_colours;
};
