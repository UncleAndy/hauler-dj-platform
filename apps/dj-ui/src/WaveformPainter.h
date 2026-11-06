// Paints a deck's waveform strip (A07, DJP-UI-004).
//
// The strip is redrawn every frame while the track scrolls, so it is drawn here in C++ with plain
// QPainter calls over an array of floats. Doing the same in QML Canvas cost a QVariantMap per bar and
// a JavaScript round trip per rectangle, which is what made a fast scroll look like a slideshow.

#pragma once

#include <QColor>
#include <QList>
#include <QPainter>

#include <algorithm>

class Waveform;

struct WaveformStyle
{
    bool coloured = true;
    int barWidth = 2;
    int barGap = 0;
    // Top to bottom brightness of a bar: the reference waveform glows from the centre outwards
    // instead of being a flat block of colour.
    double slices[3] = {0.42, 1.0, 0.42};
    QColor background;
    QColor outside;
    QColor flat;
    QColor border;
    QColor accent;
    QColor marker;
    QColor playhead;
};

// Bar width plus gap, in pixels, and never less than one: a bar has to occupy at least a pixel.
inline int frameStep(const WaveformStyle &style)
{
    return std::max(1, style.barWidth) + std::max(0, style.barGap);
}

// `windowStart` and `span` are fractions of the track; `playheadRatio` is where it is now. Hotcues
// and the loop are fractions too, so nothing is drawn twice with two different timebases.
void paintWaveform(QPainter &painter, const Waveform &waveform, const WaveformStyle &style,
                   const QRect &area, double windowStart, double span, double playheadRatio,
                   const QList<double> &hotcues, bool hasLoop, double loopFrom, double loopTo);
