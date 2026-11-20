// Monitoring check: the headphone switch under each deck's meter, and the engine answering it.
//
// Two things are measured, and they are different: where the button is (a hand must find it under the meter of its
// own deck, without the meters moving) and what it does (the engine must report the channel as cued).

#include "Checks.h"

#include <QEventLoop>
#include <QQuickItem>
#include <QQuickWindow>
#include <QTimer>

#include <cmath>
#include <QVariantList>
#include <QVariantMap>

namespace {

/// Wait for the engine to answer: the command goes out over the socket and the snapshot comes back.
void settle(int milliseconds)
{
    QEventLoop loop;
    QTimer::singleShot(milliseconds, &loop, &QEventLoop::quit);
    loop.exec();
}

/// Whether the engine reports this channel as cued.
bool cued(const BridgeClient& bridge, int index)
{
    const QVariantList decks = bridge.snapshot().value(QStringLiteral("decks")).toList();
    if (index >= decks.size()) {
        return false;
    }
    return decks.at(index).toMap().value(QStringLiteral("cue")).toBool();
}

} // namespace

namespace checks {

int runCueCheck(QQmlApplicationEngine& engine, std::unique_ptr<BridgeClient>& bridge)
{
    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
    if (window == nullptr || !bridge) {
        std::fprintf(stderr, "dj-ui: the cue check needs a window and a bridge\n");
        return 1;
    }
    settle(600);
    auto* content = window->contentItem();

    const QStringList ids = {QStringLiteral("A"), QStringLiteral("B")};
    for (const QString& id : ids) {
        auto* meter = checks::findItem(content, QStringLiteral("meter_") + id);
        auto* button = checks::findItem(content, QStringLiteral("headphones_") + id);
        if (meter == nullptr || button == nullptr) {
            std::fprintf(stderr, "dj-ui: deck %s has no meter or no headphone button\n",
                         qPrintable(id));
            return 1;
        }
        // Under the meter, not on it: the button owns the row under the meter now (owner's decision, 20.11).
        const QPointF meterBottom = meter->mapToScene(QPointF(0.0, meter->height()));
        const QPointF buttonCentre = button->mapToScene(QPointF(button->width() / 2.0,
                                                              button->height() / 2.0));
        if (buttonCentre.y() < meterBottom.y()) {
            std::fprintf(stderr,
                         "dj-ui: the headphone button of deck %s stands at %.1f, above the meter's bottom %.1f\n",
                         qPrintable(id), buttonCentre.y(), meterBottom.y());
            return 1;
        }
        // Square, and at least twice the old 16×13: a finger, not a dot.
        if (std::abs(button->width() - button->height()) > 1.0 || button->width() < 28.0) {
            std::fprintf(stderr,
                         "dj-ui: the headphone button of deck %s is %.1f×%.1f; it must be square and at least 28\n",
                         qPrintable(id), button->width(), button->height());
            return 1;
        }
        // Centred under the fader: the strip's centre, not the number's left edge.
        auto* parent = button->parentItem();
        if (parent == nullptr) {
            std::fprintf(stderr, "dj-ui: the headphone button of deck %s has no parent\n", qPrintable(id));
            return 1;
        }
        const QPointF stripCentre = parent->mapToScene(QPointF(parent->width() / 2.0, 0.0));
        if (std::abs(buttonCentre.x() - stripCentre.x()) > 2.0) {
            std::fprintf(stderr,
                         "dj-ui: the headphone button of deck %s stands at %.1f, the strip's centre is %.1f\n",
                         qPrintable(id), buttonCentre.x(), stripCentre.x());
            return 1;
        }
    }

    auto* first = checks::findItem(content, QStringLiteral("headphones_A"));
    if (!QMetaObject::invokeMethod(first, "toggle")) {
        std::fprintf(stderr, "dj-ui: the headphone button has no toggle\n");
        return 1;
    }
    settle(700);
    if (!cued(*bridge, 0)) {
        std::fprintf(stderr, "dj-ui: the engine did not report deck A as cued\n");
        return 1;
    }
    std::fprintf(stderr, "dj-ui: cue on deck A reached the engine and came back on\n");

    if (!QMetaObject::invokeMethod(first, "toggle")) {
        std::fprintf(stderr, "dj-ui: the headphone button has no toggle\n");
        return 1;
    }
    settle(700);
    if (cued(*bridge, 0)) {
        std::fprintf(stderr, "dj-ui: the engine still reports deck A as cued after the second press\n");
        return 1;
    }
    std::fprintf(stderr, "dj-ui: cue on deck A off again, and the engine says so\n");
    std::fprintf(stderr, "dj-ui: all cases passed\n");
    return 0;
}

} // namespace checks
