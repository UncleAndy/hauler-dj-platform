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

/// One pad drawn on a strip: where it is, what colour it wears, what it is called.
struct CueMark
{
    double ratio = -1.0;
    QColor color;
    QString label;
};

/// One beat of the grid, as a fraction of the track: drawn as a line, thicker when it starts a bar.
struct BeatMark
{
    double ratio = -1.0;
    /// Every fourth beat is a bar: the beat a DJ counts to four on, and the one worth a thicker line.
    bool bar = false;
    /// Every sixteenth beat starts a square of four bars. Without it a shifted grid looks untouched:
    /// moving the whole grid by exactly one bar lands every bar line on the line before it, and only the
    /// start of the square tells the DJ which bar is the first one.
    bool square = false;
};

/// What the last paint actually drew: the check reads these numbers instead of believing the strip.
struct WaveformStats
{
    int beats = 0;
    int bars = 0;
    int squares = 0;
    int anchors = 0;
    /// Distance between two neighbouring beats on screen, in pixels: 0 when fewer than two are visible.
    double beatSpacing = 0.0;
    /// Where the first visible square starts, as a fraction of the track: the number that moves when the
    /// grid is shifted by a bar, and the one the strip's check reads.
    double firstSquare = -1.0;
    /// One when the virtual play head of slip was on the strip, zero when there was nothing to draw.
    int slip = 0;
};

/// A loop stored on a pad: drawn as a pair of edge marks along the bottom of the strip, so a loop that
/// nobody has pressed yet is still visible on the track.
struct LoopMark
{
    double from = -1.0;
    double to = -1.0;
    /// Armed loops are drawn as the loops they are; a saved loop that loop mode is not watching is a
    /// reminder of where the loop is, not a promise that it will repeat.
    bool active = false;
};

struct WaveformStyle
{
    bool coloured = true;
    /// Height of a pad's number, in pixels: a full-track strip has no room for the size the zoomed
    /// strip uses, and a clipped number is not a label.
    int markFont = 9;
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

// `windowStart` and `span` are fractions of the track; `playheadRatio` is where it is now. Cues
// and the loop are fractions too, so nothing is drawn twice with two different timebases.
WaveformStats paintWaveform(QPainter &painter, const Waveform &waveform, const WaveformStyle &style,
                             const QRect &area, double windowStart, double span,
                             double playheadRatio, double slipRatio, const QList<CueMark> &cues,
                             const QList<LoopMark> &padLoops, bool hasLoop, double loopFrom,
                             double loopTo, bool loopActive, double pendingFrom,
                             const QList<BeatMark> &beats, const QList<double> &anchors);
