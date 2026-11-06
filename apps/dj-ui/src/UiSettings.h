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
    Q_PROPERTY(QString themeBackground READ themeBackground CONSTANT)
    Q_PROPERTY(QString themeSurface READ themeSurface CONSTANT)
    Q_PROPERTY(QString themePanel READ themePanel CONSTANT)
    Q_PROPERTY(QString themeRaised READ themeRaised CONSTANT)
    Q_PROPERTY(QString themeBorder READ themeBorder CONSTANT)
    Q_PROPERTY(QString themeText READ themeText CONSTANT)
    Q_PROPERTY(QString themeTextDim READ themeTextDim CONSTANT)
    Q_PROPERTY(QString themeAccent READ themeAccent CONSTANT)
    Q_PROPERTY(QString themeAccentDim READ themeAccentDim CONSTANT)
    Q_PROPERTY(QString themeWarning READ themeWarning CONSTANT)
    Q_PROPERTY(QString themeDanger READ themeDanger CONSTANT)
    Q_PROPERTY(QString themeMarker READ themeMarker CONSTANT)
    Q_PROPERTY(QString themePlayhead READ themePlayhead CONSTANT)
    Q_PROPERTY(QString themeWaveBackground READ themeWaveBackground CONSTANT)
    Q_PROPERTY(QString themeWaveOutline READ themeWaveOutline CONSTANT)
    Q_PROPERTY(QString waveformStyle READ waveformStyle NOTIFY changed)
    // Frames drawn per second; the strip and the playhead projection run at this rate.
    Q_PROPERTY(int frameHz READ frameHz NOTIFY changed)
    // Milliseconds between frames, ready for a Timer interval: QML should not divide on its own.
    Q_PROPERTY(int frameInterval READ frameInterval NOTIFY changed)
    // Zoom the strip starts at, as a factor: 10 means the visible window is a tenth of the track.
    Q_PROPERTY(double waveformZoom READ waveformZoom NOTIFY changed)
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

    // The interface palette, measured from the owner's TRAKTOR PRO 4 screenshots: almost entirely
    // neutral dark greys, soft grey text instead of white, one blue accent. That is what working in
    // the dark needs — no glare, nothing saturated competing with the waveforms. Numbers, not
    // assets: ADR-001 forbids their graphics, and a measured colour is not a graphic.
    QString themeBackground() const { return QStringLiteral("#101010"); }
    QString themeSurface() const { return QStringLiteral("#101010"); }
    QString themePanel() const { return QStringLiteral("#181818"); }
    QString themeRaised() const { return QStringLiteral("#282828"); }
    QString themeBorder() const { return QStringLiteral("#383838"); }
    QString themeText() const { return QStringLiteral("#c8c8c8"); }
    QString themeTextDim() const { return QStringLiteral("#8a8a8a"); }
    QString themeAccent() const { return QStringLiteral("#5090f0"); }
    QString themeAccentDim() const { return QStringLiteral("#204070"); }
    QString themeWarning() const { return QStringLiteral("#e0a06a"); }
    QString themeDanger() const { return QStringLiteral("#e06a5a"); }
    QString themeMarker() const { return QStringLiteral("#e2c07f"); }
    QString themePlayhead() const { return QStringLiteral("#f0f3f6"); }
    QString themeWaveBackground() const { return QStringLiteral("#141414"); }
    QString themeWaveOutline() const { return QStringLiteral("#5b636d"); }

    QString waveformStyle() const { return m_waveformStyle; }
    int frameHz() const { return m_frameHz; }
    double waveformZoom() const { return m_waveformZoom; }
    int frameInterval() const { return qMax(1, 1000 / qMax(1, m_frameHz)); }
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

    // Frames per second, 5..240; false when the number is outside what a display can use.
    bool setFrameHz(int hz);

    // Default zoom, 1..100; false when outside that.
    bool setWaveformZoom(double factor);

    // "coloured" or "plain"; anything else is refused.
    bool setWaveformStyle(const QString &style);

    // Set from the command line; an invalid colour is refused and the default kept.
    bool setWaveformOutsideColor(const QString &color);

signals:
    void changed();

private:
    QString m_waveformOutsideColor = QStringLiteral("#101010");
    QString m_waveformStyle = QStringLiteral("coloured");
    // Twenty frames a second: the owner's default, and enough with the projection between snapshots.
    int m_frameHz = 20;
    // Ten times: the whole track on the overview strip, a tenth of it under the playhead.
    double m_waveformZoom = 10.0;
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
