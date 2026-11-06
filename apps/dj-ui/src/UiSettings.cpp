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
