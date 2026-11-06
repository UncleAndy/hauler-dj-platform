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
