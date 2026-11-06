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
    // Colours of the three bands; the mix between neighbours is interpolated per column.
    Q_PROPERTY(QString waveformLowColor READ waveformLowColor NOTIFY changed)
    Q_PROPERTY(QString waveformMidColor READ waveformMidColor NOTIFY changed)
    Q_PROPERTY(QString waveformHighColor READ waveformHighColor NOTIFY changed)

public:
    explicit UiSettings(QObject *parent = nullptr);

    QString waveformOutsideColor() const { return m_waveformOutsideColor; }
    QString waveformStyle() const { return m_waveformStyle; }
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
    // Values measured from the owner's TRAKTOR PRO 4 screenshots (median hue of the strip): bass
    // dark red, mids green, highs violet-blue, lifted slightly so they read on our darker strip.
    // Numbers, not assets — ADR-001 forbids taking graphics, and a colour is not a graphic.
    QString m_waveformLowColor = QStringLiteral("#c33a26");
    QString m_waveformMidColor = QStringLiteral("#7aca4c");
    QString m_waveformHighColor = QStringLiteral("#9b6bd8");
};
