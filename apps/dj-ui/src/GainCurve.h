// How far a level knob has moved, and what that is worth in decibels.
//
// A gain knob's travel is not its range. The hand turns a pointer; the ear hears decibels. Straight
// through (a linear law) means the knob is as fast at unity as it is at the floor, and near unity that
// is too fast: one pixel of hand on the channel trim was 2,7 dB, which reads as a jump rather than a
// touch. The curve below spends less travel around unity and more near the ends, which is where nobody
// is delicate — a band that is already 40 dB down does not need single-decibel control.
//
// The shape is Signalsmith's `curves::CubicSegmentCurve` (`signalsmith-dsp/curves.h`, MIT): one cubic
// segment between (0, 0) and (1, 1) with an explicit gradient at each end. Position 0 is unity, 1 is the
// end of the range, and the curve maps position to the share of the range that position is worth. With
// the start gradient `m` and the end gradient `2 − m`, that segment reduces to
// `y = m·t + (1 − m)·t²` — soft in the middle, steep at the end, exact at both ends, monotone for any
// `m ≥ 0`. `m = 1` is the straight law this replaces.
#pragma once

#include <QString>

#include "signalsmith-dsp/curves.h"

class GainCurve
{
public:
    GainCurve();

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
    double m_middle = 0.5;
    signalsmith::curves::CubicSegmentCurve<float> m_curve;
};
