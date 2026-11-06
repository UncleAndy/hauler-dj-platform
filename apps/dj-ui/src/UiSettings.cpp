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
