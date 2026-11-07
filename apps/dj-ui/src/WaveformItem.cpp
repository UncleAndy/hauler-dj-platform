#include "WaveformItem.h"

#include "Waveform.h"
#include "WaveformPainter.h"

#include <QPainter>

#include <algorithm>

WaveformItem::WaveformItem(QQuickItem *parent)
    : QQuickPaintedItem(parent)
{
    setAntialiasing(false);
    setOpaquePainting(true);
}

void WaveformItem::paint(QPainter *painter)
{
    // The envelope is a QML object of the deck panel: it can be destroyed before this item, and a raw
    // pointer here would paint through freed memory at shutdown.
    if (m_waveform.isNull()) {
        return;
    }
    WaveformStyle style;
    style.coloured = m_coloured;
    style.barWidth = m_barWidth;
    style.barGap = m_barGap;
    style.markFont = m_markFont;
    style.background = m_background;
    style.outside = m_outside;
    style.flat = m_flat;
    style.border = m_border;
    style.accent = m_accent;
    style.marker = m_marker;
    style.playhead = m_playhead;

    // Each mark arrives as {ratio, color, label}: the strip paints in fractions, and the conversion
    // from the pad's frame belongs to whoever knows the track's length (the QML side).
    QList<CueMark> cues;
    cues.reserve(m_cues.size());
    for (const QVariant &entry : m_cues) {
        const QVariantMap map = entry.toMap();
        const double ratio = map.value(QStringLiteral("ratio")).toDouble();
        if (!(ratio >= 0.0) || ratio > 1.0) {
            continue;
        }
        CueMark mark;
        mark.ratio = ratio;
        mark.color = QColor(map.value(QStringLiteral("color")).toString());
        mark.label = map.value(QStringLiteral("label")).toString();
        cues.append(mark);
    }

    paintWaveform(*painter, *m_waveform, style, QRect(0, 0, int(width()), int(height())),
                  m_windowStart, m_span, m_playheadRatio, cues, m_hasLoop, m_loopFrom, m_loopTo);
}

void WaveformItem::setWaveform(Waveform *waveform)
{
    if (m_waveform == waveform) {
        return;
    }
    if (!m_waveform.isNull()) {
        disconnect(m_waveform, nullptr, this, nullptr);
    }
    m_waveform = waveform;
    if (!m_waveform.isNull()) {
        // A new envelope, or a new one loaded into the same object, repaints: this is how the
        // waveform appears after a track is loaded.
        connect(m_waveform, &Waveform::changed, this, [this] { update(); });
    }
    update();
    emit appearanceChanged();
}

void WaveformItem::setWindowStart(double value)
{
    if (qFuzzyCompare(m_windowStart, value)) {
        return;
    }
    m_windowStart = value;
    update();
    emit appearanceChanged();
}

void WaveformItem::setSpan(double value)
{
    if (qFuzzyCompare(m_span, value)) {
        return;
    }
    m_span = value;
    update();
    emit appearanceChanged();
}

void WaveformItem::setPlayheadRatio(double value)
{
    if (qFuzzyCompare(m_playheadRatio, value)) {
        return;
    }
    m_playheadRatio = value;
    update();
    emit appearanceChanged();
}

void WaveformItem::setCues(const QVariantList &value)
{
    m_cues = value;
    update();
    emit appearanceChanged();
}

void WaveformItem::setHasLoop(bool value)
{
    if (m_hasLoop == value) {
        return;
    }
    m_hasLoop = value;
    update();
    emit appearanceChanged();
}

void WaveformItem::setLoopFrom(double value)
{
    if (qFuzzyCompare(m_loopFrom, value)) {
        return;
    }
    m_loopFrom = value;
    update();
    emit appearanceChanged();
}

void WaveformItem::setLoopTo(double value)
{
    if (qFuzzyCompare(m_loopTo, value)) {
        return;
    }
    m_loopTo = value;
    update();
    emit appearanceChanged();
}

void WaveformItem::setColoured(bool value)
{
    if (m_coloured == value) {
        return;
    }
    m_coloured = value;
    update();
    emit appearanceChanged();
}

void WaveformItem::setBarWidth(int value)
{
    if (m_barWidth == value) {
        return;
    }
    m_barWidth = value;
    update();
    emit appearanceChanged();
}

void WaveformItem::setBarGap(int value)
{
    if (m_barGap == value) {
        return;
    }
    m_barGap = value;
    update();
    emit appearanceChanged();
}

void WaveformItem::setBackground(const QColor &value)
{
    m_background = value;
    update();
    emit appearanceChanged();
}

void WaveformItem::setOutside(const QColor &value)
{
    m_outside = value;
    update();
    emit appearanceChanged();
}

void WaveformItem::setFlat(const QColor &value)
{
    m_flat = value;
    update();
    emit appearanceChanged();
}

void WaveformItem::setBorder(const QColor &value)
{
    m_border = value;
    update();
    emit appearanceChanged();
}

void WaveformItem::setAccent(const QColor &value)
{
    m_accent = value;
    update();
    emit appearanceChanged();
}

void WaveformItem::setMarker(const QColor &value)
{
    m_marker = value;
    update();
    emit appearanceChanged();
}

void WaveformItem::setPlayhead(const QColor &value)
{
    m_playhead = value;
    update();
    emit appearanceChanged();
}

void WaveformItem::setMarkFont(int value)
{
    if (m_markFont == value) {
        return;
    }
    m_markFont = value;
    update();
}
