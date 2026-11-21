// UI-only settings: things the shell decides and the engine has no opinion about.
//
// Kept separate from crates/dj-config on purpose: that file describes the audio engine (block size,
// cache policy, thresholds) and is read where the callback lives. Colours belong to the shell.
#pragma once

#include <QFontDatabase>
#include <QObject>
#include <QJsonObject>
#include <QString>
#include <QStringList>

#include "GainCurve.h"

class UiSettings : public QObject
{
    Q_OBJECT
    // Colour of the regions before the start and after the end of a track on the waveform strip.
    // Defaults to the strip's own background, so the edges look like empty space, not a warning.
    Q_PROPERTY(QString waveformOutsideColor READ waveformOutsideColor WRITE setWaveformOutsideColor NOTIFY changed)
    // How the strip is drawn: "coloured" paints bass warm and treble cold, "plain" uses one colour.
    // Фон окна и область под панелями.
    Q_PROPERTY(QString themeBackground READ themeBackground WRITE setThemeBackground NOTIFY changed)
    // Поверхность панелей.
    Q_PROPERTY(QString themeSurface READ themeSurface WRITE setThemeSurface NOTIFY changed)
    // Панель внутри поверхности.
    Q_PROPERTY(QString themePanel READ themePanel WRITE setThemePanel NOTIFY changed)
    // Поднятая поверхность: кнопки, поля.
    Q_PROPERTY(QString themeRaised READ themeRaised WRITE setThemeRaised NOTIFY changed)
    // Рамки и разделители.
    Q_PROPERTY(QString themeBorder READ themeBorder WRITE setThemeBorder NOTIFY changed)
    // Основной текст.
    Q_PROPERTY(QString themeText READ themeText WRITE setThemeText NOTIFY changed)
    // Text on the accent: near-black, because the accent is bright.
    // Текст на акценте: почти чёрный, потому что акцент яркий.
    Q_PROPERTY(QString themeTextOnAccent READ themeTextOnAccent WRITE setThemeTextOnAccent NOTIFY changed)
    // Второстепенный текст и подписи.
    Q_PROPERTY(QString themeTextDim READ themeTextDim WRITE setThemeTextDim NOTIFY changed)
    // The collection's column headers. The accent's own family, dimmed: a header has to read as a
    // header, and the column that decides the sort keeps the accent itself, so the two do not fight.
    // Заголовки колонок коллекции.
    Q_PROPERTY(QString themeHeaderText READ themeHeaderText WRITE setThemeHeaderText NOTIFY changed)
    // The system's own fixed-pitch family, resolved at run time. The browser and the collection are
    // tables of numbers and names, and a proportional face makes the columns wobble — but which font a
    // desktop calls monospace is the desktop's business, and naming one here would bundle a font and its
    // licence into every install.
    Q_PROPERTY(QString monoFamily READ monoFamily CONSTANT)
    // Акцент: выделение, активное.
    Q_PROPERTY(QString themeAccent READ themeAccent WRITE setThemeAccent NOTIFY changed)
    // Приглушённый акцент.
    Q_PROPERTY(QString themeAccentDim READ themeAccentDim WRITE setThemeAccentDim NOTIFY changed)
    // Предупреждение.
    Q_PROPERTY(QString themeWarning READ themeWarning WRITE setThemeWarning NOTIFY changed)
    // Ошибка и опасное действие.
    Q_PROPERTY(QString themeDanger READ themeDanger WRITE setThemeDanger NOTIFY changed)
    // Метр: запас.
    Q_PROPERTY(QString themeMeterGreen READ themeMeterGreen WRITE setThemeMeterGreen NOTIFY changed)
    // Метр: близко к пределу.
    Q_PROPERTY(QString themeMeterYellow READ themeMeterYellow WRITE setThemeMeterYellow NOTIFY changed)
    // Метр: клип.
    Q_PROPERTY(QString themeMeterRed READ themeMeterRed WRITE setThemeMeterRed NOTIFY changed)
    // Метки: кью, хоткью, петля.
    Q_PROPERTY(QString themeMarker READ themeMarker WRITE setThemeMarker NOTIFY changed)
    // Плейхед.
    Q_PROPERTY(QString themePlayhead READ themePlayhead WRITE setThemePlayhead NOTIFY changed)
    // Фон волновой полосы.
    Q_PROPERTY(QString themeWaveBackground READ themeWaveBackground WRITE setThemeWaveBackground NOTIFY changed)
    // Контур волновой полосы.
    Q_PROPERTY(QString themeWaveOutline READ themeWaveOutline WRITE setThemeWaveOutline NOTIFY changed)
    Q_PROPERTY(QString waveformStyle READ waveformStyle WRITE setWaveformStyle NOTIFY changed)
    // The two deck modes a DJ decides once for the set rather than per deck: snap (a loop point lands on
    // the grid) and quantize (the loop's end is whole beats after its start). The decks read these and
    // the flags still travel inside each command, so the engine protocol is untouched (owner's decision).
    Q_PROPERTY(bool snapGrid READ snapGrid WRITE setSnapGrid NOTIFY changed)
    Q_PROPERTY(bool quantizeGrid READ quantizeGrid WRITE setQuantizeGrid NOTIFY changed)
    // Whether a hand on the strip is heard: a scratch, or a silent seek. Silent by default — a booth is
    // not a place to discover that dragging a strip makes noise (owner's decision, 14.11: a setting, not
    // a button on every deck).
    Q_PROPERTY(bool scrubAudible READ scrubAudible WRITE setScrubAudible NOTIFY changed)
    // Frames drawn per second; the strip and the playhead projection run at this rate.
    Q_PROPERTY(int frameHz READ frameHz WRITE setFrameHz NOTIFY changed)
    // Milliseconds between frames, ready for a Timer interval: QML should not divide on its own.
    Q_PROPERTY(int frameInterval READ frameInterval NOTIFY changed)
    // Zoom the strip starts at, as a factor: 10 means the visible window is a tenth of the track.
    Q_PROPERTY(double waveformZoom READ waveformZoom WRITE setWaveformZoom NOTIFY changed)
    // Bar geometry of the spectral waveform. A gap makes the picture read as a bar chart; the
    // waveform in a deck is continuous, so the default gap is zero.
    Q_PROPERTY(int waveformBarWidth READ waveformBarWidth WRITE setWaveformBarWidth NOTIFY changed)
    Q_PROPERTY(int waveformBarGap READ waveformBarGap WRITE setWaveformBarGap NOTIFY changed)
    // Colours of the three bands; the mix between neighbours is interpolated per column.
    Q_PROPERTY(QString waveformLowColor READ waveformLowColor WRITE setWaveformLowColor NOTIFY changed)
    Q_PROPERTY(QString waveformMidColor READ waveformMidColor WRITE setWaveformMidColor NOTIFY changed)
    Q_PROPERTY(QString waveformHighColor READ waveformHighColor WRITE setWaveformHighColor NOTIFY changed)
    // Throw of the pitch fader, in percent either side of the file's own speed. A setting, not a
    // constant: 8 % is the trained-hands range for matching two tracks, 16 % is what a long blend or a
    // big tempo gap needs, and the owner asked for 16 as the default.
    Q_PROPERTY(double pitchRangePercent READ pitchRangePercent WRITE setPitchRangePercent NOTIFY changed)
    // The law of the level knobs: "linear" is straight through, "cubic" spends less travel around unity
    // and more near the ends. A setting because the right shape is a matter of the hand, not of maths.
    Q_PROPERTY(QString gainCurve READ gainCurve WRITE setGainCurve NOTIFY changed)
    // How much of the straight rate the knob keeps at unity: 1.0 is the straight law, 0.5 is half.
    Q_PROPERTY(double gainCurveMiddle READ gainCurveMiddle WRITE setGainCurveMiddle NOTIFY changed)
    // The faders have their own law and their own shape: a fader is ridden, and the top third of its
    // throw is where a mix happens, so it takes the logarithmic (B-taper) shape rather than a cubic.
    Q_PROPERTY(QString faderCurve READ faderCurve WRITE setFaderCurve NOTIFY changed)
    Q_PROPERTY(double faderCurveMiddle READ faderCurveMiddle WRITE setFaderCurveMiddle NOTIFY changed)

public:
    explicit UiSettings(QObject *parent = nullptr);

