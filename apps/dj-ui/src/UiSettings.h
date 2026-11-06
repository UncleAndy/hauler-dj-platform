// UI-only settings: things the shell decides and the engine has no opinion about.
//
// Kept separate from crates/dj-config on purpose: that file describes the audio engine (block size,
// cache policy, thresholds) and is read where the callback lives. Colours belong to the shell.
#pragma once

#include <QObject>
#include <QString>

class UiSettings : public QObject
{
    Q_OBJECT
    // Colour of the regions before the start and after the end of a track on the waveform strip.
    // Defaults to the strip's own background, so the edges look like empty space, not a warning.
    Q_PROPERTY(QString waveformOutsideColor READ waveformOutsideColor NOTIFY changed)
    // How the strip is drawn: "coloured" paints bass warm and treble cold, "plain" uses one colour.
    Q_PROPERTY(QString waveformStyle READ waveformStyle NOTIFY changed)
    // Bar geometry of the spectral waveform. A gap makes the picture read as a bar chart; the
    // waveform in a deck is continuous, so the default gap is zero.
    Q_PROPERTY(int waveformBarWidth READ waveformBarWidth NOTIFY changed)
    Q_PROPERTY(int waveformBarGap READ waveformBarGap NOTIFY changed)
    // Colours of the three bands; the mix between neighbours is interpolated per column.
    Q_PROPERTY(QString waveformLowColor READ waveformLowColor NOTIFY changed)
    Q_PROPERTY(QString waveformMidColor READ waveformMidColor NOTIFY changed)
    Q_PROPERTY(QString waveformHighColor READ waveformHighColor NOTIFY changed)

public:
    explicit UiSettings(QObject *parent = nullptr);

    QString waveformOutsideColor() const { return m_waveformOutsideColor; }
    QString waveformStyle() const { return m_waveformStyle; }
    int waveformBarWidth() const { return m_waveformBarWidth; }
    int waveformBarGap() const { return m_waveformBarGap; }

    // Both must stay positive; a zero width would leave nothing to draw.
    bool setWaveformBarWidth(int width);
    bool setWaveformBarGap(int gap);
    QString waveformLowColor() const { return m_waveformLowColor; }
    QString waveformMidColor() const { return m_waveformMidColor; }
    QString waveformHighColor() const { return m_waveformHighColor; }

    // Sets one of "low", "mid", "high"; false when the band name or the colour is wrong.
    bool setWaveformBandColor(const QString &band, const QString &color);

    // "coloured" or "plain"; anything else is refused.
    bool setWaveformStyle(const QString &style);

    // Set from the command line; an invalid colour is refused and the default kept.
    bool setWaveformOutsideColor(const QString &color);

signals:
    void changed();

private:
    QString m_waveformOutsideColor = QStringLiteral("#101317");
    QString m_waveformStyle = QStringLiteral("coloured");
    // One bar per pixel: the waveform is a continuous curve, and two-pixel bars halve the
    // horizontal detail for nothing.
    int m_waveformBarWidth = 1;
    int m_waveformBarGap = 0;
    // Red, green and blue channels: bass drives red, mids green, highs blue. Mixed additively, so
    // bass with mids reads orange and all three at once read pale.
    QString m_waveformLowColor = QStringLiteral("#ff2f1f");
    QString m_waveformMidColor = QStringLiteral("#35d13a");
    QString m_waveformHighColor = QStringLiteral("#2f6bff");
};
