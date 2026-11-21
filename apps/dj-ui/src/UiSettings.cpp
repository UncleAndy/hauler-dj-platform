#include "UiSettings.h"

#include <QColor>

UiSettings::UiSettings(QObject *parent)
    : QObject(parent)
{
}

bool UiSettings::setWaveformOutsideColor(const QString &color)
{
    const QColor parsed(color);
    if (!parsed.isValid()) {
        return false;
    }
    if (m_waveformOutsideColor == color) {
        return true;
    }
    m_waveformOutsideColor = color;
    emit changed();
    return true;
}

bool UiSettings::setWaveformStyle(const QString &style)
{
    const QString normalised = style.trimmed().toLower();
    if (normalised != QStringLiteral("coloured") && normalised != QStringLiteral("plain")) {
        return false;
    }
    if (m_waveformStyle == normalised) {
        return true;
    }
    m_waveformStyle = normalised;
    emit changed();
    return true;
}

bool UiSettings::setWaveformBandColor(const QString &band, const QString &color)
{
    if (!QColor(color).isValid()) {
        return false;
    }
    const QString name = band.trimmed().toLower();
    QString *target = nullptr;
    if (name == QStringLiteral("low")) {
        target = &m_waveformLowColor;
    } else if (name == QStringLiteral("mid")) {
        target = &m_waveformMidColor;
    } else if (name == QStringLiteral("high")) {
        target = &m_waveformHighColor;
    }
    if (target == nullptr) {
        return false;
    }
    if (*target != color) {
        *target = color;
        emit changed();
    }
    return true;
}

bool UiSettings::setFrameHz(int hz)
{
    if (hz < 5 || hz > 240) {
        return false;
    }
    if (m_frameHz == hz) {
        return true;
    }
    m_frameHz = hz;
    emit changed();
    return true;
}

bool UiSettings::setWaveformZoom(double factor)
{
    if (!(factor >= 1.0 && factor <= 400.0)) {
        return false;
    }
    if (qFuzzyCompare(m_waveformZoom, factor)) {
        return true;
    }
    m_waveformZoom = factor;
    emit changed();
    return true;
}

bool UiSettings::setPitchRangePercent(double percent)
{
    if (!(percent >= 1.0 && percent <= 50.0)) {
        return false;
    }
    if (qFuzzyCompare(m_pitchRangePercent, percent)) {
        return true;
    }
    m_pitchRangePercent = percent;
    emit changed();
    return true;
}

bool UiSettings::setWaveformBarWidth(int width)
{
    if (width <= 0 || width > 32) {
        return false;
    }
    m_waveformBarWidth = width;
    emit changed();
    return true;
}

bool UiSettings::setWaveformBarGap(int gap)
{
    if (gap < 0 || gap > 32) {
        return false;
    }
    m_waveformBarGap = gap;
    emit changed();
    return true;
}

bool UiSettings::setGainCurve(const QString &kind)
{
    if (kind == m_gainCurve.kind()) {
        return true;
    }
    GainCurve wanted = m_gainCurve;
    if (!wanted.configure(kind, m_gainCurve.middle())) {
        return false;
    }
    m_gainCurve = wanted;
    emit changed();
    return true;
}

bool UiSettings::setGainCurveMiddle(double middle)
{
    if (!m_gainCurve.configure(m_gainCurve.kind(), middle)) {
        return false;
    }
    emit changed();
    return true;
}

double UiSettings::gainCurveValue(double position) const
{
    return m_gainCurve.value(position);
}

double UiSettings::gainCurvePosition(double share) const
{
    return m_gainCurve.position(share);
}

bool UiSettings::setFaderCurve(const QString &kind)
{
    if (!m_faderCurve.configure(kind, m_faderCurve.middle())) {
        return false;
    }
    emit changed();
    return true;
}

bool UiSettings::setFaderCurveMiddle(double middle)
{
    if (!m_faderCurve.configure(m_faderCurve.kind(), middle)) {
        return false;
    }
    emit changed();
    return true;
}