    QString waveformOutsideColor() const { return m_waveformOutsideColor; }

    // The palette. It starts as the measured TRAKTOR scheme: almost entirely neutral dark greys, soft grey text
    // instead of white, one blue accent — no glare, nothing saturated competing with the waveforms. Numbers, not
    // assets: ADR-001 forbids their graphics, and a measured colour is not a graphic. Every one of them can be
    // changed, and the change is written into layout.json, so a window that offers a theme is not offering a
    // picture of one.
    QString themeBackground() const { return m_themeBackground; }
    QString themeSurface() const { return m_themeSurface; }
    QString themePanel() const { return m_themePanel; }
    QString themeRaised() const { return m_themeRaised; }
    QString themeBorder() const { return m_themeBorder; }
    QString themeTextOnAccent() const { return m_themeTextOnAccent; }
    QString themeText() const { return m_themeText; }
    QString themeTextDim() const { return m_themeTextDim; }
    QString themeHeaderText() const { return m_themeHeaderText; }

    QString monoFamily() const
    {
        // The system's fixed-pitch font as the platform defines it: fontconfig's `monospace` alias on
        // this desktop, Consolas or Menlo where those are the ones a system calls fixed pitch.
        const QFont fixed = QFontDatabase::systemFont(QFontDatabase::FixedFont);
        return fixed.family().isEmpty() ? QStringLiteral("monospace") : fixed.family();
    }
    QString themeAccent() const { return m_themeAccent; }
    QString themeAccentDim() const { return m_themeAccentDim; }
    QString themeWarning() const { return m_themeWarning; }
    QString themeDanger() const { return m_themeDanger; }
    QString themeMeterGreen() const { return m_themeMeterGreen; }
    QString themeMeterYellow() const { return m_themeMeterYellow; }
    QString themeMeterRed() const { return m_themeMeterRed; }
    QString themeMarker() const { return m_themeMarker; }
    QString themePlayhead() const { return m_themePlayhead; }
    QString themeWaveBackground() const { return m_themeWaveBackground; }
    QString themeWaveOutline() const { return m_themeWaveOutline; }

