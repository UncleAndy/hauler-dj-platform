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

public:
    explicit UiSettings(QObject *parent = nullptr);

    QString waveformOutsideColor() const { return m_waveformOutsideColor; }
    QString waveformStyle() const { return m_waveformStyle; }

    // "coloured" or "plain"; anything else is refused.
    bool setWaveformStyle(const QString &style);

    // Set from the command line; an invalid colour is refused and the default kept.
    bool setWaveformOutsideColor(const QString &color);

signals:
    void changed();

private:
    QString m_waveformOutsideColor = QStringLiteral("#101317");
    QString m_waveformStyle = QStringLiteral("coloured");
};