double UiSettings::faderCurveValue(double share) const
{
    // A fader's range is the same shape as a knob's, only upside down: 1 is unity, 0 is silence, so the
    // travel is measured from unity down.
    return 1.0 - m_faderCurve.value(1.0 - share);
}

double UiSettings::faderCurvePosition(double value) const
{
    return 1.0 - m_faderCurve.position(1.0 - value);
}

void UiSettings::applyColour(QString &slot, const QString &colour)
{
    const QColor parsed(colour);
    if (!parsed.isValid()) {
        return;
    }
    // The file carries what the DJ chose, and a name is as good as a hex triple: keep the text as written unless it
    // names a colour that parses differently, so a theme stays readable when someone opens layout.json.
    const QString normalised = colour.trimmed();
    if (slot == normalised) {
        return;
    }
    slot = normalised;
    emit changed();
}

bool UiSettings::setThemeColour(const QString &name, const QString &colour)
{
    // One colour by the name of its property: the theme sets change twenty at once, and a table of member
    // pointers would only hide which field each name means.
    const QString key = name.trimmed();
    if (key == QLatin1String("themeBackground")) {
        applyColour(m_themeBackground, colour);
        return true;
    }
    if (key == QLatin1String("themeSurface")) {
        applyColour(m_themeSurface, colour);
        return true;
    }
    if (key == QLatin1String("themePanel")) {
        applyColour(m_themePanel, colour);
        return true;
    }
    if (key == QLatin1String("themeRaised")) {
        applyColour(m_themeRaised, colour);
        return true;
    }
    if (key == QLatin1String("themeBorder")) {
        applyColour(m_themeBorder, colour);
        return true;
    }
    if (key == QLatin1String("themeText")) {
        applyColour(m_themeText, colour);
        return true;
    }
    if (key == QLatin1String("themeTextOnAccent")) {
        applyColour(m_themeTextOnAccent, colour);
        return true;
    }
    if (key == QLatin1String("themeTextDim")) {
        applyColour(m_themeTextDim, colour);
        return true;
    }
    if (key == QLatin1String("themeHeaderText")) {
        applyColour(m_themeHeaderText, colour);
        return true;
    }
    if (key == QLatin1String("themeAccent")) {
        applyColour(m_themeAccent, colour);
        return true;
    }
    if (key == QLatin1String("themeAccentDim")) {
        applyColour(m_themeAccentDim, colour);
        return true;
    }
    if (key == QLatin1String("themeWarning")) {
        applyColour(m_themeWarning, colour);
        return true;
    }
    if (key == QLatin1String("themeDanger")) {
        applyColour(m_themeDanger, colour);
        return true;
    }
    if (key == QLatin1String("themeMeterGreen")) {
        applyColour(m_themeMeterGreen, colour);
        return true;
    }
    if (key == QLatin1String("themeMeterYellow")) {
        applyColour(m_themeMeterYellow, colour);
        return true;
    }
    if (key == QLatin1String("themeMeterRed")) {
        applyColour(m_themeMeterRed, colour);
        return true;
    }
    if (key == QLatin1String("themeMarker")) {
        applyColour(m_themeMarker, colour);
        return true;
    }
    if (key == QLatin1String("themePlayhead")) {
        applyColour(m_themePlayhead, colour);
        return true;
    }
    if (key == QLatin1String("themeWaveBackground")) {
        applyColour(m_themeWaveBackground, colour);
        return true;
    }
    if (key == QLatin1String("themeWaveOutline")) {
        applyColour(m_themeWaveOutline, colour);
        return true;
    }
    return false;
}

