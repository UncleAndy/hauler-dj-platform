// The waveform strip as a QML item, painted in C++ (A07, DJP-UI-004).
//
// QML asks for a repaint by changing a property; the drawing itself never crosses into JavaScript, so
// a fast scroll stays at the display's frame rate.

#pragma once

#include <QColor>
#include <QPointer>
#include <QQuickPaintedItem>
#include <QVariantList>

class Waveform;

class WaveformItem : public QQuickPaintedItem
{
    Q_OBJECT
    Q_PROPERTY(Waveform *waveform READ waveform WRITE setWaveform NOTIFY appearanceChanged)
    Q_PROPERTY(double windowStart READ windowStart WRITE setWindowStart NOTIFY appearanceChanged)
    Q_PROPERTY(double span READ span WRITE setSpan NOTIFY appearanceChanged)
    Q_PROPERTY(double playheadRatio READ playheadRatio WRITE setPlayheadRatio NOTIFY appearanceChanged)
    Q_PROPERTY(QVariantList hotcues READ hotcues WRITE setHotcues NOTIFY appearanceChanged)
    Q_PROPERTY(bool hasLoop READ hasLoop WRITE setHasLoop NOTIFY appearanceChanged)
    Q_PROPERTY(double loopFrom READ loopFrom WRITE setLoopFrom NOTIFY appearanceChanged)
    Q_PROPERTY(double loopTo READ loopTo WRITE setLoopTo NOTIFY appearanceChanged)
    Q_PROPERTY(bool coloured READ coloured WRITE setColoured NOTIFY appearanceChanged)
    Q_PROPERTY(int barWidth READ barWidth WRITE setBarWidth NOTIFY appearanceChanged)
    Q_PROPERTY(int barGap READ barGap WRITE setBarGap NOTIFY appearanceChanged)
    Q_PROPERTY(QColor background READ background WRITE setBackground NOTIFY appearanceChanged)
    Q_PROPERTY(QColor outside READ outside WRITE setOutside NOTIFY appearanceChanged)
    Q_PROPERTY(QColor flat READ flat WRITE setFlat NOTIFY appearanceChanged)
    Q_PROPERTY(QColor border READ border WRITE setBorder NOTIFY appearanceChanged)
    Q_PROPERTY(QColor accent READ accent WRITE setAccent NOTIFY appearanceChanged)
    Q_PROPERTY(QColor marker READ marker WRITE setMarker NOTIFY appearanceChanged)
    Q_PROPERTY(QColor playhead READ playhead WRITE setPlayhead NOTIFY appearanceChanged)

public:
    explicit WaveformItem(QQuickItem *parent = nullptr);

    void paint(QPainter *painter) override;

    Waveform *waveform() const { return m_waveform.data(); }
    void setWaveform(Waveform *waveform);

    double windowStart() const { return m_windowStart; }
    void setWindowStart(double value);
    double span() const { return m_span; }
    void setSpan(double value);
    double playheadRatio() const { return m_playheadRatio; }
    void setPlayheadRatio(double value);
    QVariantList hotcues() const { return m_hotcues; }
    void setHotcues(const QVariantList &value);
    bool hasLoop() const { return m_hasLoop; }
    void setHasLoop(bool value);
    double loopFrom() const { return m_loopFrom; }
    void setLoopFrom(double value);
    double loopTo() const { return m_loopTo; }
    void setLoopTo(double value);
    bool coloured() const { return m_coloured; }
    void setColoured(bool value);
    int barWidth() const { return m_barWidth; }
    void setBarWidth(int value);
    int barGap() const { return m_barGap; }
    void setBarGap(int value);
    QColor background() const { return m_background; }
    void setBackground(const QColor &value);
    QColor outside() const { return m_outside; }
    void setOutside(const QColor &value);
    QColor flat() const { return m_flat; }
    void setFlat(const QColor &value);
    QColor border() const { return m_border; }
    void setBorder(const QColor &value);
    QColor accent() const { return m_accent; }
    void setAccent(const QColor &value);
    QColor marker() const { return m_marker; }
    void setMarker(const QColor &value);
    QColor playhead() const { return m_playhead; }
    void setPlayhead(const QColor &value);

signals:
    void appearanceChanged();

private:
    // QPointer, not a raw pointer: the envelope is a QML object of the deck panel and may be destroyed
    // before this item. A raw pointer here turns a shutdown into a heap corruption report.
    QPointer<Waveform> m_waveform;
    double m_windowStart = 0.0;
    double m_span = 1.0;
    double m_playheadRatio = 0.0;
    QVariantList m_hotcues;
    bool m_hasLoop = false;
    double m_loopFrom = 0.0;
    double m_loopTo = 0.0;
    bool m_coloured = true;
    int m_barWidth = 2;
    int m_barGap = 0;
    QColor m_background;
    QColor m_outside;
    QColor m_flat;
    QColor m_border;
    QColor m_accent;
    QColor m_marker;
    QColor m_playhead;
};
