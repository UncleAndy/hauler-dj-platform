#include "GainCurve.h"

#include <algorithm>
#include <cmath>

GainCurve::GainCurve(double middle, const QString &kind)
{
    configure(kind, middle);
}

bool GainCurve::configure(const QString &kind, double middle)
{
    const bool linear = kind == QStringLiteral("linear");
    const bool cubic = kind == QStringLiteral("cubic");
    const bool logarithmic = kind == QStringLiteral("logarithmic");
    if (!linear && !cubic && !logarithmic) {
        return false;
    }
    if (!(middle > 0.05 && middle <= 1.0)) {
        return false;
    }
    m_kind = linear ? QStringLiteral("linear")
                    : (logarithmic ? QStringLiteral("logarithmic") : QStringLiteral("cubic"));
    m_middle = middle;
    m_curve.clear();
    if (cubic) {
        // A cubic segment from unity to the end of the range, with the gradients the middle asks for.
        // The end takes the rest of the average slope, so the whole range still fits the same travel.
        m_curve.lowGrad = static_cast<float>(middle);
        m_curve.highGrad = static_cast<float>(2.0 - middle);
        m_curve.add(0.0f, 0.0f).add(1.0f, 1.0f);
        m_curve.update(true);
    }
    if (logarithmic) {
        // Three points, in this class's convention (0 is unity, 1 is the far end of the range):
        // (0, 0), (0,5, 0,875), (1, 1) — half the travel has already given up seven eighths of the range.
        // `Reciprocal` puts the hyperbola `1,167t/(0,167 + t)` through them: steep at the start, gentle at
        // the end, no pole inside 0…1. On a fader that measures −10,5 dB at a quarter of the throw and
        // −18 dB at half, which is where a B-taper sits.
        m_reciprocal = signalsmith::curves::Reciprocal<float>(0.0f, 0.875f, 1.0f);
    }
    return true;
}

double GainCurve::value(double position) const
{
    if (!std::isfinite(position)) {
        return 0.0;
    }
    const double clamped = std::clamp(position, 0.0, 1.0);
    if (m_kind == QStringLiteral("linear")) {
        return clamped;
    }
    if (m_kind == QStringLiteral("logarithmic")) {
        return std::clamp(static_cast<double>(m_reciprocal(static_cast<float>(clamped))), 0.0, 1.0);
    }
    return std::clamp(static_cast<double>(m_curve(static_cast<float>(clamped))), 0.0, 1.0);
}

double GainCurve::position(double share) const
{
    if (!std::isfinite(share)) {
        return 0.0;
    }
    const double clamped = std::clamp(share, 0.0, 1.0);
    if (m_kind == QStringLiteral("logarithmic")) {
        // The rational curve inverts in closed form, so the pointer does not need a search.
        return std::clamp(static_cast<double>(m_reciprocal.inverse(static_cast<float>(clamped))), 0.0,
                          1.0);
    }
    if (m_kind == QStringLiteral("linear") || clamped <= 0.0) {
        return clamped;
    }
    if (clamped >= 1.0) {
        return 1.0;
    }
    // The segment is monotone, so a bisection is exact enough to be invisible on screen and cheap enough
    // to run while a hand drags: forty halvings put the answer inside a millionth of the travel.
    double low = 0.0;
    double high = 1.0;
    for (int step = 0; step < 40; ++step) {
        const double middle = 0.5 * (low + high);
        if (value(middle) < clamped) {
            low = middle;
        } else {
            high = middle;
        }
    }
    return 0.5 * (low + high);
}