namespace {

// The sets the window offers. Three is enough to tell a booth from a daylight desk, and each is a whole palette:
// surfaces, text, accent and the meters together, because a dark surface under a warm accent reads as a mistake.
struct ThemeSet {
    const char *name;
    struct {
        const char *key;
        const char *colour;
    } colours[20];
};

const ThemeSet themeSetsTable[] = {
    { "dark",
      { { "themeBackground", "#101010" }, { "themeSurface", "#101010" }, { "themePanel", "#181818" },
        { "themeRaised", "#282828" }, { "themeBorder", "#383838" }, { "themeText", "#c8c8c8" },
        { "themeTextOnAccent", "#0b0b0b" }, { "themeTextDim", "#8a8a8a" }, { "themeHeaderText", "#7ea6dd" },
        { "themeAccent", "#5090f0" }, { "themeAccentDim", "#204070" }, { "themeWarning", "#e0a06a" },
        { "themeDanger", "#e06a5a" }, { "themeMeterGreen", "#3fbf5f" }, { "themeMeterYellow", "#e0c020" },
        { "themeMeterRed", "#ff4438" }, { "themeMarker", "#e2c07f" }, { "themePlayhead", "#f0f3f6" },
        { "themeWaveBackground", "#141414" }, { "themeWaveOutline", "#5b636d" } } },
    { "graphite",
      { { "themeBackground", "#15171a" }, { "themeSurface", "#1b1e22" }, { "themePanel", "#22262b" },
        { "themeRaised", "#2c3137" }, { "themeBorder", "#3a4048" }, { "themeText", "#d2d6db" },
        { "themeTextOnAccent", "#0c0e10" }, { "themeTextDim", "#8d949c" }, { "themeHeaderText", "#9fb4cc" },
        { "themeAccent", "#7fa6c9" }, { "themeAccentDim", "#33475a" }, { "themeWarning", "#d9b07a" },
        { "themeDanger", "#d97b6c" }, { "themeMeterGreen", "#69c283" }, { "themeMeterYellow", "#dfcb5c" },
        { "themeMeterRed", "#ef6f63" }, { "themeMarker", "#c9bda2" }, { "themePlayhead", "#eef1f4" },
        { "themeWaveBackground", "#191c20" }, { "themeWaveOutline", "#6a737d" } } },
    { "warm",
      { { "themeBackground", "#141110" }, { "themeSurface", "#1a1614" }, { "themePanel", "#221c19" },
        { "themeRaised", "#2e2622" }, { "themeBorder", "#41352f" }, { "themeText", "#d8cfc6" },
        { "themeTextOnAccent", "#120e0c" }, { "themeTextDim", "#978a80" }, { "themeHeaderText", "#d8a97a" },
        { "themeAccent", "#e08a4a" }, { "themeAccentDim", "#5a3720" }, { "themeWarning", "#e3b26a" },
        { "themeDanger", "#d9614f" }, { "themeMeterGreen", "#8fbf62" }, { "themeMeterYellow", "#e0b34a" },
        { "themeMeterRed", "#f0553c" }, { "themeMarker", "#e0b783" }, { "themePlayhead", "#f6efe7" },
        { "themeWaveBackground", "#191412" }, { "themeWaveOutline", "#6d5c50" } } },
};

} // namespace

QStringList UiSettings::themeSets() const
{
    QStringList names;
    for (const ThemeSet &set : themeSetsTable) {
        names << QString::fromLatin1(set.name);
    }
    return names;
}

bool UiSettings::applyThemeSet(const QString &name)
{
    const QString wanted = name.trimmed().toLower();
    for (const ThemeSet &set : themeSetsTable) {
        if (wanted != QString::fromLatin1(set.name)) {
            continue;
        }
        for (const auto &colour : set.colours) {
            setThemeColour(QString::fromLatin1(colour.key), QString::fromLatin1(colour.colour));
        }
        return true;
    }
    return false;
}

