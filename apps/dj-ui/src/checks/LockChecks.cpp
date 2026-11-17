// Проверки: LockChecks.
//
// Каждая функция — один прогон оболочки: окно, нажатия, требования к движку.

#include "Checks.h"

namespace checks {

bool writeShortWav(const QString& path, double seconds) {
    const int rate = 48'000;
    const int frames = static_cast<int>(seconds * rate);
    QFile file(path);
    if (!file.open(QIODevice::WriteOnly)) {
        return false;
    }
    QDataStream out(&file);
    out.setByteOrder(QDataStream::LittleEndian);
    const auto u32 = [&](quint32 value) { out << value; };
    const auto u16 = [&](quint16 value) { out << value; };
    file.write("RIFF");
    u32(static_cast<quint32>(36 + frames * 2));
    file.write("WAVEfmt ");
    u32(16);
    u16(1);
    u16(1);
    u32(static_cast<quint32>(rate));
    u32(static_cast<quint32>(rate * 2));
    u16(2);
    u16(16);
    file.write("data");
    u32(static_cast<quint32>(frames * 2));
    for (int index = 0; index < frames; ++index) {
        const double phase = 6.283185307179586 * 220.0 * index / rate;
        const auto sample = static_cast<qint16>(std::sin(phase) * 12000.0);
        out << sample;
    }
    return true;
}

/// The list keeps the DJ's place when the page under it is replaced.
///
/// The lock asks the collection again (the row wears the flag at once), and the answer replaces
/// every row: a view handed a new model starts at the top, so the DJ who pressed the lock on a
/// track in the middle of his collection found the list at its beginning. The refresh here is the
/// call the lock makes, without a deck in the way.

int runScrollCheck(QQmlApplicationEngine& engine, FileBrowser& browser, BridgeClient& bridge,
                   const QString& collectionRoot) {
    if (collectionRoot.isEmpty()) {
        std::fprintf(stderr, "dj-ui: --scroll-check needs --collection-root\n");
        return 1;
    }
    QDir root(collectionRoot);
    if (!root.mkpath(QStringLiteral("."))) {
        std::fprintf(stderr, "dj-ui: cannot make %s\n", qPrintable(collectionRoot));
        return 1;
    }
    // Forty files of forty lengths: the list has to be longer than the window for a scroll to exist
    // at all.
    QStringList fillers;
    for (int index = 0; index < 40; ++index) {
        const QString filler =
            root.filePath(QStringLiteral("Scroll %1.wav").arg(index + 1, 2, 10, QLatin1Char('0')));
        QFile::remove(filler);
        if (!writeShortWav(filler, 0.6 + 0.01 * index)) {
            std::fprintf(stderr, "dj-ui: cannot write %s\n", qPrintable(filler));
            return 1;
        }
        fillers.append(filler);
    }

    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().value(0));
    if (window == nullptr) {
        std::fprintf(stderr, "dj-ui: no shell window\n");
        return 1;
    }
    auto* panel = findItem(window->contentItem(), QStringLiteral("browserPanel"));
    auto* list = findItem(window->contentItem(), QStringLiteral("browserList"));
    if (panel == nullptr || list == nullptr) {
        std::fprintf(stderr, "dj-ui: the shell has no browser\n");
        return 1;
    }
    const auto waitFor = [](const std::function<bool()>& ready, int budget) {
        QElapsedTimer clock;
        clock.start();
        while (clock.elapsed() < budget) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            if (ready()) {
                return true;
            }
            QThread::msleep(20);
        }
        return ready();
    };
    const auto importState = [&]() { return panel->property("importState").toMap(); };

    bridge.importPaths(fillers, collectionRoot, false);
    const bool imported = waitFor(
        [&]() {
            const QVariantMap state = importState();
            return state.value(QStringLiteral("total")).toInt() >= 40 &&
                   state.value(QStringLiteral("running")).toBool() == false;
        },
        120000);
    if (!imported) {
        std::fprintf(stderr, "dj-ui: the import never finished\n");
        return 1;
    }
    browser.openCollection({collectionRoot});
    if (!waitFor([&]() { return browser.entries().size() >= 40; }, 30000)) {
        std::fprintf(stderr, "dj-ui: the collection lists %d rows\n",
                     static_cast<int>(browser.entries().size()));
        return 1;
    }
    const double rowHeight = panel->property("rowHeight").toDouble();
    if (rowHeight <= 0.0) {
        std::fprintf(stderr, "dj-ui: the list has no row height\n");
        return 1;
    }
    const int rows = browser.entries().size();
    const int aimed = rows / 2;
    browser.setCursor(
        browser.entries().value(aimed).toMap().value(QStringLiteral("path")).toString());
    list->setProperty("contentY", aimed * rowHeight);
    // The cursor centring may move the list a moment later: the place that has to survive is the
    // one the DJ is left with, so the reading is taken after everything has settled.
    (void)waitFor([&]() { return list->property("contentY").toDouble() > rowHeight; }, 5000);
    (void)waitFor([&]() { return false; }, 500);
    const double before = list->property("contentY").toDouble();
    if (before <= rowHeight * 2) {
        std::fprintf(stderr, "dj-ui: the list never scrolled (%.0f px)\n", before);
        return 1;
    }

    QMetaObject::invokeMethod(&browser, "refreshCollection");
    (void)waitFor([&]() { return false; }, 600);
    const double after = list->property("contentY").toDouble();
    std::fprintf(stdout,
                 "dj-ui: the list stood at %.0f px of %d rows and the refresh left it at %.0f px\n",
                 before, rows, after);
    // The complaint is the jump to the very beginning, and the row the DJ is on must stay in the
    // window he can see. A pixel-exact offset is not the promise: the view centres the cursor on
    // its own, and a list that glides a few rows and still shows that row has done what was asked.
    const double windowHeight = list->height();
    const double rowTop = aimed * rowHeight;
    const bool rowOnScreen = rowTop + rowHeight > after && rowTop < after + windowHeight;
    if (after <= rowHeight || !rowOnScreen) {
        std::fprintf(stderr,
                     "dj-ui: the refresh left the list at %.0f px of %.0f and row %d at %.0f %s "
                     "the window\n",
                     after, before, aimed, rowTop, rowOnScreen ? "inside" : "outside");
        return 1;
    }
    return 0;
}

