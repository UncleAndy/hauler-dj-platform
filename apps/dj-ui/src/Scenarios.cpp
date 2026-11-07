// Scenario checks for the DJ shell (DJP-DECK-003/004 and the UI around them).
//
// Every case is a user story driven through the real widgets: the button is clicked, the strips are read
// through their own properties, and the facts come from the engine's snapshot. A case that cannot see
// what it is looking for fails with a sentence saying what it expected, so a refusal is never silent.

#include "Scenarios.h"

#include "BridgeClient.h"

#include <QCoreApplication>
#include <QElapsedTimer>
#include <QEventLoop>
#include <QFile>
#include <QMouseEvent>
#include <QQuickItem>
#include <QQuickWindow>
#include <QString>
#include <QThread>
#include <QVariantList>
#include <QVariantMap>

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <functional>
#include <vector>

namespace djui {
namespace {

QQuickItem *findItem(QQuickItem *root, const QString &name)
{
    if (root == nullptr) {
        return nullptr;
    }
    if (root->objectName() == name) {
        return root;
    }
    for (QQuickItem *child : root->childItems()) {
        if (QQuickItem *found = findItem(child, name)) {
            return found;
        }
    }
    return nullptr;
}

void pump(int rounds)
{
    for (int step = 0; step < rounds; ++step) {
        QCoreApplication::processEvents(QEventLoop::AllEvents, 10);
        QThread::msleep(5);
    }
}

bool waitFor(const std::function<bool()> &condition, int timeout_ms)
{
    QElapsedTimer clock;
    clock.start();
    while (clock.elapsed() < timeout_ms) {
        if (condition()) {
            return true;
        }
        pump(1);
    }
    return condition();
}

/// Write a stereo 48 kHz sine of the given amplitude: a track whose level is exactly what a case needs.
///
/// The auto-gain cases cannot use the shared fixture: its correction is large enough that the old shell
/// showed it too, so a defect in the quiet half would go unnoticed.
QString writeTone(const QString &path, double amplitude, double seconds)
{
    const int rate = 48000;
    const int frames = int(rate * seconds);
    QByteArray data;
    data.reserve(frames * 4);
    for (int i = 0; i < frames; ++i) {
        const double value = amplitude * std::sin(2.0 * M_PI * 440.0 * i / rate);
        const auto sample = static_cast<qint16>(std::lround(value * 32767.0));
        for (int channel = 0; channel < 2; ++channel) {
            data.append(char(sample & 0xff));
            data.append(char((sample >> 8) & 0xff));
        }
    }
    QByteArray header;
    const auto put32 = [&header](quint32 value) {
        header.append(char(value & 0xff));
        header.append(char((value >> 8) & 0xff));
        header.append(char((value >> 16) & 0xff));
        header.append(char((value >> 24) & 0xff));
    };
    const auto put16 = [&header](quint16 value) {
        header.append(char(value & 0xff));
        header.append(char((value >> 8) & 0xff));
    };
    header.append("RIFF");
    put32(quint32(36 + data.size()));
    header.append("WAVEfmt ");
    put32(16);
    put16(1);
    put16(2);
    put32(quint32(rate));
    put32(quint32(rate * 4));
    put16(4);
    put16(16);
    header.append("data");
    put32(quint32(data.size()));
    QFile file(path);
    if (!file.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
        return QString();
    }
    file.write(header);
    file.write(data);
    file.close();
    return path;
}

/// What a case says when it fails.
using Reason = QString;

/// One deck, seen through the bridge and the widgets that draw it.
class Runner
{
public:
    Runner(BridgeClient &bridge, QQuickWindow &window, QString deck, QString fixture)
        : m_bridge(bridge)
        , m_window(window)
        , m_deck(std::move(deck))
        , m_fixture(std::move(fixture))
    {
    }

    /// Load the same file again, as a DJ would after closing and reopening the deck.
    void reload()
    {
        m_bridge.play(m_deck, false);
        m_bridge.load(m_deck, m_fixture);
    }

    /// Put a specific file on the deck.
    void load(const QString &path)
    {
        m_bridge.play(m_deck, false);
        m_bridge.load(m_deck, path);
    }

    /// Empty the deck: what the strips paint has to come from the file, not from what was on screen.
    void eject()
    {
        m_bridge.play(m_deck, false);
        m_bridge.eject(m_deck);
        pump(40);
        waitFor([this] { return total() <= 0.0; }, 3000);
    }

    // ---- facts from the engine ---------------------------------------------------------------
    QVariantMap deck() const
    {
        const QVariantList decks = m_bridge.snapshot().value(QStringLiteral("decks")).toList();
        for (const QVariant &entry : decks) {
            const QVariantMap map = entry.toMap();
            if (map.value(QStringLiteral("deck")).toString() == m_deck) {
                return map;
            }
        }
        return {};
    }

    double frames() const { return deck().value(QStringLiteral("source_frame")).toDouble(); }

    bool playing() const { return deck().value(QStringLiteral("playing")).toBool(); }

    double total() const
    {
        const QVariantMap state = deck();
        const QVariant left = state.value(QStringLiteral("remaining_frames"));
        if (!left.isValid()) {
            return 0.0;
        }
        return state.value(QStringLiteral("source_frame")).toDouble() + left.toDouble();
    }

    int armed() const { return int(deck().value(QStringLiteral("active_loops")).toList().size()); }

    bool hasLoopRegion() const
    {
        const QVariant value = deck().value(QStringLiteral("loop_region"));
        return value.isValid() && !value.isNull();
    }

    QVariantMap loopRegion() const { return deck().value(QStringLiteral("loop_region")).toMap(); }

    QVariant padLoop(int slot) const
    {
        const QVariantList loops = deck().value(QStringLiteral("pad_loops")).toList();
        return slot < loops.size() ? loops.at(slot) : QVariant();
    }

