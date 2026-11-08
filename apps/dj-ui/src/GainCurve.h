// How far a level knob has moved, and what that is worth in decibels.
//
// A gain knob's travel is not its range. The hand turns a pointer; the ear hears decibels. Straight
// through (a linear law) means the knob is as fast at unity as it is at the floor, and near unity that
// is too fast: one pixel of hand on the channel trim was 2,7 dB, which reads as a jump rather than a
// touch. The curve below spends less travel around unity and more near the ends, which is where nobody
// is delicate — a band that is already 40 dB down does not need single-decibel control.
//
// Two shapes, both from Signalsmith's `curves.h` (MIT).
//
// `cubic` is one `curves::CubicSegmentCurve` between (0, 0) and (1, 1) with an explicit gradient at each
// end. Position 0 is unity, 1 is the end of the range, and the curve maps position to the share of the
// range that position is worth. With the start gradient `m` and the end gradient `2 − m`, that segment
// reduces to `y = m·t + (1 − m)·t²` — soft in the middle, steep at the end, exact at both ends,
// monotone for any `m ≥ 0`. `m = 1` is the straight law this replaces.
//
// `logarithmic` is a `curves::Reciprocal` — the rational curve `(a + bx)/(c + dx)` through three points,
// which is the B-taper a fader wants: a quarter of the travel already takes the level down about ten
// decibels, half of it takes eighteen, and the bottom of the travel moves the last few decibels. A
// fader is not a knob: the interesting range of a mix lives in the top third of its throw, so that
// third has to be the fine one.
//
// Both shapes answer the same question — how much of the range a position is worth, 0 at unity and 1 at
// the far end — so a caller does not care which one is in force. The cubic is soft *around* unity; the
// logarithmic is soft *at the far end*, which is the other way round, and that is the whole difference
// between a knob that is set once and a fader that is ridden.
//
// `linear` is the straight law both of them replace.
#pragma once

#include <QString>

#include "signalsmith-dsp/curves.h"

class GainCurve
{
public:
    /// `middle` is the gradient at unity for the cubic shape: 0.1 for a knob that is set once, 0.5 for a
    /// fader. The faders start on the logarithmic shape, which is the B-taper they want.
    explicit GainCurve(double middle, const QString &kind = QStringLiteral("cubic"));

    /// `kind` is "linear" or "cubic"; `middle` is the gradient at unity, where 1 is straight through.
    /// Returns false and keeps the old law when either argument is out of range.
    bool configure(const QString &kind, double middle);

    QString kind() const { return m_kind; }
    double middle() const { return m_middle; }

    /// Position 0…1 (unity to the end of the range) as a share of the range, 0…1.
    double value(double position) const;

    /// The position that lands on a share of the range — the inverse, for drawing the pointer.
    double position(double share) const;

private:
    QString m_kind = QStringLiteral("cubic");
    double m_middle;
    signalsmith::curves::CubicSegmentCurve<float> m_curve;
    signalsmith::curves::Reciprocal<float> m_reciprocal;
};
