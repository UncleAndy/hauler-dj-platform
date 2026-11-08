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
    GainCurve faders = m_faderCurve;
    if (!wanted.configure(kind, m_gainCurve.middle())
        || !faders.configure(kind, m_faderCurve.middle())) {
        return false;
    }
    m_gainCurve = wanted;
    m_faderCurve = faders;
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