    // One colour, by the name of its property. The window writes through the property; this is for the theme sets,
    // which change twenty of them at once and must not flicker through twenty repaints.
    bool setThemeColour(const QString &name, const QString &colour);

    // The sets the window offers: a whole palette at a time, because colours chosen one by one do not make a theme.
    Q_INVOKABLE QStringList themeSets() const;
    Q_INVOKABLE QString themeSet() const;
    Q_INVOKABLE bool applyThemeSet(const QString &name);

    QString waveformStyle() const { return m_waveformStyle; }
    bool snapGrid() const { return m_snapGrid; }
    void setSnapGrid(bool on)
    {
        if (m_snapGrid == on) {
            return;
        }
        m_snapGrid = on;
        emit changed();
    }
    bool quantizeGrid() const { return m_quantizeGrid; }
    void setQuantizeGrid(bool on)
    {
        if (m_quantizeGrid == on) {
            return;
        }
        m_quantizeGrid = on;
        emit changed();
    }
    bool scrubAudible() const { return m_scrubAudible; }
    void setScrubAudible(bool on)
    {
        if (m_scrubAudible == on) {
            return;
        }
        m_scrubAudible = on;
        emit changed();
    }
    int frameHz() const { return m_frameHz; }
    double waveformZoom() const { return m_waveformZoom; }
    int frameInterval() const { return qMax(1, 1000 / qMax(1, m_frameHz)); }
    int waveformBarWidth() const { return m_waveformBarWidth; }
    double pitchRangePercent() const { return m_pitchRangePercent; }
    int waveformBarGap() const { return m_waveformBarGap; }

