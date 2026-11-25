// The effect block's place in a strip, measured before any control is added to it.
//
// The strip is full: a letter, a gain knob, three EQ knobs, the meter that carries the fader, and the row under
// the meter where the headphone switch lives. Anything new is paid for by the meter, so the price is measured
// before it is spent — and the one thing the block must never do is move the meters off the line the master's
// meter ends on (owner's decision, 21.11).

#include "Checks.h"

#include "BridgeClient.h"

#include <QEventLoop>
#include <QPointF>
#include <QQuickItem>
#include <QQuickWindow>
#include <QTimer>

#include <cmath>
#include <cstdio>

namespace {

void settle(int milliseconds)
{
    QEventLoop loop;
    QTimer::singleShot(milliseconds, &loop, &QEventLoop::quit);
    loop.exec();
}

/// Where an item ends, in window coordinates: the bottom is what the meters share, whatever the parents do.
double bottomOf(QQuickItem* item)
{
    return item->mapToScene(QPointF(0.0, item->height())).y();
}

} // namespace

namespace checks {

int runFxCheck(QQmlApplicationEngine& engine, std::unique_ptr<BridgeClient>& bridge)
{
    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
    if (window == nullptr || !bridge) {
        std::fprintf(stderr, "dj-ui: the fx check needs a window and a bridge\n");
        return 1;
    }
    // The strips are built from the engine's decks: without them there is no strip to measure.
    settle(700);
    auto* content = window->contentItem();

    auto* strip = checks::findItem(content, QStringLiteral("strip_A"));
    auto* meter = checks::findItem(content, QStringLiteral("meter_A"));
    // `MasterMeter.qml` names itself `masterMeter`; the instance must not rename it, or the check that measures
    // where the master column sits loses its landmark (found the hard way, 21.11).
    auto* masterMeter = checks::findItem(content, QStringLiteral("masterMeter"));
    auto* masterColumn = checks::findItem(content, QStringLiteral("masterColumn"));
    if (strip == nullptr || meter == nullptr || masterMeter == nullptr || masterColumn == nullptr) {
        std::fprintf(stderr, "dj-ui: no strip, no meter or no master column to measure\n");
        return 1;
    }

    const double stripTop = strip->mapToScene(QPointF(0.0, 0.0)).y();
    std::fprintf(stdout, "dj-ui: strip A is %.1f wide and %.1f high\n", strip->width(), strip->height());
    std::fprintf(stdout, "dj-ui: the meter of A runs %.1f … %.1f (%.1f px)\n",
                 meter->mapToScene(QPointF(0.0, 0.0)).y(), bottomOf(meter), meter->height());
    std::fprintf(stdout, "dj-ui: the meter of the master runs %.1f … %.1f (%.1f px)\n",
                 masterMeter->mapToScene(QPointF(0.0, 0.0)).y(), bottomOf(masterMeter), masterMeter->height());
    std::fprintf(stdout, "dj-ui: the strip has %.1f px above its meter (from %.1f)\n",
                 meter->mapToScene(QPointF(0.0, 0.0)).y() - stripTop, stripTop);
    std::fprintf(stdout, "dj-ui: the master column is %.1f high, its meter starts %.1f px down\n",
                 masterColumn->height(),
                 masterMeter->mapToScene(QPointF(0.0, 0.0)).y() - masterColumn->mapToScene(QPointF(0.0, 0.0)).y());

    // The invariant the block is going to be paid for: the deck's meter and the master's end on one line.
    const double deckBottom = bottomOf(meter);
    const double masterBottom = bottomOf(masterMeter);
    if (std::abs(deckBottom - masterBottom) > 0.5) {
        std::fprintf(stderr,
                     "dj-ui: the deck meter ends at %.1f and the master's at %.1f — the meters no longer share a "
                     "line\n",
                     deckBottom, masterBottom);
        return 1;
    }
    std::fprintf(stdout, "dj-ui: the meters still end on one line (y %.1f)\n", deckBottom);

    // The other half of the budget: width. A knob and a button side by side need roughly twice the strip's
    // width, so what matters is whether the mixer has room to hand out or the room is already spoken for.
    auto* row = checks::findItem(content, QStringLiteral("stripRow"));
    int strips = 0;
    for (const QString& id : {QStringLiteral("A"), QStringLiteral("B"), QStringLiteral("C"), QStringLiteral("D")}) {
        if (checks::findItem(content, QStringLiteral("strip_") + id) != nullptr) {
            ++strips;
        }
    }
    std::fprintf(stdout, "dj-ui: the window is %.1f wide and the strip row %.1f, holding %d strips\n",
                 static_cast<double>(window->width()), row != nullptr ? row->width() : -1.0, strips);
    // The knob the block will copy: the EQ knob's diameter is the size a strip can hold today.
    double knobSide = -1.0;
    if (auto* eqKnob = checks::findItem(content, QStringLiteral("trim_A")); eqKnob != nullptr) {
        knobSide = eqKnob->width();
    }
    std::fprintf(stdout, "dj-ui: a strip knob is %.1f px across, the meter height setting is %s\n", knobSide,
                 meter != nullptr ? qPrintable(QString::number(meter->height(), 'f', 1)) : "-");

    // The budget the block will spend: what is left between the foot of the EQ block and the top of the meter.
    auto* eqStack = checks::findItem(content, QStringLiteral("eqStack_A"));
    if (eqStack != nullptr) {
        const double eqFoot = bottomOf(eqStack);
        std::fprintf(stdout, "dj-ui: the EQ block of A ends at %.1f, the meter starts at %.1f: %.1f px spare\n",
                     eqFoot, meter->mapToScene(QPointF(0.0, 0.0)).y(),
                     meter->mapToScene(QPointF(0.0, 0.0)).y() - eqFoot);
    } else {
        std::fprintf(stdout, "dj-ui: the EQ block of A has no name of its own, so the spare height is the meter's "
                             "top minus the strip's top (see above)\n");
    }

    // The three controls of the block, and what each of them does to the engine.
    auto* knob = checks::findItem(content, QStringLiteral("fxKnob_A"));
    auto* switchItem = checks::findItem(content, QStringLiteral("fx_A"));
    auto* picker = checks::findItem(content, QStringLiteral("fxPick_A"));
    if (knob == nullptr || switchItem == nullptr || picker == nullptr) {
        std::fprintf(stderr, "dj-ui: the strip of A has no effect block (knob %s, switch %s, chooser %s)\n",
                     knob != nullptr ? "yes" : "missing", switchItem != nullptr ? "yes" : "missing",
                     picker != nullptr ? "yes" : "missing");
        return 1;
    }
    std::fprintf(stdout, "dj-ui: the effect block stands under the EQ, the meter keeps %.1f px\n", meter->height());

    // The knob's travel, both halves: the pointer straight up is no effect at all, and its two ends are the far ends of
    // the two effects the unit does — one to the left, one to the right (owner's request, 25.11).
    const auto askAndRead = [&](double decibels, const char* what) {
        if (!QMetaObject::invokeMethod(knob, "ask", Q_ARG(QVariant, decibels))) {
            std::fprintf(stderr, "dj-ui: the effect knob takes no value (%s)\n", what);
            return false;
        }
        settle(500);
        return true;
    };
    const auto angleOf = [&knob]() { return knob->property("angle").toDouble(); };
    if (!askAndRead(-6.0, "bottom of the left half")) {
        return 1;
    }
    const double leftAngle = angleOf();
    if (leftAngle > -125.0) {
        std::fprintf(stderr, "dj-ui: the knob at its bottom-left stands at %.1f degrees, not near −135\n",
                     leftAngle);
        return 1;
    }
    if (!askAndRead(6.0, "bottom of the right half")) {
        return 1;
    }
    const double rightAngle = angleOf();
    if (rightAngle < 125.0) {
        std::fprintf(stderr, "dj-ui: the knob at its bottom-right stands at %.1f degrees, not near +135\n",
                     rightAngle);
        return 1;
    }
    std::fprintf(stdout, "dj-ui: the effect knob turns through both halves: %.0f … %.0f degrees\n", leftAngle,
                 rightAngle);

    // And the amount: straight up is zero, and −3 dB of the travel is halfway into the left-hand effect — the engine
    // has to say both.
    if (!askAndRead(0.0, "the centre")) {
        return 1;
    }
    const auto patchOf = [&bridge]() {
        const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
        if (decks.isEmpty()) {
            return QVariantMap();
        }
        return decks.at(0).toMap().value(QStringLiteral("fx")).toMap();
    };
    const double centre = patchOf().value(QStringLiteral("amount")).toDouble();
    if (std::abs(centre) > 0.02) {
        std::fprintf(stderr,
                     "dj-ui: the effect knob straight up is not zero: the engine has %.3f\n",
                     centre);
        return 1;
    }
    if (!askAndRead(-3.0, "halfway into the left-hand effect")) {
        return 1;
    }
    double amount = patchOf().value(QStringLiteral("amount")).toDouble();
    if (std::abs(amount + 0.5) > 0.05) {
        std::fprintf(stderr,
                     "dj-ui: the effect knob turned halfway left and the engine has %.3f, not −0.5\n",
                     amount);
        return 1;
    }
    std::fprintf(stdout,
                 "dj-ui: the effect knob reached the engine: straight up %.2f, halfway left %.2f\n",
                 centre, amount);

    // The switch: pressed once, the engine has the deck's effect in the path.
    if (!QMetaObject::invokeMethod(switchItem, "toggle")) {
        std::fprintf(stderr, "dj-ui: the effect switch has no toggle\n");
        return 1;
    }
    settle(500);
    if (!patchOf().value(QStringLiteral("enabled")).toBool()) {
        std::fprintf(stderr, "dj-ui: the effect switch did not reach the engine\n");
        return 1;
    }
    std::fprintf(stdout, "dj-ui: the switch put the effect in the path\n");

    // The chooser: the menu's own function, so the check walks the path a hand walks.
    if (!QMetaObject::invokeMethod(picker, "pick", Q_ARG(QVariant, QStringLiteral("echo")))) {
        std::fprintf(stderr, "dj-ui: the effect chooser has no pick\n");
        return 1;
    }
    settle(500);
    const QString kind = patchOf().value(QStringLiteral("kind")).toString();
    if (kind != QStringLiteral("echo")) {
        std::fprintf(stderr, "dj-ui: the choir asked for echo and the engine plays \"%s\"\n", qPrintable(kind));
        return 1;
    }
    std::fprintf(stdout, "dj-ui: the chooser put echo in the slot, and the engine says so\n");

    // A roll is held, not latched: pressing puts the loop in the path, releasing gives the live signal back
    // (owner's decision 21.11 — a latched roll repeats a short cut of the beat for as long as nobody remembers to
    // press it again, which is exactly what the owner heard as "constant very short loops").
    if (!QMetaObject::invokeMethod(picker, "pick", Q_ARG(QVariant, QStringLiteral("roll")))) {
        return 1;
    }
    settle(400);
    if (!QMetaObject::invokeMethod(switchItem, "press")) {
        std::fprintf(stderr, "dj-ui: the effect switch has no press\n");
        return 1;
    }
    settle(400);
    if (!patchOf().value(QStringLiteral("enabled")).toBool()) {
        std::fprintf(stderr, "dj-ui: holding the roll did not put the loop in the path\n");
        return 1;
    }
    if (!QMetaObject::invokeMethod(switchItem, "release")) {
        std::fprintf(stderr, "dj-ui: the effect switch has no release\n");
        return 1;
    }
    settle(400);
    if (patchOf().value(QStringLiteral("enabled")).toBool()) {
        std::fprintf(stderr, "dj-ui: the roll stayed latched after the hand let go\n");
        return 1;
    }
    std::fprintf(stdout, "dj-ui: the roll is held, not latched: down puts the loop in, up gives the deck back\n");

    // Back to echo for the last press: the switch itself, once more, out of the path.
    if (!QMetaObject::invokeMethod(picker, "pick", Q_ARG(QVariant, QStringLiteral("echo")))) {
        return 1;
    }
    settle(300);
    // And the switch once more: it must turn the deck's effect around, whatever state the hand left it in.
    const bool before = patchOf().value(QStringLiteral("enabled")).toBool();
    if (!QMetaObject::invokeMethod(switchItem, "toggle")) {
        return 1;
    }
    settle(500);
    if (patchOf().value(QStringLiteral("enabled")).toBool() == before) {
        std::fprintf(stderr, "dj-ui: the switch did not turn the effect around\n");
        return 1;
    }
    std::fprintf(stdout, "dj-ui: all cases passed\n");
    return 0;
}

} // namespace checks