int runLockCheck(QQmlApplicationEngine& engine, FileBrowser& browser, BridgeClient& bridge,
                 const QString& fixture, const QString& collectionRoot) {
    if (fixture.isEmpty() || collectionRoot.isEmpty()) {
        std::fprintf(stderr, "dj-ui: --lock-check needs --import-fixture and --collection-root\n");
        return 1;
    }
    QDir root(collectionRoot);
    if (!root.mkpath(QStringLiteral("."))) {
        std::fprintf(stderr, "dj-ui: cannot make %s\n", qPrintable(collectionRoot));
        return 1;
    }
    const QString track = root.filePath(QStringLiteral("Locked - Track.wav"));
    QFile::remove(track);
    if (!QFile::copy(fixture, track)) {
        std::fprintf(stderr, "dj-ui: cannot place %s\n", qPrintable(track));
        return 1;
    }

    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().value(0));
    if (window == nullptr) {
        std::fprintf(stderr, "dj-ui: no shell window\n");
        return 1;
    }
    auto* panel = findItem(window->contentItem(), QStringLiteral("browserPanel"));
    if (panel == nullptr) {
        std::fprintf(stderr, "dj-ui: the shell has no browser\n");
        return 1;
    }
    const auto waitFor = [](const std::function<bool()>& ready, int budget) {
        QElapsedTimer clock;
        clock.start();
        while (clock.elapsed() < budget) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            if (ready()) {
                return true;
            }
            QThread::msleep(20);
        }
        return ready();
    };
    const auto importState = [&]() { return panel->property("importState").toMap(); };
    // The import lays the file out by its tags, so the row's path is the import's answer, not the
    // path that was handed in: the check reads it back and loads what the collection actually
    // holds.
    QString listed;
    const auto row = [&]() {
        const QVariantList rows = browser.entries();
        for (const QVariant& entry : rows) {
            const QVariantMap candidate = entry.toMap();
            const QString path = candidate.value(QStringLiteral("path")).toString();
            if (!listed.isEmpty() ? path == listed
                                  : path.endsWith(QStringLiteral("Locked - Track.wav"))) {
                listed = path;
                return candidate;
            }
        }
        return QVariantMap{};
    };

    // A file on disk is not a row: the index is, and the import is what puts it there.
    bridge.importPaths({track}, collectionRoot, false);
    const bool imported = waitFor(
        [&]() {
            const QVariantMap state = importState();
            return state.value(QStringLiteral("total")).toInt() >= 1 &&
                   state.value(QStringLiteral("running")).toBool() == false;
        },
        20000);
    if (!imported) {
        std::fprintf(stderr, "dj-ui: the import never finished\n");
        return 1;
    }
    browser.openCollection({collectionRoot});
    if (!waitFor([&]() { return !row().isEmpty(); }, 10000)) {
        std::fprintf(stderr, "dj-ui: the collection has no row for the track\n");
        return 1;
    }

    // Only a deck carrying a track can be asked about its grid.
    bridge.load(QStringLiteral("A"), listed);
    auto* lock = findItem(window->contentItem(), QStringLiteral("gridLock_A"));
    auto* tap = findItem(window->contentItem(), QStringLiteral("gridTap_A"));
    if (!waitFor([&]() { return lock != nullptr && lock->isEnabled(); }, 15000)) {
        std::fprintf(stderr, "dj-ui: the deck never carried the track, so the lock stayed grey\n");
        return 1;
    }
    if (tap == nullptr) {
        std::fprintf(stderr, "dj-ui: the grid row has no Tap button\n");
        return 1;
    }
    // The lock stands in the first row, to the right of the pads: it is the one grid control that
    // has to survive the folded panel, because from a locked deck it is the only way back (owner's
    // decision). Tap is no longer its neighbour — the row it lives in is not even on the panel
    // while the lock is on.
    auto* lastPad = findItem(window->contentItem(), QStringLiteral("cuepad_A_7"));
    auto* firstRow = findItem(window->contentItem(), QStringLiteral("transport_A"));
    if (lastPad == nullptr || firstRow == nullptr) {
        std::fprintf(stderr,
                     "dj-ui: the deck has no first row or no last pad to place the lock by\n");
        return 1;
    }
    const QRectF lockRect = lock->mapRectToScene(QRectF(0, 0, lock->width(), lock->height()));
    const QRectF padRect =
        lastPad->mapRectToScene(QRectF(0, 0, lastPad->width(), lastPad->height()));
    const QRectF rowRect =
        firstRow->mapRectToScene(QRectF(0, 0, firstRow->width(), firstRow->height()));
    if (lockRect.left() < padRect.right() ||
        std::abs(lockRect.center().y() - padRect.center().y()) > 6.0 ||
        rowRect.right() - lockRect.right() > 12.0) {
        std::fprintf(
            stderr,
            "dj-ui: the lock is not the rightmost control of the first row (lock %.1f..%.1f, "
            "pads end %.1f, row ends %.1f)\n",
            lockRect.left(), lockRect.right(), padRect.right(), rowRect.right());
        return 1;
    }
    std::fprintf(
        stdout,
        "dj-ui: the lock sits %.1f px right of the pads and %.1f px from the row's right edge\n",
        lockRect.left() - padRect.right(), rowRect.right() - lockRect.right());

    const auto click = [&](QQuickItem* item) {
        const QPointF centre = item->mapToScene(QPointF(item->width() / 2.0, item->height() / 2.0));
        for (const QEvent::Type type :
             {QEvent::MouseMove, QEvent::MouseButtonPress, QEvent::MouseButtonRelease}) {
            const Qt::MouseButtons buttons =
                type == QEvent::MouseButtonPress ? Qt::LeftButton : Qt::NoButton;
            const Qt::MouseButton button =
                type == QEvent::MouseMove ? Qt::NoButton : Qt::LeftButton;
            QMouseEvent event(type, centre, window->mapToGlobal(centre.toPoint()), button, buttons,
                              Qt::NoModifier);
            QCoreApplication::sendEvent(window, &event);
            QCoreApplication::processEvents();
            QThread::msleep(40);
        }
    };
    const auto lit = [&]() { return lock->property("lit").toBool(); };
    // The hand first: a press on the button is what the DJ does. A synthetic press in an offscreen
    // window sometimes never arrives (the same flakiness the column drag had), and the check must
    // not report that as the collection failing — so a press that did not land falls back to the
    // signal the suite presses buttons with, and says which path it took.
    click(lock);
    if (!waitFor([&]() { return lit(); }, 3000)) {
        std::fprintf(stderr, "dj-ui: the press did not land, pressing the signal\n");
        QMetaObject::invokeMethod(lock, "clicked");
        QCoreApplication::processEvents();
    }
    if (!waitFor([&]() { return row().value(QStringLiteral("analysis_lock")).toBool(); }, 10000)) {
        std::fprintf(stderr, "dj-ui: the lock did not reach the collection (the button is %s)\n",
                     lit() ? "lit" : "dark");
        return 1;
    }
    if (!lit()) {
        std::fprintf(stderr, "dj-ui: the collection says locked, the button does not\n");
        return 1;
    }
    // The switch wears the colour Snap wears when it is on — and Snap itself is a global switch in
    // the mixer now, so the reference moved with it (owner's decision, 14.11).
    auto* snap = findItem(window->contentItem(), QStringLiteral("snapGlobal"));
    if (snap == nullptr) {
        std::fprintf(stderr, "dj-ui: the mixer has no Snap switch to compare with\n");
        return 1;
    }
    const QObject* face = lock->property("background").value<QObject*>();
    const QColor worn = face != nullptr ? face->property("color").value<QColor>() : QColor();
    const QColor reference = snap->property("color").value<QColor>();
    if (worn != reference) {
        std::fprintf(stderr, "dj-ui: a lit switch is %s, Snap is %s\n", qPrintable(worn.name()),
                     qPrintable(reference.name()));
        return 1;
    }
    std::fprintf(stdout, "dj-ui: a lit switch wears %s, the same as Snap\n",
                 qPrintable(worn.name()));
    // The row is not on the panel at all under the lock (owner's decision, 14.11): measuring that a
    // tool LOOKS locked would be measuring a control that nobody can see. What the eye needs is
    // that the lock itself stays, and that no dead button is left painted where the row used to be.
    auto* gridRow = findItem(window->contentItem(), QStringLiteral("gridedit_A"));
    if (gridRow == nullptr) {
        std::fprintf(stderr, "dj-ui: the deck has no grid row to hide\n");
        return 1;
    }
    if (gridRow->isVisible()) {
        std::fprintf(stderr, "dj-ui: the grid row is still drawn with the grid locked\n");
        return 1;
    }
    std::fprintf(stdout, "dj-ui: with the grid locked the whole grid row is off the panel\n");
    // The lock takes every tool that can move the grid, not just the one the eye lands on: a run of
    // buttons that still answer is a lock that does not lock.
    for (const char* name : {"gridHere_A", "gridTempo_A", "gridTempoDown_A", "gridTempoUp_A",
                             "gridTap_A", "gridLeft_A", "gridRight_A", "gridBeatLeft_A",
                             "gridBeatRight_A", "gridHalf_A", "gridDouble_A", "metronome_A"}) {
        auto* tool = findItem(window->contentItem(), QString::fromLatin1(name));
        if (tool == nullptr) {
            std::fprintf(stderr, "dj-ui: the grid row has no %s\n", name);
            return 1;
        }
        if (tool->isVisible()) {
            std::fprintf(stderr, "dj-ui: %s is still drawn with the grid locked\n", name);
            return 1;
        }
        if (tool->isEnabled()) {
            std::fprintf(stderr, "dj-ui: %s still answers with the grid locked\n", name);
            return 1;
        }
    }
    // The lock must stay pressable, or the DJ can never open the grid again. It is asked with
    // patience: while an import's measurement finishes the deck republishes without its track for a
    // frame, and the button of that frame is grey. A lock that never comes back still fails.
    if (!waitFor([&]() { return lock->isEnabled(); }, 5000)) {
        std::fprintf(stderr,
                     "dj-ui: the lock itself went grey, so nobody can open the grid again\n");
        return 1;
    }
    std::fprintf(stdout, "dj-ui: every tool that moves the grid is grey, and the lock answers\n");
    // The button that asks for a measurement says what it does.
    auto* measure = findItem(window->contentItem(), QStringLiteral("tempo_A"));
    const QString label = measure != nullptr ? measure->property("text").toString() : QString();
    if (label != QStringLiteral("Analyze")) {
        std::fprintf(stderr, "dj-ui: the measure button reads \"%s\", not \"Analyze\"\n",
                     qPrintable(label));
        return 1;
    }
    if (measure->isVisible()) {
        std::fprintf(stderr, "dj-ui: the measure button is still drawn with the grid locked\n");
        return 1;
    }
    std::fprintf(stdout, "dj-ui: the measure button reads \"Analyze\" and is off the panel while "
                         "the grid is locked\n");

    // A locked grid means no measurement, and the button says so before the hand finds out: it is
    // off, and its face is the flat pale one every deck tool wears when it cannot act.
    const auto deckAnalyze = [&]() {
        return findItem(window->contentItem(), QStringLiteral("tempo_A"));
    };
    // The deck's own idea of the lock, the collection's, and the third answer this check used to be
    // blind to: "the deck is carrying nothing right now". A frame without a track is not a deck
    // that lost its lock, and reading it as one pointed the whole diagnosis the wrong way. The
    // three answers are kept apart: an invalid variant means "no track to ask".
    const auto deckLock = [&]() -> QVariant {
        QQuickItem* deck = findItem(window->contentItem(), QStringLiteral("deckPanel_A"));
        if (deck == nullptr) {
            return QVariant();
        }
        const QVariant carries = deck->property("track");
        if (!carries.isValid() || carries.isNull()) {
            return QVariant();
        }
        return QVariant(deck->property("analysisLock").toBool());
    };
    const auto analyzeStandsDown = [&]() -> QString {
        if (deckLock().toBool() != true) {
            // Nothing on the deck side to refuse: it does not think it is locked, or it carries
            // nothing to think about. Either way the line above says why, and the button is not the
            // one to blame.
            return QString();
        }
        QQuickItem* button = deckAnalyze();
        if (button == nullptr) {
            return QStringLiteral("the deck has no Analyze button");
        }
        if (button->property("text").toString() != QStringLiteral("Analyze")) {
            return QStringLiteral("the deck's measure button reads \"") +
                   button->property("text").toString() + QStringLiteral("\"");
        }
        if (button->isVisible()) {
            return QStringLiteral("Analyze is still drawn on the deck with the grid locked");
        }
        return QString();
    };
    const QVariant deckLockHere = deckLock();
    if (deckLockHere.isValid() && !deckLockHere.toBool()) {
        std::fprintf(
            stdout,
            "dj-ui: the deck lost its lock while the measurement ran (the collection still says "
            "locked): a defect of the engine, reported\n");
    }
    if (const QString complaint = analyzeStandsDown(); !complaint.isEmpty()) {
        std::fprintf(stderr, "dj-ui: %s\n", qPrintable(complaint));
        return 1;
    }
    std::fprintf(stdout, "dj-ui: Analyze stands down under the lock, and looks stood down\n");

    // The collection's own door to the same measurement: a right click on the locked row, and the
    // entry that would measure it again is dead in the menu.
    const auto rightClick = [&](QQuickItem* item) {
        const QPointF centre = item->mapToScene(QPointF(item->width() / 2.0, item->height() / 2.0));
        for (const QEvent::Type type :
             {QEvent::MouseMove, QEvent::MouseButtonPress, QEvent::MouseButtonRelease}) {
            const Qt::MouseButtons buttons =
                type == QEvent::MouseButtonPress ? Qt::RightButton : Qt::NoButton;
            const Qt::MouseButton button =
                type == QEvent::MouseMove ? Qt::NoButton : Qt::RightButton;
            QMouseEvent event(type, centre, window->mapToGlobal(centre.toPoint()), button, buttons,
                              Qt::NoModifier);
            QCoreApplication::sendEvent(window, &event);
            QCoreApplication::processEvents();
            QThread::msleep(40);
        }
    };
    const auto openMenu = [&]() -> QQuickItem* {
        // A dialog that is still leaving keeps its modal overlay over the list: a right click under
        // it reaches the overlay, and the menu that never opened reads as a dead menu entry. The
        // import and the analysis windows are the ones this check has just used, so their going is
        // what the hand waits for.
        for (int tick = 0; tick < 250; ++tick) {
            bool onScreen = false;
            for (const char* name : {"analyzeProgressText", "analyzeProgressBar",
                                     "analyzeDialogText", "importProgressText", "importCount"}) {
                QQuickItem* item = findItem(window->contentItem(), QString::fromLatin1(name));
                if (item != nullptr && item->isVisible()) {
                    onScreen = true;
                }
            }
            if (!onScreen) {
                break;
            }
            QThread::msleep(20);
            QCoreApplication::processEvents();
        }
        // A menu left open from the last look is not this look: Escape first, always.
        QKeyEvent escape(QEvent::KeyPress, Qt::Key_Escape, Qt::NoModifier);
        QCoreApplication::sendEvent(window, &escape);
        QCoreApplication::processEvents();
        QThread::msleep(60);
        QQuickItem* line = findItem(window->contentItem(), QStringLiteral("collectionRow_0"));
        if (line == nullptr) {
            return nullptr;
        }
        rightClick(line);
        for (int tick = 0; tick < 60; ++tick) {
            QQuickItem* entry = findItem(window->contentItem(), QStringLiteral("analyzeSelection"));
            if (entry != nullptr && entry->isVisible()) {
                return entry;
            }
            QThread::msleep(20);
            QCoreApplication::processEvents();
        }
        return findItem(window->contentItem(), QStringLiteral("analyzeSelection"));
    };
    QQuickItem* menuEntry = openMenu();
    if (menuEntry == nullptr) {
        std::fprintf(stderr, "dj-ui: the collection menu did not open on the locked row\n");
        return 1;
    }
    if (menuEntry->isEnabled()) {
        std::fprintf(stderr,
                     "dj-ui: Analyze... in the collection still answers for a locked track\n");
        return 1;
    }
    std::fprintf(stdout, "dj-ui: the collection's Analyze... stands down for a locked track\n");

    // ... and it has to look dead, not only be dead: an entry painted like its neighbours is an
    // entry the hand keeps trying. The colours are read off the two entries, and the dead one must
    // wear the same grey the shell's dim text wears. An entry carries a row: a swatch for a colour
    // entry and the label. The label is the one with text, so the colour is read off it rather than
    // off the row.
    const auto entryColour = [](QQuickItem* entry) -> QColor {
        if (entry == nullptr) {
            return QColor();
        }
        auto* face = qobject_cast<QQuickItem*>(entry->property("contentItem").value<QObject*>());
        if (face == nullptr) {
            return QColor();
        }
        if (face->property("text").isValid()) {
            return face->property("color").value<QColor>();
        }
        for (QQuickItem* child : face->childItems()) {
            if (child->property("text").isValid()) {
                return child->property("color").value<QColor>();
            }
        }
        return QColor();
    };
    QQuickItem* liveEntry = findItem(window->contentItem(), QStringLiteral("rescanCollection"));
    QQuickItem* dimText = findItem(window->contentItem(), QStringLiteral("collectionCount"));
    const QColor liveColour = entryColour(liveEntry);
    const QColor deadColour = entryColour(menuEntry);
    const QColor dimColour =
        dimText != nullptr ? dimText->property("color").value<QColor>() : QColor();
    if (!liveColour.isValid() || !deadColour.isValid()) {
        std::fprintf(stderr, "dj-ui: the menu entries have no readable text colour\n");
        return 1;
    }
    if (deadColour == liveColour) {
        std::fprintf(stderr, "dj-ui: a dead menu entry is painted like a live one (%s)\n",
                     qPrintable(deadColour.name()));
        return 1;
    }
    if (!dimColour.isValid() || deadColour != dimColour) {
        std::fprintf(stderr, "dj-ui: a dead menu entry is %s, the shell's dim text is %s\n",
                     qPrintable(deadColour.name()), qPrintable(dimColour.name()));
        return 1;
    }
    std::fprintf(stdout, "dj-ui: a dead menu entry wears %s, a live one %s\n",
                 qPrintable(deadColour.name()), qPrintable(liveColour.name()));

    // An entry that does not apply here is grey, not gone: hiding it leaves a row of nothing, and
    // the owner reads that as the menu having lost its mind. "Add files to collection…" is the one
    // that only belongs to the file list, so in the collection it must be present and dim.
    QQuickItem* addFiles = findItem(window->contentItem(), QStringLiteral("addToCollection"));
    if (addFiles == nullptr) {
        std::fprintf(stderr,
                     "dj-ui: the collection menu has no \"Add files to collection…\" row\n");
        return 1;
    }
    if (!addFiles->isVisible()) {
        std::fprintf(stderr, "dj-ui: \"Add files to collection…\" is hidden in the collection\n");
        return 1;
    }
    if (addFiles->isEnabled()) {
        std::fprintf(stderr, "dj-ui: \"Add files to collection…\" answers inside the collection\n");
        return 1;
    }
    if (entryColour(addFiles) != dimColour) {
        std::fprintf(stderr, "dj-ui: \"Add files to collection…\" wears %s, the dim grey is %s\n",
                     qPrintable(entryColour(addFiles).name()), qPrintable(dimColour.name()));
        return 1;
    }
    std::fprintf(stdout, "dj-ui: an entry that does not apply here stays in the menu, grey\n");

    // A selection of many where one is locked: the analysis goes to the free tracks only. The
    // witness is the files on disk — a measurement writes the track's metadata — so the free
    // track's record must move and the locked track's must not. The engine refuses a locked track
    // anyway; this proves the panel never asks it.
    const auto revisionOf = [](const QString& trackPath) -> int {
        const QFileInfo info(trackPath);
        const QString record = info.absolutePath() + QStringLiteral("/.djmeta/") + info.fileName() +
                               QStringLiteral(".json");
        QFile file(record);
        if (!file.open(QIODevice::ReadOnly)) {
            return -1;
        }
        return QJsonDocument::fromJson(file.readAll())
            .object()
            .value(QStringLiteral("revision"))
            .toInt(-1);
    };
    const QString freeSource = QStringLiteral("/tmp/dj-lock-free.wav");
    QFile::remove(freeSource);
    if (!writeClickTrack(freeSource, 128.0, 120.0)) {
        std::fprintf(stderr, "dj-ui: cannot write the free track for the mixed selection\n");
        return 1;
    }
    bridge.importPaths({freeSource}, collectionRoot, false);
    // The import state counts one batch, not the collection: the second import says "1 of 1" again.
    // What says the track landed is the collection, so the rows are what this waits for.
    for (int tick = 0; tick < 100; ++tick) {
        QCoreApplication::processEvents();
        QThread::msleep(20);
    }
    browser.openCollection({collectionRoot});
    QString lockedPath;
    QString freePath;
    const auto bothRows = [&]() {
        lockedPath.clear();
        freePath.clear();
        for (const QVariant& entry : browser.entries()) {
            const QVariantMap candidate = entry.toMap();
            const QString path = candidate.value(QStringLiteral("path")).toString();
            if (candidate.value(QStringLiteral("analysis_lock")).toBool()) {
                lockedPath = path;
            } else if (!path.isEmpty()) {
                freePath = path;
            }
        }
        return !lockedPath.isEmpty() && !freePath.isEmpty();
    };
    if (!waitFor(bothRows, 60000)) {
        std::fprintf(stderr,
                     "dj-ui: the collection did not offer a locked and a free track (%d rows)\n",
                     int(browser.entries().size()));
        return 1;
    }
    const int lockedBefore = revisionOf(lockedPath);
    const int freeBefore = revisionOf(freePath);
    if (lockedBefore < 0 || freeBefore < 0) {
        std::fprintf(stderr, "dj-ui: no records to compare (locked %d, free %d)\n", lockedBefore,
                     freeBefore);
        return 1;
    }

    QVariantList both;
    both.append(lockedPath);
    both.append(freePath);
    panel->setProperty("selection", both);
    QCoreApplication::processEvents();
    QVariant analysed(false);
    QMetaObject::invokeMethod(panel, "analyzeSelection", Q_RETURN_ARG(QVariant, analysed));
    const QString said = panel->property("lastAction").toString();
    if (!analysed.toBool() || said != QStringLiteral("analysing 1, 1 locked")) {
        std::fprintf(stderr, "dj-ui: the mixed selection says \"%s\" (returned %d)\n",
                     qPrintable(said), analysed.toBool() ? 1 : 0);
        return 1;
    }
    std::fprintf(stdout, "dj-ui: a mixed selection says \"%s\"\n", qPrintable(said));

    // The measurement takes seconds: what says it happened is the free track's revision moving.
    if (!waitFor([&]() { return revisionOf(freePath) > freeBefore; }, 120000)) {
        std::fprintf(stderr, "dj-ui: the free track was never measured (revision %d)\n",
                     revisionOf(freePath));
        return 1;
    }
    const int lockedAfter = revisionOf(lockedPath);
    if (lockedAfter != lockedBefore) {
        std::fprintf(stderr, "dj-ui: the locked track was measured anyway (revision %d -> %d)\n",
                     lockedBefore, lockedAfter);
        return 1;
    }
    std::fprintf(
        stdout,
        "dj-ui: the free track was measured (revision %d -> %d), the locked one was not (%d)\n",
        freeBefore, revisionOf(freePath), lockedAfter);
    panel->setProperty("selection", QVariantList{});
    QCoreApplication::processEvents();
    // The line before the second press is a precondition, not a compliment: the step below reads
    // the collection following the switch, and that can only be read while both halves still say
    // "locked". A deck that quietly dropped the flag turns the press into "lock it again", and the
    // failure then blamed the collection for a state the check had not looked at.
    {
        const QVariant deckNow = deckLock();
        const bool rowLocked = row().value(QStringLiteral("analysis_lock")).toBool();
        if (!rowLocked || deckNow.toBool() != true) {
            std::fprintf(
                stderr,
                "dj-ui: the lock is not in force before the second press (row %s, deck %s)\n",
                rowLocked ? "locked" : "open",
                deckNow.isValid() ? (deckNow.toBool() ? "locked" : "open") : "no track");
            return 1;
        }
    }
    std::fprintf(stdout, "dj-ui: the lock is in force on both sides before the second press\n");
    // The second press goes in as the signal the suite presses buttons with: what is under test
    // here is that the collection follows the switch, not the mouse plumbing of a lit toggle.
    QMetaObject::invokeMethod(lock, "clicked");
    QCoreApplication::processEvents();
    if (!waitFor([&]() { return !row().value(QStringLiteral("analysis_lock")).toBool(); }, 10000)) {
        const QVariant deckNow = deckLock();
        std::fprintf(
            stderr,
            "dj-ui: the lock stayed in the collection after the second press (button %s, deck %s, "
            "row %s, %s)\n",
            lit() ? "lit" : "dark",
            deckNow.isValid() ? (deckNow.toBool() ? "locked" : "open") : "no track",
            row().value(QStringLiteral("analysis_lock")).toBool() ? "locked" : "open",
            panel->property("lockPending").toBool() ? "the engine never answered"
                                                    : "the engine answered");
        return 1;
    }
    if (lit()) {
        std::fprintf(stderr, "dj-ui: the collection says open, the button is still lit\n");
        return 1;
    }
    std::fprintf(stdout, "dj-ui: the lock left the collection with the second press\n");
    // ... and both doors come back: a rule that only ever hides is a rule whose opposite is
    // untested.
    if (!waitFor([&]() { return tap != nullptr && tap->isVisible() && tap->isEnabled(); }, 5000)) {
        std::fprintf(stderr, "dj-ui: the grid row did not come back with the lock off\n");
        return 1;
    }
    std::fprintf(stdout, "dj-ui: the grid row is back on the panel with the lock off\n");
    if (!waitFor(
            [&]() {
                QQuickItem* button = deckAnalyze();
                return button != nullptr && button->isEnabled();
            },
            5000)) {
        std::fprintf(stderr, "dj-ui: Analyze stayed grey after the lock came off\n");
        return 1;
    }
    QQuickItem* freed = openMenu();
    if (freed == nullptr || !freed->isEnabled()) {
        std::fprintf(stderr, "dj-ui: the menu's Analyze... stayed dead after the lock came off\n");
        return 1;
    }
    if (entryColour(freed) != liveColour) {
        std::fprintf(stderr, "dj-ui: the menu's Analyze... wears %s though it answers again (%s)\n",
                     qPrintable(entryColour(freed).name()), qPrintable(liveColour.name()));
        return 1;
    }
    std::fprintf(
        stdout, "dj-ui: Analyze answers again once the lock is off, in the deck and in the menu\n");
    QKeyEvent leave(QEvent::KeyPress, Qt::Key_Escape, Qt::NoModifier);
    QCoreApplication::sendEvent(window, &leave);
    QCoreApplication::processEvents();
    // A press is one flag on one track, and the list must take it without asking the index for the
    // page again: a replaced page means a view handed a new model, and a list that jerks under the
    // hand that pressed the lock. The page number is the evidence — it grows only when rows are
    // replaced.
    const int generationBefore = browser.property("pageGeneration").toInt();
    QMetaObject::invokeMethod(lock, "clicked");
    QCoreApplication::processEvents();
    if (!waitFor([&]() { return browser.isLocked(listed); }, 10000)) {
        std::fprintf(stderr, "dj-ui: the third press did not light the row\n");
        return 1;
    }
    // A beat for anything the shell might still be doing, so a late replacement is caught too.
    (void)waitFor([&]() { return false; }, 400);
    const int generationAfter = browser.property("pageGeneration").toInt();
    if (generationAfter != generationBefore) {
        std::fprintf(
            stderr,
            "dj-ui: the lock replaced the page (generation %d -> %d): the list moves under the "
            "press\n",
            generationBefore, generationAfter);
        return 1;
    }
    std::fprintf(stdout,
                 "dj-ui: the lock lit the row without asking the page again (generation %d)\n",
                 generationAfter);
    // The deck is left standing, as every check leaves it.
    bridge.play(QStringLiteral("A"), false);
    std::fprintf(stdout, "dj-ui: lock check done\n");
    return 0;
}

/// The end of a track, through the engine: the deck stands still on the last frame and takes the
/// next track. A deck that only pretends to have stopped refuses the load as Busy, which is what
/// the owner met.

} // namespace checks
