#include "GainCurve.h"

#include <algorithm>
#include <cmath>

GainCurve::GainCurve()
{
    configure(QStringLiteral("cubic"), 0.5);
}

bool GainCurve::configure(const QString &kind, double middle)
{
    const bool linear = kind == QStringLiteral("linear");
    if (!linear && kind != QStringLiteral("cubic")) {
        return false;
    }
    if (!(middle > 0.05 && middle <= 1.0)) {
        return false;
    }
    m_kind = linear ? QStringLiteral("linear") : QStringLiteral("cubic");
    m_middle = middle;
    m_curve.clear();
    if (!linear) {
        // A cubic segment from unity to the end of the range, with the gradients the middle asks for.
        // The end takes the rest of the average slope, so the whole range still fits the same travel.
        m_curve.lowGrad = static_cast<float>(middle);
        m_curve.highGrad = static_cast<float>(2.0 - middle);
        m_curve.add(0.0f, 0.0f).add(1.0f, 1.0f);
        m_curve.update(true);
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
    return std::clamp(static_cast<double>(m_curve(static_cast<float>(clamped))), 0.0, 1.0);
}

double GainCurve::position(double share) const
{
    if (!std::isfinite(share)) {
        return 0.0;
    }
    const double clamped = std::clamp(share, 0.0, 1.0);
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