    QVariant hotcue(int slot) const
    {
        const QVariantList cues = deck().value(QStringLiteral("hotcues")).toList();
        return slot < cues.size() ? cues.at(slot) : QVariant();
    }

    QString cueLabel(int slot) const
    {
        const QVariantList labels = deck().value(QStringLiteral("cue_labels")).toList();
        return slot < labels.size() ? labels.at(slot).toString() : QString();
    }

    QString cueColor(int slot) const
    {
        const QVariantList colors = deck().value(QStringLiteral("cue_colors")).toList();
        return slot < colors.size() ? colors.at(slot).toString() : QString();
    }

    // ---- facts from the widgets --------------------------------------------------------------
    QQuickItem *item(const QString &name) const { return findItem(m_window.contentItem(), name); }

    double property(const QString &name, const char *key) const
    {
        QQuickItem *found = item(name);
        return found == nullptr ? -1.0 : found->property(key).toDouble();
    }

    bool flag(const QString &name, const char *key) const
    {
        QQuickItem *found = item(name);
        return found != nullptr && found->property(key).toBool();
    }

    QVariantList list(const QString &name, const char *key) const
    {
        QQuickItem *found = item(name);
        return found == nullptr ? QVariantList() : found->property(key).toList();
    }

    QVariantList stripCues() const { return list(stripName(), "cues"); }
    QVariantList stripLoops() const { return list(stripName(), "padLoops"); }
    bool stripHasLoop() const { return flag(stripName(), "hasLoop"); }
    bool stripLoopActive() const { return flag(stripName(), "loopActive"); }
    double stripPending() const { return property(stripName(), "pendingFrom"); }

    QString deckId() const { return m_deck; }
    QString stripName() const { return QStringLiteral("waveStrip_") + m_deck; }
    QString padName(int slot) const { return QStringLiteral("cuepad_%1_%2").arg(m_deck).arg(slot); }
    QString loopButtonName() const { return QStringLiteral("loop_") + m_deck; }

    int storedLoops() const
    {
        int count = 0;
        for (int slot = 0; slot < 8; ++slot) {
            const QVariant loop = padLoop(slot);
            if (loop.isValid() && !loop.isNull()) {
                ++count;
            }
        }
        return count;
    }

    int armedMarks() const
    {
        int lit = 0;
        for (const QVariant &entry : stripLoops()) {
            if (entry.toMap().value(QStringLiteral("active")).toBool()) {
                ++lit;
            }
        }
        return lit;
    }

    // ---- commands, so a case can set up its own world --------------------------------------
    void seek(double frame) { m_bridge.seek(m_deck, qulonglong(frame)); }
    void play(bool enabled) { m_bridge.play(m_deck, enabled); }
    void save(int slot) { m_bridge.setPadLoop(m_deck, slot); }
    void label(int slot)
    {
        m_bridge.setCueMeta(m_deck, slot, QStringLiteral("Drop"), QStringLiteral("#5fb85f"));
    }

    // ---- gestures --------------------------------------------------------------------------
    bool click(const QString &name, Qt::KeyboardModifiers modifiers = Qt::NoModifier)
    {
        QQuickItem *target = item(name);
        if (target == nullptr) {
            return false;
        }
        const QPointF at = target->mapToScene(QPointF(target->width() / 2.0, target->height() / 2.0));
        for (const QEvent::Type type : {QEvent::MouseButtonPress, QEvent::MouseButtonRelease}) {
            QMouseEvent event(type, at, m_window.mapToGlobal(at.toPoint()), Qt::LeftButton,
                              Qt::LeftButton, modifiers);
            QCoreApplication::sendEvent(&m_window, &event);
            QCoreApplication::processEvents();
        }
        pump(30);
        return true;
    }

    bool pressLoop(Qt::KeyboardModifiers modifiers = Qt::NoModifier)
    {
        return click(loopButtonName(), modifiers);
    }

    bool pressPad(int slot, Qt::KeyboardModifiers modifiers = Qt::NoModifier)
    {
        return click(padName(slot), modifiers);
    }

    bool loopPending() const { return flag(loopButtonName(), "pending"); }
    bool loopButtonLit() const { return flag(loopButtonName(), "active"); }

    /// Cancel a pair that is waiting for its second press.
    void cancelPending()
    {
        if (!loopPending()) {
            return;
        }
        pressLoop(Qt::ShiftModifier);
        waitFor([this] { return !loopPending(); }, 1500);
    }

    /// A deck with no loop, no pads and a playhead at zero.
    void reset()
    {
        // A pair left waiting by the previous case would swallow the next case's first press.
        cancelPending();
        m_bridge.setLoopMode(m_deck, false);
        m_bridge.setLoop(m_deck, false, 0, 0);
        for (int slot = 0; slot < 8; ++slot) {
            m_bridge.cue(m_deck, QStringLiteral("Delete"), slot);
        }
        m_bridge.play(m_deck, false);
        m_bridge.seek(m_deck, 0);
        pump(40);
        waitFor([this] { return armed() == 0 && !hasLoopRegion() && !loopPending(); }, 3000);
    }

    /// Mark a start at `from`, then close the loop at `to`.
    bool makeLoop(double from, double to, Reason &reason)
    {
        play(false);
        seek(from);
        if (!waitFor([this, from] { return std::abs(frames() - from) < 2000.0; }, 3000)) {
            reason = QStringLiteral("the deck did not reach frame %1").arg(from);
            return false;
        }
        if (!pressLoop()) {
            reason = QStringLiteral("the loop button is missing");
            return false;
        }
        if (!loopPending()) {
            reason = QStringLiteral("the first press did not mark a start");
            return false;
        }
        seek(to);
        if (!waitFor([this, to] { return std::abs(frames() - to) < 2000.0; }, 3000)) {
            reason = QStringLiteral("the deck did not reach frame %1").arg(to);
            return false;
        }
        pressLoop();
        return true;
    }