QString UiSettings::themeSet() const
{
    // Which set the colours are, if any: one colour written by hand makes it "custom", and the window shows that
    // rather than pretending the nearest set is in force.
    for (const ThemeSet &set : themeSetsTable) {
        bool same = true;
        for (const auto &colour : set.colours) {
            const QString key = QString::fromLatin1(colour.key);
            QString current;
            if (key == QStringLiteral("themeBackground")) {
                current = m_themeBackground;
            } else if (key == QStringLiteral("themeSurface")) {
                current = m_themeSurface;
            } else if (key == QStringLiteral("themePanel")) {
                current = m_themePanel;
            } else if (key == QStringLiteral("themeRaised")) {
                current = m_themeRaised;
            } else if (key == QStringLiteral("themeBorder")) {
                current = m_themeBorder;
            } else if (key == QStringLiteral("themeText")) {
                current = m_themeText;
            } else if (key == QStringLiteral("themeTextOnAccent")) {
                current = m_themeTextOnAccent;
            } else if (key == QStringLiteral("themeTextDim")) {
                current = m_themeTextDim;
            } else if (key == QStringLiteral("themeHeaderText")) {
                current = m_themeHeaderText;
            } else if (key == QStringLiteral("themeAccent")) {
                current = m_themeAccent;
            } else if (key == QStringLiteral("themeAccentDim")) {
                current = m_themeAccentDim;
            } else if (key == QStringLiteral("themeWarning")) {
                current = m_themeWarning;
            } else if (key == QStringLiteral("themeDanger")) {
                current = m_themeDanger;
            } else if (key == QStringLiteral("themeMeterGreen")) {
                current = m_themeMeterGreen;
            } else if (key == QStringLiteral("themeMeterYellow")) {
                current = m_themeMeterYellow;
            } else if (key == QStringLiteral("themeMeterRed")) {
                current = m_themeMeterRed;
            } else if (key == QStringLiteral("themeMarker")) {
                current = m_themeMarker;
            } else if (key == QStringLiteral("themePlayhead")) {
                current = m_themePlayhead;
            } else if (key == QStringLiteral("themeWaveBackground")) {
                current = m_themeWaveBackground;
            } else if (key == QStringLiteral("themeWaveOutline")) {
                current = m_themeWaveOutline;
            }
            if (current.compare(QString::fromLatin1(colour.colour), Qt::CaseInsensitive) != 0) {
                same = false;
                break;
            }
        }
        if (same) {
            return QString::fromLatin1(set.name);
        }
    }
    return QStringLiteral("custom");
}

QJsonObject UiSettings::toJson() const
{
    QJsonObject out;
    out.insert(QStringLiteral("theme_background"), m_themeBackground);
    out.insert(QStringLiteral("theme_surface"), m_themeSurface);
    out.insert(QStringLiteral("theme_panel"), m_themePanel);
    out.insert(QStringLiteral("theme_raised"), m_themeRaised);
    out.insert(QStringLiteral("theme_border"), m_themeBorder);
    out.insert(QStringLiteral("theme_text"), m_themeText);
    out.insert(QStringLiteral("theme_text_on_accent"), m_themeTextOnAccent);
    out.insert(QStringLiteral("theme_text_dim"), m_themeTextDim);
    out.insert(QStringLiteral("theme_header_text"), m_themeHeaderText);
    out.insert(QStringLiteral("theme_accent"), m_themeAccent);
    out.insert(QStringLiteral("theme_accent_dim"), m_themeAccentDim);
    out.insert(QStringLiteral("theme_warning"), m_themeWarning);
    out.insert(QStringLiteral("theme_danger"), m_themeDanger);
    out.insert(QStringLiteral("theme_meter_green"), m_themeMeterGreen);
    out.insert(QStringLiteral("theme_meter_yellow"), m_themeMeterYellow);
    out.insert(QStringLiteral("theme_meter_red"), m_themeMeterRed);
    out.insert(QStringLiteral("theme_marker"), m_themeMarker);
    out.insert(QStringLiteral("theme_playhead"), m_themePlayhead);
    out.insert(QStringLiteral("theme_wave_background"), m_themeWaveBackground);
    out.insert(QStringLiteral("theme_wave_outline"), m_themeWaveOutline);
    out.insert(QStringLiteral("waveform_outside_color"), m_waveformOutsideColor);
    out.insert(QStringLiteral("waveform_low_color"), m_waveformLowColor);
    out.insert(QStringLiteral("waveform_mid_color"), m_waveformMidColor);
    out.insert(QStringLiteral("waveform_high_color"), m_waveformHighColor);
    out.insert(QStringLiteral("waveform_style"), m_waveformStyle);
    out.insert(QStringLiteral("waveform_zoom"), m_waveformZoom);
    out.insert(QStringLiteral("waveform_bar_width"), m_waveformBarWidth);
    out.insert(QStringLiteral("waveform_bar_gap"), m_waveformBarGap);
    out.insert(QStringLiteral("frame_hz"), m_frameHz);
    out.insert(QStringLiteral("pitch_range_percent"), m_pitchRangePercent);
    out.insert(QStringLiteral("gain_curve"), m_gainCurve.kind());
    out.insert(QStringLiteral("gain_curve_middle"), m_gainCurve.middle());
    out.insert(QStringLiteral("fader_curve"), m_faderCurve.kind());
    out.insert(QStringLiteral("fader_curve_middle"), m_faderCurve.middle());
    return out;
}