    // Both must stay positive; a zero width would leave nothing to draw.
    bool setWaveformBarWidth(int width);
    bool setWaveformBarGap(int gap);
    QString waveformLowColor() const { return m_waveformLowColor; }
    QString waveformMidColor() const { return m_waveformMidColor; }
    QString waveformHighColor() const { return m_waveformHighColor; }

    // Sets one of "low", "mid", "high"; false when the band name or the colour is wrong.
    bool setWaveformBandColor(const QString &band, const QString &color);

    void setThemeBackground(const QString &colour) { applyColour(m_themeBackground, colour); }
    void setThemeSurface(const QString &colour) { applyColour(m_themeSurface, colour); }
    void setThemePanel(const QString &colour) { applyColour(m_themePanel, colour); }
    void setThemeRaised(const QString &colour) { applyColour(m_themeRaised, colour); }
    void setThemeBorder(const QString &colour) { applyColour(m_themeBorder, colour); }
    void setThemeText(const QString &colour) { applyColour(m_themeText, colour); }
    void setThemeTextOnAccent(const QString &colour) { applyColour(m_themeTextOnAccent, colour); }
    void setThemeTextDim(const QString &colour) { applyColour(m_themeTextDim, colour); }
    void setThemeHeaderText(const QString &colour) { applyColour(m_themeHeaderText, colour); }
    void setThemeAccent(const QString &colour) { applyColour(m_themeAccent, colour); }
    void setThemeAccentDim(const QString &colour) { applyColour(m_themeAccentDim, colour); }
    void setThemeWarning(const QString &colour) { applyColour(m_themeWarning, colour); }
    void setThemeDanger(const QString &colour) { applyColour(m_themeDanger, colour); }
    void setThemeMeterGreen(const QString &colour) { applyColour(m_themeMeterGreen, colour); }
    void setThemeMeterYellow(const QString &colour) { applyColour(m_themeMeterYellow, colour); }
    void setThemeMeterRed(const QString &colour) { applyColour(m_themeMeterRed, colour); }
    void setThemeMarker(const QString &colour) { applyColour(m_themeMarker, colour); }
    void setThemePlayhead(const QString &colour) { applyColour(m_themePlayhead, colour); }
    void setThemeWaveBackground(const QString &colour) { applyColour(m_themeWaveBackground, colour); }
    void setThemeWaveOutline(const QString &colour) { applyColour(m_themeWaveOutline, colour); }    void setWaveformLowColor(const QString &colour) { applyColour(m_waveformLowColor, colour); }
    void setWaveformMidColor(const QString &colour) { applyColour(m_waveformMidColor, colour); }
    void setWaveformHighColor(const QString &colour) { applyColour(m_waveformHighColor, colour); }

    // Frames per second, 5..240; false when the number is outside what a display can use.
    bool setFrameHz(int hz);

    // Default zoom, 1..400; false when outside that.
    bool setWaveformZoom(double factor);

    // "coloured" or "plain"; anything else is refused.
    bool setWaveformStyle(const QString &style);

    // 1..50 percent; false when outside, because a throw nobody can aim with is not a setting.
    bool setPitchRangePercent(double percent);

    QString gainCurve() const { return m_gainCurve.kind(); }
    double gainCurveMiddle() const { return m_gainCurve.middle(); }
    bool setGainCurve(const QString &kind);
    bool setGainCurveMiddle(double middle);

    QString faderCurve() const { return m_faderCurve.kind(); }
    double faderCurveMiddle() const { return m_faderCurve.middle(); }
    bool setFaderCurve(const QString &kind);
    bool setFaderCurveMiddle(double middle);

