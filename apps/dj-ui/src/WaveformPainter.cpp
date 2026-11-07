#include "WaveformPainter.h"

#include "Waveform.h"

#include <QCoreApplication>

#include <algorithm>
#include <cmath>

void paintWaveform(QPainter &painter, const Waveform &waveform, const WaveformStyle &style,
                   const QRect &area, double windowStart, double span, double playheadRatio,
                   const QList<CueMark> &cues, bool hasLoop, double loopFrom, double loopTo)
{
    if (area.width() <= 0 || area.height() <= 0) {
        return;
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

    if (hasLoop && loopTo > loopFrom) {
        const int from = xOfRatio(loopFrom);
        const int to = xOfRatio(loopTo);
        if (to > from) {
            const int left = std::max(area.left(), from);
            const int right = std::min(area.right() + 1, to);
            if (right > left) {
                QColor tint = style.accent;
                tint.setAlpha(36);
                painter.fillRect(QRect(left, area.top(), right - left, height), tint);
            }
            painter.setPen(style.accent);
            painter.drawLine(from, area.top(), from, area.bottom());
            painter.drawLine(to, area.top(), to, area.bottom());
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

    painter.setPen(style.playhead);
    const int playhead = xOfRatio(playheadRatio);
    painter.drawLine(playhead, area.top(), playhead, area.bottom());
    painter.restore();
}