bool UiSettings::fromJson(const QJsonObject &source)
{
    // A key that is not there keeps its default: an older file must not blank a newer setting, and the setters
    // already refuse nonsense (a colour that does not parse, a rate a display cannot use).
    const auto colour = [&source](const char *key, QString &slot) {
        const QJsonValue value = source.value(QString::fromLatin1(key));
        if (value.isString()) {
            slot = value.toString();
        }
    };
    colour("theme_background", m_themeBackground);
    colour("theme_surface", m_themeSurface);
    colour("theme_panel", m_themePanel);
    colour("theme_raised", m_themeRaised);
    colour("theme_border", m_themeBorder);
    colour("theme_text", m_themeText);
    colour("theme_text_on_accent", m_themeTextOnAccent);
    colour("theme_text_dim", m_themeTextDim);
    colour("theme_header_text", m_themeHeaderText);
    colour("theme_accent", m_themeAccent);
    colour("theme_accent_dim", m_themeAccentDim);
    colour("theme_warning", m_themeWarning);
    colour("theme_danger", m_themeDanger);
    colour("theme_meter_green", m_themeMeterGreen);
    colour("theme_meter_yellow", m_themeMeterYellow);
    colour("theme_meter_red", m_themeMeterRed);
    colour("theme_marker", m_themeMarker);
    colour("theme_playhead", m_themePlayhead);
    colour("theme_wave_background", m_themeWaveBackground);
    colour("theme_wave_outline", m_themeWaveOutline);
    colour("waveform_outside_color", m_waveformOutsideColor);
    colour("waveform_low_color", m_waveformLowColor);
    colour("waveform_mid_color", m_waveformMidColor);
    colour("waveform_high_color", m_waveformHighColor);

    const auto number = [&source](const char *key, double &slot) {
        const QJsonValue value = source.value(QString::fromLatin1(key));
        if (value.isDouble()) {
            slot = value.toDouble();
        }
    };
    const auto integer = [&source](const char *key, int &slot) {
        const QJsonValue value = source.value(QString::fromLatin1(key));
        if (value.isDouble()) {
            slot = value.toInt();
        }
    };
    const auto text = [&source](const char *key, QString &slot) {
        const QJsonValue value = source.value(QString::fromLatin1(key));
        if (value.isString()) {
            slot = value.toString();
        }
    };
    text("waveform_style", m_waveformStyle);
    number("waveform_zoom", m_waveformZoom);
    integer("waveform_bar_width", m_waveformBarWidth);
    integer("waveform_bar_gap", m_waveformBarGap);
    integer("frame_hz", m_frameHz);
    number("pitch_range_percent", m_pitchRangePercent);
    // The curves go through their own setters: a law the shell does not know is refused there, and the default kept.
    const auto kind = [&source](const char *key) -> QString {
        const QJsonValue value = source.value(QString::fromLatin1(key));
        return value.isString() ? value.toString() : QString();
    };
    const QString gain = kind("gain_curve");
    if (!gain.isEmpty()) {
        setGainCurve(gain);
    }
    const QString fader = kind("fader_curve");
    if (!fader.isEmpty()) {
        setFaderCurve(fader);
    }
    double middle = 0.0;
    number("gain_curve_middle", middle);
    if (middle > 0.0) {
        setGainCurveMiddle(middle);
    }
    middle = 0.0;
    number("fader_curve_middle", middle);
    if (middle > 0.0) {
        setFaderCurveMiddle(middle);
    }
    emit changed();
    return true;
}