    /// Loop positions as fractions of the track, so a case does not depend on the fixture's length.
    void span(double &first, double &second, double &third, double &fourth) const
    {
        const double length = total() > 0.0 ? total() : 144000.0;
        first = std::round(length * 0.10);
        second = std::round(length * 0.20);
        third = std::round(length * 0.30);
        fourth = std::round(length * 0.40);
    }

    bool waitFrames(double target, int timeout_ms = 3000)
    {
        return waitFor([this, target] { return std::abs(frames() - target) < 2000.0; }, timeout_ms);
    }

private:
    BridgeClient &m_bridge;
    QQuickWindow &m_window;
    QString m_deck;
    QString m_fixture;
};

struct Case
{
    const char *name;
    std::function<bool(Runner &, Reason &)> body;
};

} // namespace

int runScenarios(BridgeClient &bridge, QQuickWindow &window, const QString &deck,
                 const QString &fixture)
{
    Runner runner(bridge, window, deck, fixture);
    if (!waitFor([&]() { return runner.total() > 0.0; }, 5000)) {
        std::fprintf(stderr, "dj-ui: the deck never reported a track\n");
        return 1;
    }

    // A cold start, which is what a DJ meets when opening a track from yesterday: a file that remembers
    // loops must show them before anything is clicked.
    const int remembered = runner.storedLoops();
    if (remembered > 0) {
        const bool painted = waitFor([&] { return int(runner.stripLoops().size()) >= remembered; }, 4000);
        std::fprintf(stdout, "scenario ok: a cold start paints %d remembered loop(s): %s\n", remembered,
                     painted ? "yes" : "no");
        if (!painted) {
            std::fprintf(stderr,
                         "scenario FAILED: the track remembers %d loop(s) but the strip paints %d\n",
                         remembered, int(runner.stripLoops().size()));
            return 1;
        }
    }

    std::vector<Case> cases;

    cases.push_back({"the clock says how much the track's level was corrected",
                     [](Runner &run, Reason &why) {
                         run.reload();
                         if (!waitFor([&] { return run.total() > 0.0; }, 6000)) {
                             why = QStringLiteral("the track did not load");
                             return false;
                         }
                         const double gain = run.deck().value(QStringLiteral("auto_gain")).toDouble();
                         if (!(gain > 0.0)) {
                             why = QStringLiteral("the snapshot carries no auto_gain");
                             return false;
                         }
                         const double db = 20.0 * std::log10(gain);
                         QQuickItem *label = run.item(QStringLiteral("autogain_") + run.deckId());
                         if (label == nullptr) {
                             why = QStringLiteral("there is no correction label beside the clock");
                             return false;
                         }
                         if (std::abs(db) >= 0.5 && !label->isVisible()) {
                             why = QStringLiteral("the correction is %1 dB but the label is hidden")
                                       .arg(db, 0, 'f', 1);
                             return false;
                         }
                         const QString text = label->property("text").toString();
                         // The label writes the sign as a typographic minus and the magnitude as a
                         // number: compare the number, not the character.
                         if (std::abs(db) >= 0.5
                             && !text.contains(QString::number(std::abs(db), 'f', 1))) {
                             why = QStringLiteral("the label reads \"%1\" for %2 dB")
                                       .arg(text, QString::number(db, 'f', 1));
                             return false;
                         }
                         return true;
                     }});


    // ---- marking and closing a loop ----------------------------------------------------------
    cases.push_back({"a press with no loop marks a start, and only that",
                     [](Runner &run, Reason &why) {
                         double a = 0.0;
                         double b = 0.0;
                         double c = 0.0;
                         double d = 0.0;
                         run.span(a, b, c, d);
                         run.seek(a);
                         run.waitFrames(a);
                         if (!run.pressLoop()) {
                             why = QStringLiteral("the loop button is missing");
                             return false;
                         }
                         if (!run.loopPending()) {
                             why = QStringLiteral("the first press did not start a pair");
                             return false;
                         }
                         if (run.stripPending() < 0.0) {
                             why = QStringLiteral("no start marker appeared on the strip");
                             return false;
                         }
                         if (run.hasLoopRegion() || run.armed() != 0) {
                             why = QStringLiteral("the first press already armed a loop");
                             return false;
                         }
                         return true;
                     }});

    cases.push_back({"the second press closes the loop, arms it and lights the button",
                     [](Runner &run, Reason &why) {
                         double a = 0.0;
                         double b = 0.0;
                         double c = 0.0;
                         double d = 0.0;
                         run.span(a, b, c, d);
                         if (!run.makeLoop(a, b, why)) {
                             return false;
                         }
                         if (run.armed() != 1) {
                             why = QStringLiteral("the loop is armed %1 times").arg(run.armed());
                             return false;
                         }
                         if (!run.loopButtonLit()) {
                             why = QStringLiteral("the loop button did not light up");
                             return false;
                         }
                         if (!run.stripLoopActive()) {
                             why = QStringLiteral("the strip does not paint the loop as active");
                             return false;
                         }
                         const double start = run.loopRegion().value(QStringLiteral("start")).toDouble();
                         if (std::abs(start - a) > 2000.0) {
                             why = QStringLiteral("the loop starts at %1 instead of %2").arg(start).arg(a);
                             return false;
                         }
                         if (std::abs(run.frames() - a) > 12000.0) {
                             why = QStringLiteral("the playhead was not taken to the loop's start");
                             return false;
                         }
                         return true;
                     }});

    cases.push_back({"playing crosses the loop's end and wraps inside it",
                     [](Runner &run, Reason &why) {
                         double a = 0.0;
                         double b = 0.0;
                         double c = 0.0;
                         double d = 0.0;
                         run.span(a, b, c, d);
                         if (!run.makeLoop(a, b, why)) {
                             return false;
                         }
                         const double end = run.loopRegion().value(QStringLiteral("end")).toDouble();
                         run.play(true);
                         pump(30);
                         double peak = run.frames();
                         for (int step = 0; step < 12; ++step) {
                             pump(6);
                             peak = std::max(peak, run.frames());
                         }
                         run.play(false);
                         if (peak > end + 12000.0) {
                             why = QStringLiteral("the playhead ran past the loop: %1 > %2")
                                       .arg(peak)
                                       .arg(end);
                             return false;
                         }
                         return true;
                     }});

    // ---- the button as a mode switch ---------------------------------------------------------
    cases.push_back({"shift switches the mode off and leaves the sound alone",
                     [](Runner &run, Reason &why) {
                         double a = 0.0;
                         double b = 0.0;
                         double c = 0.0;
                         double d = 0.0;
                         run.span(a, b, c, d);
                         if (!run.makeLoop(a, b, why)) {
                             return false;
                         }
                         run.play(false);
                         pump(20);
                         const double before = run.frames();
                         run.pressLoop(Qt::ShiftModifier);
                         if (run.armed() != 0) {
                             why = QStringLiteral("the mode is still armed");
                             return false;
                         }
                         if (run.loopButtonLit()) {
                             why = QStringLiteral("the button stayed lit");
                             return false;
                         }
                         if (std::abs(run.frames() - before) > 64.0) {
                             why = QStringLiteral("the playhead moved: %1 -> %2").arg(before).arg(run.frames());
                             return false;
                         }
                         if (!run.stripHasLoop() || run.stripLoopActive()) {
                             why = QStringLiteral("the loop lost its edges or kept its fill");
                             return false;
                         }
                         return true;
                     }});

    cases.push_back({"shift switches the mode back on without moving the playhead",
                     [](Runner &run, Reason &why) {
                         double a = 0.0;
                         double b = 0.0;
                         double c = 0.0;
                         double d = 0.0;
                         run.span(a, b, c, d);
                         if (!run.makeLoop(a, b, why)) {
                             return false;
                         }
                         run.pressLoop(Qt::ShiftModifier);
                         const double before = run.frames();
                         run.pressLoop(Qt::ShiftModifier);
                         if (run.armed() == 0) {
                             why = QStringLiteral("the mode did not come back on");
                             return false;
                         }
                         if (std::abs(run.frames() - before) > 64.0) {
                             why = QStringLiteral("the playhead moved: %1 -> %2").arg(before).arg(run.frames());
                             return false;
                         }
                         return true;
                     }});

    cases.push_back({"shift cancels a pending pair and does not touch the mode",
                     [](Runner &run, Reason &why) {
                         double a = 0.0;
                         double b = 0.0;
                         double c = 0.0;
                         double d = 0.0;
                         run.span(a, b, c, d);
                         if (!run.makeLoop(c, d, why)) {
                             return false;
                         }
                         run.pressLoop(Qt::ShiftModifier);
                         run.seek(a);
                         run.waitFrames(a);
                         run.pressLoop();
                         if (!run.loopPending()) {
                             why = QStringLiteral("the press did not start a pair");
                             return false;
                         }
                         const int armedBefore = run.armed();
                         run.pressLoop(Qt::ShiftModifier);
                         if (run.loopPending()) {
                             why = QStringLiteral("shift did not cancel the pair");
                             return false;
                         }
                         if (run.armed() != armedBefore) {
                             why = QStringLiteral("shift changed the mode while cancelling a pair");
                             return false;
                         }
                         return true;
                     }});

    cases.push_back({"a press inside an armed loop does not move the playhead",
                     [](Runner &run, Reason &why) {
                         double a = 0.0;
                         double b = 0.0;
                         double c = 0.0;
                         double d = 0.0;
                         run.span(a, b, c, d);
                         if (!run.makeLoop(a, b, why)) {
                             return false;
                         }
                         run.pressLoop(Qt::ShiftModifier);
                         const double inside = (a + b) / 2.0;
                         run.seek(inside);
                         if (!run.waitFrames(inside)) {
                             why = QStringLiteral("the deck did not reach the middle of the loop");
                             return false;
                         }
                         const double before = run.frames();
                         run.pressLoop();
                         if (run.armed() == 0) {
                             why = QStringLiteral("the press did not switch the mode on");
                             return false;
                         }
                         if (std::abs(run.frames() - before) > 64.0) {
                             why = QStringLiteral("the press moved the playhead: %1 -> %2")
                                       .arg(before)
                                       .arg(run.frames());
                             return false;
                         }
                         return true;
                     }});

    cases.push_back({"a press outside an armed loop starts another loop instead",
                     [](Runner &run, Reason &why) {
                         double a = 0.0;
                         double b = 0.0;
                         double c = 0.0;
                         double d = 0.0;
                         run.span(a, b, c, d);
                         if (!run.makeLoop(c, d, why)) {
                             return false;
                         }
                         run.seek(a);
                         if (!run.waitFrames(a)) {
                             why = QStringLiteral("the deck did not reach the point before the loop");
                             return false;
                         }
                         const double before = run.frames();
                         run.pressLoop();
                         if (!run.loopPending()) {
                             why = QStringLiteral("the press outside the loop did not mark a start");
                             return false;
                         }
                         if (run.armed() != 0) {
                             why = QStringLiteral("the old loop is still armed while a new one is marked");
                             return false;
                         }
                         if (std::abs(run.frames() - before) > 64.0) {
                             why = QStringLiteral("the press jumped into the old loop");
                             return false;
                         }
                         return true;
                     }});

    cases.push_back({"a pair that would cross an existing loop is refused",
                     [](Runner &run, Reason &why) {
                         double a = 0.0;
                         double b = 0.0;
                         double c = 0.0;
                         double d = 0.0;
                         run.span(a, b, c, d);
                         if (!run.makeLoop(b, c, why)) {
                             return false;
                         }
                         const QVariantMap before = run.loopRegion();
                         run.pressLoop(Qt::ShiftModifier);
                         run.cancelPending();
                         run.seek(a);
                         run.waitFrames(a);
                         run.pressLoop();
                         run.seek((b + c) / 2.0);
                         run.waitFrames((b + c) / 2.0);
                         run.pressLoop();
                         if (run.loopPending()) {
                             why = QStringLiteral("the refused pair is still waiting");
                             return false;
                         }
                         if (run.loopRegion() != before) {
                             why = QStringLiteral("the refused pair rewrote the existing loop");
                             return false;
                         }
                         return true;
                     }});

    // ---- pads and saved loops ----------------------------------------------------------------
    cases.push_back({"saving a loop on a pad arms it and paints a mark",
                     [](Runner &run, Reason &why) {
                         double a = 0.0;
                         double b = 0.0;
                         double c = 0.0;
                         double d = 0.0;
                         run.span(a, b, c, d);
                         if (!run.makeLoop(a, b, why)) {
                             return false;
                         }
                         run.save(2);
                         pump(30);
                         const QVariant loop = run.padLoop(2);
                         if (!loop.isValid() || loop.isNull()) {
                             why = QStringLiteral("pad 2 did not take the loop");
                             return false;
                         }
                         if (run.armed() < 1) {
                             why = QStringLiteral("the saved loop is not armed");
                             return false;
                         }
                         if (run.armedMarks() < 1) {
                             why = QStringLiteral("the strip does not paint the saved loop as active");
                             return false;
                         }
                         return true;
                     }});

    cases.push_back({"saving moves the loop off the button, not a copy of it",
                     [](Runner &run, Reason &why) {
                         double a = 0.0;
                         double b = 0.0;
                         double c = 0.0;
                         double d = 0.0;
                         run.span(a, b, c, d);
                         if (!run.makeLoop(a, b, why)) {
                             return false;
                         }
                         const int armedBefore = run.armed();
                         run.save(3);
                         pump(30);
                         if (run.hasLoopRegion()) {
                             why = QStringLiteral("the button still holds a copy of the loop");
                             return false;
                         }
                         if (run.armed() != armedBefore) {
                             why = QStringLiteral("the loop is armed %1 times instead of %2")
                                       .arg(run.armed())
                                       .arg(armedBefore);
                             return false;
                         }
                         return true;
                     }});

    cases.push_back({"saving a playing loop does not move the sound",
                     [](Runner &run, Reason &why) {
                         double a = 0.0;
                         double b = 0.0;
                         double c = 0.0;
                         double d = 0.0;
                         run.span(a, b, c, d);
                         if (!run.makeLoop(a, b, why)) {
                             return false;
                         }
                         run.play(true);
                         pump(40);
                         double previous = run.frames();
                         double rewind = 0.0;
                         run.save(4);
                         for (int step = 0; step < 10; ++step) {
                             pump(6);
                             const double now = run.frames();
                             rewind = std::max(rewind, previous - now);
                             previous = now;
                         }
                         run.play(false);
                         // A loop wraps by its own length while the deck plays; that jump back is the
                         // loop doing its job, not the save moving the sound.
                         const double start = run.padLoop(4).toMap().value(QStringLiteral("start")).toDouble();
                         const double end = run.padLoop(4).toMap().value(QStringLiteral("end")).toDouble();
                         const double length = end - start;
                         const bool wrapped = std::abs(rewind - length) < 4096.0;
                         if (rewind > 1024.0) {
                             if (!wrapped) {
                                 why = QStringLiteral("saving the loop sent the sound back by %1 frames")
                                           .arg(rewind);
                                 return false;
                             }
                         }
                         return true;
                     }});

    cases.push_back({"a pad press arms every saved loop at once",
                     [](Runner &run, Reason &why) {
                         double a = 0.0;
                         double b = 0.0;
                         double c = 0.0;
                         double d = 0.0;
                         run.span(a, b, c, d);
                         if (!run.makeLoop(a, b, why)) {
                             return false;
                         }
                         run.save(1);
                         pump(30);
                         if (!run.makeLoop(c, d, why)) {
                             return false;
                         }
                         run.save(2);
                         pump(30);
                         run.pressLoop(Qt::ShiftModifier);
                         if (run.armed() != 0) {
                             why = QStringLiteral("the mode did not switch off");
                             return false;
                         }
                         run.pressPad(1);
                         if (run.armed() < 2) {
                             why = QStringLiteral("pressing a pad armed %1 loop(s) instead of both")
                                       .arg(run.armed());
                             return false;
                         }
                         if (!run.loopButtonLit()) {
                             why = QStringLiteral("the button did not light up for the mode");
                             return false;
                         }
                         return true;
                     }});

    cases.push_back({"with the mode off no saved loop is painted active",
                     [](Runner &run, Reason &why) {
                         double a = 0.0;
                         double b = 0.0;
                         double c = 0.0;
                         double d = 0.0;
                         run.span(a, b, c, d);
                         if (!run.makeLoop(a, b, why)) {
                             return false;
                         }
                         run.save(1);
                         pump(30);
                         if (!run.makeLoop(c, d, why)) {
                             return false;
                         }
                         run.save(2);
                         pump(30);
                         run.pressLoop(Qt::ShiftModifier);
                         if (run.armedMarks() != 0) {
                             why = QStringLiteral("%1 saved loop(s) are still painted active")
                                       .arg(run.armedMarks());
                             return false;
                         }
                         if (run.loopButtonLit()) {
                             why = QStringLiteral("the button is still lit");
                             return false;
                         }
                         return true;
                     }});

    cases.push_back({"shift-click deletes a pad's loop, its mark and its name",
                     [](Runner &run, Reason &why) {
                         double a = 0.0;
                         double b = 0.0;
                         double c = 0.0;
                         double d = 0.0;
                         run.span(a, b, c, d);
                         if (!run.makeLoop(a, b, why)) {
                             return false;
                         }
                         run.save(5);
                         run.label(5);
                         pump(40);
                         const int marksBefore = int(run.stripLoops().size());
                         run.pressPad(5, Qt::ShiftModifier);
                         const QVariant loop = run.padLoop(5);
                         if (loop.isValid() && !loop.isNull()) {
                             why = QStringLiteral("the pad still holds its loop");
                             return false;
                         }
                         if (run.armed() != 0) {
                             why = QStringLiteral("the deleted loop is still armed");
                             return false;
                         }
                         if (!run.cueLabel(5).isEmpty() || !run.cueColor(5).isEmpty()) {
                             why = QStringLiteral("the pad kept its name or colour");
                             return false;
                         }
                         if (int(run.stripLoops().size()) >= marksBefore) {
                             why = QStringLiteral("the strip still paints the deleted loop");
                             return false;
                         }
                         return true;
                     }});

    // ---- cue points -------------------------------------------------------------------------
    cases.push_back({"clicking an empty pad stores the playhead",
                     [](Runner &run, Reason &why) {
                         double a = 0.0;
                         double b = 0.0;
                         double c = 0.0;
                         double d = 0.0;
                         run.span(a, b, c, d);
                         run.seek(b);
                         if (!run.waitFrames(b)) {
                             why = QStringLiteral("the deck did not reach the point");
                             return false;
                         }
                         const double where = run.frames();
                         run.pressPad(3);
                         const QVariant cue = run.hotcue(3);
                         if (!cue.isValid() || cue.isNull()) {
                             why = QStringLiteral("the pad stayed empty");
                             return false;
                         }
                         if (std::abs(cue.toDouble() - where) > 2000.0) {
                             why = QStringLiteral("the pad stored %1 instead of %2")
                                       .arg(cue.toDouble())
                                       .arg(where);
                             return false;
                         }
                         return true;
                     }});

    cases.push_back({"a set pad jumps to its point", [](Runner &run, Reason &why) {
                         double a = 0.0;
                         double b = 0.0;
                         double c = 0.0;
                         double d = 0.0;
                         run.span(a, b, c, d);
                         run.seek(b);
                         run.waitFrames(b);
                         const double where = run.frames();
                         run.pressPad(3);
                         run.seek(a);
                         run.waitFrames(a);
                         run.pressPad(3);
                         if (std::abs(run.frames() - where) > 2000.0) {
                             why = QStringLiteral("the pad landed at %1 instead of %2")
                                       .arg(run.frames())
                                       .arg(where);
                             return false;
                         }
                         return true;
                     }});

    cases.push_back({"a pad keeps its name and colour, and the strip shows them",
                     [](Runner &run, Reason &why) {
                         double a = 0.0;
                         double b = 0.0;
                         double c = 0.0;
                         double d = 0.0;
                         run.span(a, b, c, d);
                         run.seek(b);
                         run.waitFrames(b);
                         run.pressPad(2);
                         run.label(2);
                         pump(40);
                         if (run.cueLabel(2) != QStringLiteral("Drop")) {
                             why = QStringLiteral("the pad lost its name");
                             return false;
                         }
                         if (run.cueColor(2) != QStringLiteral("#5fb85f")) {
                             why = QStringLiteral("the pad lost its colour");
                             return false;
                         }
                         bool found = false;
                         for (const QVariant &entry : run.stripCues()) {
                             const QVariantMap mark = entry.toMap();
                             const bool named =
                                 mark.value(QStringLiteral("label")).toString() == QStringLiteral("Drop");
                             const bool coloured = mark.value(QStringLiteral("color")).toString()
                                                   == QStringLiteral("#5fb85f");
                             if (named && coloured) {
                                 found = true;
                             }
                         }
                         if (!found) {
                             why = QStringLiteral("the strip does not carry the pad's name and colour");
                             return false;
                         }
                         return true;
                     }});

    cases.push_back({"shift-click clears a cue's point, name and colour",
                     [](Runner &run, Reason &why) {
                         double a = 0.0;
                         double b = 0.0;
                         double c = 0.0;
                         double d = 0.0;
                         run.span(a, b, c, d);
                         run.seek(b);
                         run.waitFrames(b);
                         run.pressPad(3);
                         run.label(3);
                         pump(30);
                         run.pressPad(3, Qt::ShiftModifier);
                         const QVariant cue = run.hotcue(3);
                         if (cue.isValid() && !cue.isNull()) {
                             why = QStringLiteral("the point stayed on the pad");
                             return false;
                         }
                         if (!run.cueLabel(3).isEmpty() || !run.cueColor(3).isEmpty()) {
                             why = QStringLiteral("the name or colour stayed");
                             return false;
                         }
                         return true;
                     }});

    cases.push_back({"shift-click on an empty pad changes nothing",
                     [](Runner &run, Reason &why) {
                         run.pressPad(6, Qt::ShiftModifier);
                         const QVariant cue = run.hotcue(6);
                         if (cue.isValid() && !cue.isNull()) {
                             why = QStringLiteral("a point appeared on an empty pad");
                             return false;
                         }
                         if (run.armed() != 0) {
                             why = QStringLiteral("the click armed a loop");
                             return false;
                         }
                         return true;
                     }});

    cases.push_back({"the strip carries one mark per stored pad",
                     [](Runner &run, Reason &why) {
                         double a = 0.0;
                         double b = 0.0;
                         double c = 0.0;
                         double d = 0.0;
                         run.span(a, b, c, d);
                         run.seek(a);
                         run.waitFrames(a);
                         run.pressPad(0);
                         run.seek(c);
                         run.waitFrames(c);
                         run.pressPad(1);
                         pump(30);
                         if (int(run.stripCues().size()) < 2) {
                             why = QStringLiteral("the strip shows %1 mark(s) for two pads")
                                       .arg(run.stripCues().size());
                             return false;
                         }
                         return true;
                     }});

    cases.push_back({"a mark sits where its cue point is",
                     [](Runner &run, Reason &why) {
                         double a = 0.0;
                         double b = 0.0;
                         double c = 0.0;
                         double d = 0.0;
                         run.span(a, b, c, d);
                         run.seek(c);
                         run.waitFrames(c);
                         run.pressPad(7);
                         pump(30);
                         const double total = run.total();
                         if (total <= 0.0) {
                             why = QStringLiteral("the deck does not know the track's length");
                             return false;
                         }
                         const double expected = c / total;
                         bool found = false;
                         for (const QVariant &entry : run.stripCues()) {
                             const double ratio = entry.toMap().value(QStringLiteral("ratio")).toDouble();
                             if (std::abs(ratio - expected) < 0.01) {
                                 found = true;
                             }
                         }
                         if (!found) {
                             why = QStringLiteral("no mark near %.3f of the track").arg(expected);
                             return false;
                         }
                         return true;
                     }});

    cases.push_back({"the loop button is dark while the deck has no loop",
                     [](Runner &run, Reason &why) {
                         if (run.loopButtonLit()) {
                             why = QStringLiteral("the button is lit with no loop and no mode");
                             return false;
                         }
                         if (run.stripHasLoop()) {
                             why = QStringLiteral("the strip paints a loop that does not exist");
                             return false;
                         }
                         return true;
                     }});

    cases.push_back({"a deleted loop can be made again on the same bars",
                     [](Runner &run, Reason &why) {
                         double a = 0.0;
                         double b = 0.0;
                         double c = 0.0;
                         double d = 0.0;
                         run.span(a, b, c, d);
                         if (!run.makeLoop(a, b, why)) {
                             return false;
                         }
                         run.save(1);
                         pump(30);
                         run.pressPad(1, Qt::ShiftModifier);
                         run.pressLoop(Qt::ShiftModifier);
                         run.cancelPending();
                         if (!run.makeLoop(a, b, why)) {
                             return false;
                         }
                         if (run.armed() != 1) {
                             why = QStringLiteral("the remade loop is armed %1 times").arg(run.armed());
                             return false;
                         }
                         return true;
                     }});

    cases.push_back({"an empty pad with a loop in force takes the loop, not a point",
                     [](Runner &run, Reason &why) {
                         double a = 0.0;
                         double b = 0.0;
                         double c = 0.0;
                         double d = 0.0;
                         run.span(a, b, c, d);
                         if (!run.makeLoop(a, b, why)) {
                             return false;
                         }
                         const int armedBefore = run.armed();
                         run.pressPad(0);
                         const QVariant loop = run.padLoop(0);
                         if (!loop.isValid() || loop.isNull()) {
                             why = QStringLiteral("the pad did not take the loop");
                             return false;
                         }
                         const QVariant cue = run.hotcue(0);
                         if (cue.isValid() && !cue.isNull()) {
                             why = QStringLiteral("the pad stored a point as well");
                             return false;
                         }
                         if (run.armed() < armedBefore) {
                             why = QStringLiteral("saving the loop disarmed it");
                             return false;
                         }
                         return true;
                     }});

    cases.push_back({"storing a cue while the deck plays does not move the sound",
                     [](Runner &run, Reason &why) {
                         double a = 0.0;
                         double b = 0.0;
                         double c = 0.0;
                         double d = 0.0;
                         run.span(a, b, c, d);
                         run.seek(a);
                         run.waitFrames(a);
                         run.play(true);
                         pump(30);
                         double previous = run.frames();
                         double rewind = 0.0;
                         run.pressPad(1);
                         for (int step = 0; step < 8; ++step) {
                             pump(6);
                             const double now = run.frames();
                             rewind = std::max(rewind, previous - now);
                             previous = now;
                         }
                         run.play(false);
                         const QVariant cue = run.hotcue(1);
                         if (!cue.isValid() || cue.isNull()) {
                             why = QStringLiteral("the pad stayed empty");
                             return false;
                         }
                         if (rewind > 1024.0) {
                             why = QStringLiteral("storing a cue sent the sound back by %1 frames")
                                       .arg(rewind);
                             return false;
                         }
                         return true;
                     }});

    cases.push_back({"saving a loop while paused does not move the playhead",
                     [](Runner &run, Reason &why) {
                         double a = 0.0;
                         double b = 0.0;
                         double c = 0.0;
                         double d = 0.0;
                         run.span(a, b, c, d);
                         if (!run.makeLoop(a, b, why)) {
                             return false;
                         }
                         run.play(false);
                         pump(30);
                         const double before = run.frames();
                         run.save(4);
                         pump(30);
                         if (std::abs(run.frames() - before) > 64.0) {
                             why = QStringLiteral("saving moved the playhead: %1 -> %2")
                                       .arg(before)
                                       .arg(run.frames());
                             return false;
                         }
                         return true;
                     }});

    cases.push_back({"two saved loops both repeat", [](Runner &run, Reason &why) {
                         double a = 0.0;
                         double b = 0.0;
                         double c = 0.0;
                         double d = 0.0;
                         run.span(a, b, c, d);
                         if (!run.makeLoop(a, b, why)) {
                             return false;
                         }
                         run.save(0);
                         pump(30);
                         if (!run.makeLoop(c, d, why)) {
                             return false;
                         }
                         run.save(1);
                         pump(30);
                         run.seek((c + d) / 2.0);
                         run.waitFrames((c + d) / 2.0);
                         run.play(true);
                         double peak = run.frames();
                         for (int step = 0; step < 10; ++step) {
                             pump(6);
                             peak = std::max(peak, run.frames());
                         }
                         run.play(false);
                         if (peak > d + 14000.0) {
                             why = QStringLiteral("the second loop did not repeat: %1 > %2")
                                       .arg(peak)
                                       .arg(d);
                             return false;
                         }
                         return true;
                     }});

    cases.push_back({"deleting a loop does not stop playback",
                     [](Runner &run, Reason &why) {
                         double a = 0.0;
                         double b = 0.0;
                         double c = 0.0;
                         double d = 0.0;
                         run.span(a, b, c, d);
                         if (!run.makeLoop(a, b, why)) {
                             return false;
                         }
                         run.save(2);
                         pump(30);
                         run.play(true);
                         pump(30);
                         if (!run.playing()) {
                             why = QStringLiteral("the deck is not playing before the delete");
                             return false;
                         }
                         run.pressPad(2, Qt::ShiftModifier);
                         pump(30);
                         if (!run.playing()) {
                             why = QStringLiteral("deleting a loop stopped the deck");
                             return false;
                         }
                         return true;
                     }});

    cases.push_back({"a second press closes the pair even inside another loop",
                     [](Runner &run, Reason &why) {
                         double a = 0.0;
                         double b = 0.0;
                         double c = 0.0;
                         double d = 0.0;
                         run.span(a, b, c, d);
                         if (!run.makeLoop(b, c, why)) {
                             return false;
                         }
                         run.pressLoop(Qt::ShiftModifier);
                         run.cancelPending();
                         run.seek(a);
                         run.waitFrames(a);
                         run.pressLoop();
                         if (!run.loopPending()) {
                             why = QStringLiteral("the press outside the loop did not start a pair");
                             return false;
                         }
                         run.seek((b + c) / 2.0);
                         run.waitFrames((b + c) / 2.0);
                         run.pressLoop();
                         if (run.loopPending()) {
                             why = QStringLiteral("the pair is still waiting after the second press");
                             return false;
                         }
                         return true;
                     }});

    cases.push_back({"a saved loop is a mark again after the track is reloaded",
                     [](Runner &run, Reason &why) {
                         double a = 0.0;
                         double b = 0.0;
                         double c = 0.0;
                         double d = 0.0;
                         run.span(a, b, c, d);
                         if (!run.makeLoop(a, b, why)) {
                             return false;
                         }
                         run.save(1);
                         pump(80);
                         const QVariant loop = run.padLoop(1);
                         if (!loop.isValid() || loop.isNull()) {
                             why = QStringLiteral("the pad did not take the loop before the reload");
                             return false;
                         }
                         // Empty the deck, so a mark left on screen cannot pass for a restored one.
                         run.eject();
                         if (run.padLoop(1).isValid() && !run.padLoop(1).isNull()) {
                             why = QStringLiteral("the ejected deck still holds the loop");
                             return false;
                         }
                         if (int(run.stripLoops().size()) > 0) {
                             why = QStringLiteral("the strip still paints a mark after the eject");
                             return false;
                         }
                         run.reload();
                         if (!waitFor([&] { return run.padLoop(1).isValid() && !run.padLoop(1).isNull(); },
                                      6000)) {
                             why = QStringLiteral("the reloaded deck lost the loop on pad 1");
                             return false;
                         }
                         if (!waitFor([&] { return int(run.stripLoops().size()) > 0; }, 3000)) {
                             why = QStringLiteral("the reloaded track shows no loop mark on the strip");
                             return false;
                         }
                         return true;
                     }});

    cases.push_back({"a small level correction is shown beside the clock",
                     [](Runner &run, Reason &why) {
                         // 0,343 amplitude of a sine is 12,3 dBFS RMS: the correction is +0,3 dB, the
                         // size a hand would call "nothing" — and exactly the size the shell used to
                         // swallow whole.
                         const QString path = writeTone(QStringLiteral("/tmp/dj-scenario-autogain.wav"),
                                                        0.343, 2.0);
                         if (path.isEmpty()) {
                             why = QStringLiteral("the tone for the case could not be written");
                             return false;
                         }
                         run.load(path);
                         // Wait for this file, not for "a track": the deck keeps the previous one until
                         // the loader's attach lands, and that moment briefly has no measured level.
                         const auto arrived = [&] {
                             const QVariant value = run.deck().value(QStringLiteral("auto_gain_db"));
                             return std::abs(run.total() - 96000.0) < 2.0 && value.isValid()
                                    && !value.isNull();
                         };
                         if (!waitFor(arrived, 8000)) {
                             why = QStringLiteral("the quiet track did not arrive with a measured "
                                                  "level (total %1 frames, auto_gain %2)")
                                       .arg(run.total(), 0, 'f', 0)
                                       .arg(run.deck().value(QStringLiteral("auto_gain")).toDouble(),
                                            0, 'f', 3);
                             return false;
                         }
                         const QVariant raw = run.deck().value(QStringLiteral("auto_gain_db"));
                         if (!raw.isValid() || raw.isNull()) {
                             why = QStringLiteral("the snapshot says nothing about the measured level");
                             return false;
                         }
                         const double db = raw.toDouble();
                         if (std::abs(db - 0.3) > 0.4) {
                             why = QStringLiteral("the correction is %1 dB, not the +0,3 dB of this tone")
                                       .arg(db, 0, 'f', 2);
                             return false;
                         }
                         QQuickItem *label = run.item(QStringLiteral("autogain_") + run.deckId());
                         if (label == nullptr) {
                             why = QStringLiteral("there is no correction label beside the clock");
                             return false;
                         }
                         if (!label->isVisible()) {
                             why = QStringLiteral("the correction of %1 dB is measured but not shown")
                                       .arg(db, 0, 'f', 1);
                             return false;
                         }
                         const QString text = label->property("text").toString();
                         if (!text.contains(QStringLiteral("0.3"))) {
                             why = QStringLiteral("the label reads \"%1\"").arg(text);
                             return false;
                         }
                         return true;
                     }});

    int failed = 0;
    for (const Case &entry : cases) {
        runner.reset();
        Reason why;
        const bool ok = entry.body(runner, why);
        if (ok) {
            std::fprintf(stdout, "scenario ok: %s\n", entry.name);
        } else {
            ++failed;
            std::fprintf(stderr, "scenario FAILED: %s — %s\n", entry.name, qPrintable(why));
        }
    }
    std::fprintf(stdout, "scenarios: %d of %d passed\n", int(cases.size()) - failed,
                 int(cases.size()));
    return failed == 0 ? 0 : 1;
}

} // namespace djui