    /// Position 0…1 of a level knob (unity to the end) as a share of its range.
    Q_INVOKABLE double gainCurveValue(double position) const;
    /// The position that lands on a share of the range — what the pointer is drawn from.
    Q_INVOKABLE double gainCurvePosition(double share) const;
    /// The same two directions for a channel fader: unity at 1, silence at 0.
    Q_INVOKABLE double faderCurveValue(double share) const;
    Q_INVOKABLE double faderCurvePosition(double value) const;

    // Set from the command line; an invalid colour is refused and the default kept.
    bool setWaveformOutsideColor(const QString &color);

signals:
    void changed();

public:
    // Writes the value and says so once; an invalid colour is refused and the old one kept.
    void applyColour(QString &slot, const QString &colour);

    // The stored shape of the shell's own settings, for layout.json.
    QJsonObject toJson() const;
    bool fromJson(const QJsonObject &source);

private:
    QString m_themeBackground = QStringLiteral("#101010");
    QString m_themeSurface = QStringLiteral("#101010");
    QString m_themePanel = QStringLiteral("#181818");
    QString m_themeRaised = QStringLiteral("#282828");
    QString m_themeBorder = QStringLiteral("#383838");
    QString m_themeTextOnAccent = QStringLiteral("#0b0b0b");
    QString m_themeText = QStringLiteral("#c8c8c8");
    QString m_themeTextDim = QStringLiteral("#8a8a8a");
    QString m_themeHeaderText = QStringLiteral("#7ea6dd");
    QString m_themeAccent = QStringLiteral("#5090f0");
    QString m_themeAccentDim = QStringLiteral("#204070");
    QString m_themeWarning = QStringLiteral("#e0a06a");
    QString m_themeDanger = QStringLiteral("#e06a5a");
    QString m_themeMeterGreen = QStringLiteral("#3fbf5f");
    QString m_themeMeterYellow = QStringLiteral("#e0c020");
    QString m_themeMeterRed = QStringLiteral("#ff4438");
    QString m_themeMarker = QStringLiteral("#e2c07f");
    QString m_themePlayhead = QStringLiteral("#f0f3f6");
    QString m_themeWaveBackground = QStringLiteral("#141414");
    QString m_themeWaveOutline = QStringLiteral("#5b636d");
    QString m_waveformOutsideColor = QStringLiteral("#101010");
    QString m_waveformStyle = QStringLiteral("coloured");
    // Twenty frames a second: the owner's default, and enough with the projection between snapshots.
    int m_frameHz = 20;
    // Ten times: the whole track on the overview strip, a tenth of it under the playhead.
    double m_waveformZoom = 10.0;
    // Sixteen percent: the owner's default. Wide enough for a long blend, narrow enough that the
    // handle still lands where the hand aims.
    double m_pitchRangePercent = 16.0;
    // Chosen by ear, not by arithmetic: the owner tried the range and kept 0.1 for the knobs — around
    // unity a knob is a fifth of the straight rate — and 0.5 for the faders, which are ridden.
    GainCurve m_gainCurve{0.1, QStringLiteral("cubic")};
    GainCurve m_faderCurve{0.5, QStringLiteral("logarithmic")};
    // One bar per pixel: the waveform is a continuous curve, and two-pixel bars halve the
    // horizontal detail for nothing.
    int m_waveformBarWidth = 1;
    int m_waveformBarGap = 0;
    // Red, green and blue channels: bass drives red, mids green, highs blue. Mixed additively, so
    // bass with mids reads orange and all three at once read pale.
    QString m_waveformLowColor = QStringLiteral("#ff2f1f");
    QString m_waveformMidColor = QStringLiteral("#35d13a");
    QString m_waveformHighColor = QStringLiteral("#2f6bff");

    // Global deck modes, on by default as the deck switches were.
    bool m_snapGrid = true;
    bool m_quantizeGrid = true;
    bool m_scrubAudible = false;
};
