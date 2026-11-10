#include "WaveformPainter.h"

#include "Waveform.h"

#include <QCoreApplication>

#include <algorithm>
#include <cmath>

WaveformStats paintWaveform(QPainter &painter, const Waveform &waveform,
                             const WaveformStyle &style, const QRect &area, double windowStart,
                             double span, double playheadRatio, double slipRatio,
                             const QList<CueMark> &cues,
                             const QList<LoopMark> &padLoops, bool hasLoop, double loopFrom,
                             double loopTo, bool loopActive, double pendingFrom,
                             const QList<BeatMark> &beats, const QList<double> &anchors)
{
    WaveformStats stats;
    if (area.width() <= 0 || area.height() <= 0) {
        return stats;
    }
    painter.save();
    painter.setRenderHint(QPainter::Antialiasing, false);
    painter.fillRect(area, style.background);

    const int width = area.width();
    const int height = area.height();
    const int middle = height / 2;
    const auto xOfRatio = [&](double ratio) {
        return area.left() + int(std::lround(((ratio - windowStart) / span) * double(width)));
    };

    // Everything outside the track is its own colour, so it cannot be mistaken for a quiet passage.
    const int startX = xOfRatio(0.0);
    const int endX = xOfRatio(1.0);
    if (startX > area.left()) {
        painter.fillRect(QRect(area.left(), area.top(), std::min(startX, area.right() + 1) - area.left(), height),
                         style.outside);
    }
    if (endX < area.right() + 1) {
        const int from = std::max(area.left(), endX);
        painter.fillRect(QRect(from, area.top(), area.right() + 1 - from, height), style.outside);
    }

    // The centre line keeps a quiet passage reading as silence in the track, not as a gap.
    painter.setPen(style.border);
    painter.drawLine(area.left(), area.top() + middle, area.right(), area.top() + middle);

    const int step = frameStep(style);
    const int barCount = std::max(1, width / step);
    const std::vector<Waveform::Bar> bars = waveform.bars(barCount, windowStart, span);

    for (int index = 0; index < int(bars.size()); ++index) {
        const Waveform::Bar &bar = bars[std::size_t(index)];
        if (bar.outside) {
            continue;
        }
        const double half = std::max(0.5, double(bar.amplitude) * double(middle));
        const int x = area.left() + index * step;
        if (!style.coloured) {
            painter.fillRect(QRect(x, area.top() + int(std::lround(middle - half)), style.barWidth,
                                   std::max(1, int(std::lround(half * 2.0)))),
                             style.flat);
            continue;
        }
        const QColor base(bar.colour);
        for (int slice = 0; slice < 3; ++slice) {
            const double from = middle - half + (2.0 * half) * double(slice) / 3.0;
            const double to = middle - half + (2.0 * half) * double(slice + 1) / 3.0;
            const double factor = style.slices[slice];
            painter.fillRect(QRect(x, area.top() + int(std::lround(from)), style.barWidth,
                                   std::max(1, int(std::lround(to - from)))),
                             QColor(int(std::lround(base.red() * factor)),
                                    int(std::lround(base.green() * factor)),
                                    int(std::lround(base.blue() * factor))));
        }
    }

    // The grid: a line per beat, a thicker one per bar. Drawn over the waveform and under everything the
    // DJ put there himself — the grid is what he reads the track against, and a line he cannot see is a
    // grid he cannot use (DJP-SYNC-003).
    {
        QColor beatColour = style.border;
        beatColour.setAlpha(170);
        QColor barColour = style.accent;
        barColour.setAlpha(190);
        int firstBeatX = 0;
        int secondBeatX = 0;
        for (const BeatMark &beat : beats) {
            const int x = xOfRatio(beat.ratio);
            if (x < area.left() || x > area.right()) {
                continue;
            }
            if (beat.square) {
                // A square of four bars is where a phrase starts: the one line a DJ counts from, and the
                // one that proves a one-bar shift moved something.
                painter.fillRect(QRect(x, area.top(), 3, height), style.marker);
                painter.fillRect(QRect(x, area.top() + height, 7, 2), style.marker);
                ++stats.squares;
                if (stats.firstSquare < 0.0) {
                    stats.firstSquare = beat.ratio;
                }
            } else if (beat.bar) {
                painter.fillRect(QRect(x, area.top(), 2, height), barColour);
                ++stats.bars;
            } else {
                painter.fillRect(QRect(x, area.top() + height / 8, 1, height - height / 4), beatColour);
                ++stats.beats;
            }
            if (stats.beats + stats.bars + stats.squares <= 2) {
                if (stats.beats + stats.bars + stats.squares == 1) {
                    firstBeatX = x;
                } else {
                    secondBeatX = x;
                }
            }
        }
        if (secondBeatX > firstBeatX) {
            stats.beatSpacing = double(secondBeatX - firstBeatX);
        }
        // Anchors are the frames where the tempo changes: a grid that changes under the hand has to show
        // where, or the DJ corrects the wrong part of the track.
        QColor anchorColour = style.marker;
        for (const double ratio : anchors) {
            const int x = xOfRatio(ratio);
            if (x < area.left() || x > area.right()) {
                continue;
            }
            painter.fillRect(QRect(x, area.top(), 2, 6), anchorColour);
            painter.fillRect(QRect(x, area.top(), 6, 2), anchorColour);
            ++stats.anchors;
        }
    }

    // A loop's edges are drawn whether it is running or not: a region whose boundary cannot be seen
    // is a region that cannot be switched on, and a switched-off loop that disappears leaves the DJ
    // wondering whether it was ever there. The tint follows the state, the edges never do.
    QColor edge = style.accent;
    if (!loopActive) {
        edge.setAlpha(150);
    }
    if (hasLoop && loopTo > loopFrom) {
        const int from = xOfRatio(loopFrom);
        const int to = xOfRatio(loopTo);
        if (to > from) {
            const int left = std::max(area.left(), from);
            const int right = std::min(area.right() + 1, to);
            if (right > left) {
                QColor tint = style.accent;
                tint.setAlpha(loopActive ? 36 : 12);
                painter.fillRect(QRect(left, area.top(), right - left, height), tint);
            }
            painter.fillRect(QRect(from, area.top(), 3, height), edge);
            painter.fillRect(QRect(to - 2, area.top(), 3, height), edge);
            // Handles at the top and the bottom edge, so the two boundaries read as the ends of a
            // region rather than as two stray lines.
            painter.fillRect(QRect(from - 1, area.top(), 5, 5), edge);
            painter.fillRect(QRect(to - 3, area.top(), 5, 5), edge);
            painter.fillRect(QRect(from - 1, area.bottom() - 4, 5, 5), edge);
            painter.fillRect(QRect(to - 3, area.bottom() - 4, 5, 5), edge);
        }
    }

    // Loops stored on pads: drawn before anything is pressed, because a loop you cannot find is a loop
    // you will not use. They live on the bottom edge, where the cue lines never go.
    for (const LoopMark &mark : padLoops) {
        if (mark.to <= mark.from) {
            continue;
        }
        const int from = xOfRatio(mark.from);
        const int to = xOfRatio(mark.to);
        if (to <= from) {
            continue;
        }
        const int left = std::max(area.left(), from);
        const int right = std::min(area.right() + 1, to);
        if (right <= left) {
            continue;
        }
        // A loop the DJ stored is a fact about the track, so it has to be seen before anything is
        // pressed: a very dim mark on a dark strip is a mark nobody finds.
        QColor tint = style.accent;
        tint.setAlpha(mark.active ? 56 : 30);
        painter.fillRect(QRect(left, area.bottom() - 3, right - left, 4), tint);
        QColor edge = style.accent;
        edge.setAlpha(mark.active ? 235 : 165);
        painter.fillRect(QRect(from, area.bottom() - 4, 2, 5), edge);
        painter.fillRect(QRect(to - 2, area.bottom() - 4, 2, 5), edge);
    }

    // The start of a loop that has been marked but not yet closed: a dashed line with a flag, because
    // a marked start nobody can see is a press that looks like it did nothing.
    if (pendingFrom >= 0.0) {
        const int x = xOfRatio(pendingFrom);
        if (x >= area.left() - 1 && x <= area.right() + 1) {
            QPen dashed(style.marker);
            dashed.setStyle(Qt::DashLine);
            dashed.setWidth(1);
            painter.setPen(dashed);
            painter.drawLine(x, area.top(), x, area.bottom());
            painter.fillRect(QRect(x, area.top(), 7, 6), style.marker);
            painter.fillRect(QRect(x, area.top() + 6, 3, 4), style.marker);
        }
    }

    // Every pad, in its own colour: a mark you cannot tell from the next one is not a cue point, it is
    // graffiti. The label wins over the number when the DJ gave the pad a name.
    for (int slot = 0; slot < cues.size(); ++slot) {
        const CueMark &cue = cues.at(slot);
        const int x = xOfRatio(cue.ratio);
        if (x < area.left() || x > area.right()) {
            continue;
        }
        const QColor colour = cue.color.isValid() ? cue.color : style.marker;
        painter.setPen(colour);
        painter.drawLine(x, area.top(), x, area.bottom());
        painter.fillRect(QRect(x - 1, area.top(), 3, 3), colour);
        // The baseline sits a font height below the top edge, so the glyph is whole: a number cut in
        // half by the strip's border names nothing.
        QFont font = painter.font();
        font.setPixelSize(std::max(5, style.markFont));
        painter.setFont(font);
        painter.drawText(x + 3, area.top() + std::max(5, style.markFont) + 1,
                         cue.label.isEmpty() ? QString::number(slot + 1) : cue.label);
    }

    // The virtual play head of slip (DJP-DECK-007): where the track would be had the deck not jumped or
    // repeated. Dashed and dimmer than the play head, because it is not where the sound is — it is where
    // the sound will be when slip lets go, and the two lines must never be mistaken for each other.
    if (slipRatio >= 0.0 && slipRatio <= 1.0) {
        const int slip = xOfRatio(slipRatio);
        if (slip >= area.left() && slip <= area.right()) {
            QColor dim = style.playhead;
            dim.setAlpha(150);
            QPen pen(dim);
            pen.setStyle(Qt::DashLine);
            pen.setWidth(1);
            painter.setPen(pen);
            painter.setBrush(Qt::NoBrush);
            painter.drawLine(slip, area.top(), slip, area.bottom());
            stats.slip = 1;
        }
    }

    painter.setPen(style.playhead);
    const int playhead = xOfRatio(playheadRatio);
    painter.drawLine(playhead, area.top(), playhead, area.bottom());
    painter.restore();
    return stats;
}
