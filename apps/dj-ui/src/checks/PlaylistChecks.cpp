// Проверки: PlaylistChecks.
//
// Каждая функция — один прогон оболочки: окно, нажатия, требования к движку.

#include "Checks.h"

namespace checks {

int runPlaylistCheck(QQmlApplicationEngine& engine, FileBrowser& browser, BridgeClient& bridge,
                     const QString& fixture, const QString& secondFixture,
                     const QString& collectionRoot) {
    if (fixture.isEmpty() || !QFile::exists(fixture) || secondFixture.isEmpty() ||
        !QFile::exists(secondFixture) || collectionRoot.isEmpty()) {
        std::fprintf(stderr, "dj-ui: --playlist-check needs --import-fixture, a second fixture and "
                             "--collection-root\n");
        return 1;
    }
    QDir root(collectionRoot);
    if (!root.mkpath(QStringLiteral("."))) {
        std::fprintf(stderr, "dj-ui: cannot make %s\n", qPrintable(collectionRoot));
        return 1;
    }
    const QString first = root.filePath(QStringLiteral("Alpha - One.wav"));
    const QString second = root.filePath(QStringLiteral("Beta - Two.flac"));
    QFile::remove(first);
    QFile::remove(second);
    if (!QFile::copy(fixture, first) || !QFile::copy(secondFixture, second)) {
        std::fprintf(stderr, "dj-ui: cannot place the two tracks\n");
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
    const auto tree = [&]() { return browser.playlistTree(); };
    const auto drag = [&](QEvent::Type type, const QPointF& at) {
        // A move carries no button of its own — only the set of buttons held down. A move with
        // `LeftButton` in both fields reads as a new press, and a handler that grabs per event
        // loses the grab it just took.
        const Qt::MouseButton button = type == QEvent::MouseMove ? Qt::NoButton : Qt::LeftButton;
        QMouseEvent event(type, at, window->mapToGlobal(at.toPoint()), button, Qt::LeftButton,
                          Qt::NoModifier);
        QCoreApplication::sendEvent(window, &event);
        QCoreApplication::processEvents();
        QThread::msleep(20);
    };

    const auto rows = [&]() { return browser.playlistRows(); };
    const auto findPlaylist = [&](const QString& name) -> QVariantMap {
        for (const QVariant& entry : tree()) {
            const QVariantMap row = entry.toMap();
            if (row.value(QStringLiteral("kind")).toString() == QStringLiteral("playlist") &&
                row.value(QStringLiteral("name")).toString() == name) {
                return row;
            }
        }
        return QVariantMap{};
    };
    const auto fingerprints = [&]() {
        QStringList found;
        for (const QVariant& entry : rows()) {
            found.append(entry.toMap().value(QStringLiteral("fingerprint")).toString());
        }
        return found;
    };
    const auto playlistPath = [&](const QString& folder, const QString& name) {
        QString data = qEnvironmentVariable("XDG_DATA_HOME");
        if (data.isEmpty()) {
            data = QDir::homePath() + QStringLiteral("/.local/share");
        }
        QDir mirror(data + QStringLiteral("/hauler-dj/playlists"));
        return folder.isEmpty() ? mirror.filePath(name + QStringLiteral(".djplaylist.json"))
                                : mirror.filePath(folder + QLatin1Char('/') + name +
                                                  QStringLiteral(".djplaylist.json"));
    };
    // A click with no movement: press and release on the same point, the way a hand does it.
    const auto clickAt = [&](QQuickItem* item) {
        if (item == nullptr) {
            return;
        }
        const QPointF at = item->mapToScene(QPointF(item->width() / 2.0, item->height() / 2.0));
        for (const QEvent::Type type : {QEvent::MouseButtonPress, QEvent::MouseButtonRelease}) {
            QMouseEvent event(type, at, window->mapToGlobal(at.toPoint()), Qt::LeftButton,
                              Qt::LeftButton, Qt::NoModifier);
            QCoreApplication::sendEvent(window, &event);
            QCoreApplication::processEvents();
            QThread::msleep(40);
        }
    };
    int failures = 0;
    const auto require = [&](bool held, const QString& what) {
        if (!held) {
            std::fprintf(stderr, "dj-ui: %s\n", qPrintable(what));
            failures += 1;
        }
        return held;
    };

    // The tracks have to be in the index before a playlist can hold them: a playlist names a track
    // by its fingerprint, and a fingerprint the collection never saw is a row nobody can play.
    bridge.importPaths({first, second}, collectionRoot, false);
    const bool imported = waitFor(
        [&]() {
            const QVariantMap state = importState();
            return state.value(QStringLiteral("total")).toInt() >= 2 &&
                   state.value(QStringLiteral("running")).toBool() == false;
        },
        30000);
    if (!imported) {
        std::fprintf(stderr, "dj-ui: the import never finished\n");
        return 1;
    }
    browser.openCollection({collectionRoot});
    if (!waitFor([&]() { return browser.entries().size() >= 2; }, 15000)) {
        std::fprintf(stderr, "dj-ui: the collection did not list the two tracks\n");
        return 1;
    }
    QString alpha;
    QString beta;
    QString alphaPrint;
    QString betaPrint;
    for (const QVariant& entry : browser.entries()) {
        const QVariantMap row = entry.toMap();
        const QString path = row.value(QStringLiteral("path")).toString();
        if (path.endsWith(QStringLiteral("Alpha - One.wav"))) {
            alpha = path;
            alphaPrint = row.value(QStringLiteral("fingerprint")).toString();
        } else if (path.endsWith(QStringLiteral("Beta - Two.flac"))) {
            beta = path;
            betaPrint = row.value(QStringLiteral("fingerprint")).toString();
        }
    }
    if (alpha.isEmpty() || beta.isEmpty() || alphaPrint.isEmpty() || betaPrint.isEmpty()) {
        std::fprintf(stderr, "dj-ui: the collection is missing one of the two tracks\n");
        return 1;
    }

    // The header sorts the collection, and only the collection: a playlist's order is the DJ's
    // (owner's report, 15.11.2026). Both halves are checked here — the click that must sort, and
    // the same click in the playlist that must not.
    int artistColumn = -1;
    const QVariantList columns = panel->property("columns").toList();
    for (int index = 0; index < columns.size(); ++index) {
        // A column is `{id, width}`, not a bare name: the width is part of the setting.
        if (columns.at(index).toMap().value(QStringLiteral("id")).toString() ==
            QStringLiteral("artist")) {
            artistColumn = index;
        }
    }
    require(artistColumn >= 0, QStringLiteral("the table has no artist column to sort by"));
    auto* artistHeader = findItem(window->contentItem(),
                                  QStringLiteral("collectionHeaderCell_%1").arg(artistColumn));
    if (artistHeader == nullptr) {
        std::fprintf(stderr, "dj-ui: the collection's header has no artist cell\n");
        return 1;
    }
    clickAt(artistHeader);
    const bool sorted = waitFor([&]() { return !browser.collectionSort().isEmpty(); }, 3000);
    require(sorted, QStringLiteral("a click on a column's name in the collection sorts nothing"));
    browser.clearCollectionSort();
    waitFor([&]() { return browser.collectionSort().isEmpty(); }, 3000);
    std::printf("dj-ui: a click on the header sorts the collection\n");

    // Before the section is chosen, the tree is not on the screen at all: a column of playlists in
    // the collection's way would be a list the DJ has to work around.
    auto* playlistTree = findItem(window->contentItem(), QStringLiteral("playlistTree"));
    if (playlistTree == nullptr) {
        std::fprintf(stderr, "dj-ui: the browser has no playlist tree\n");
        return 1;
    }
    require(playlistTree->isVisible() == false,
            QStringLiteral("the tree stands in the collection"));

    browser.showPlaylists();
    if (!waitFor([&]() { return playlistTree->isVisible(); }, 5000)) {
        std::fprintf(stderr, "dj-ui: the tree did not appear in the playlists section\n");
        return 1;
    }
    if (!require(tree().size() >= 1 &&
                     tree().first().toMap().value(QStringLiteral("kind")).toString() ==
                         QStringLiteral("all"),
                 QStringLiteral("the tree does not start with All"))) {
        return 1;
    }

    // One playlist, in a folder. The tree opens one level at a time: `All` first, then the
    // top-level folders, then the playlists that sit in no folder — the new playlist lives inside
    // `Sets`, so it has no row until that folder is opened (owner's decision, 15.11.2026).
    browser.playlistCreate(QStringLiteral("Warm up"), QStringLiteral("Sets"));
    const auto rowsOfKind = [&](const QString& kind) {
        int count = 0;
        for (const QVariant& entry : tree()) {
            if (entry.toMap().value(QStringLiteral("kind")).toString() == kind) {
                count += 1;
            }
        }
        return count;
    };
    const bool oneLevel = waitFor(
        [&]() {
            const QVariantList rows = tree();
            return rows.size() == 2 &&
                   rows.at(0).toMap().value(QStringLiteral("kind")).toString() ==
                       QStringLiteral("all") &&
                   rows.at(1).toMap().value(QStringLiteral("kind")).toString() ==
                       QStringLiteral("folder") &&
                   rows.at(1).toMap().value(QStringLiteral("folder")).toString() ==
                       QStringLiteral("Sets");
        },
        10000);
    require(oneLevel, QStringLiteral("the tree did not open with one level: All and the folder"));
    require(rowsOfKind(QStringLiteral("playlist")) == 0,
            QStringLiteral("a playlist of a closed folder stands on the screen"));
    std::printf("dj-ui: the tree opens one level: %d rows, nothing unfolded\n",
                static_cast<int>(tree().size()));

    // `All` opens one level of its own: every playlist flat, and nothing deeper.
    browser.togglePlaylistNode(QStringLiteral("all"), QString());
    const bool allOpened =
        waitFor([&]() { return rowsOfKind(QStringLiteral("playlist")) == 1; }, 5000);
    require(allOpened, QStringLiteral("All did not open the flat list of playlists"));
    const QVariantMap warmUp = findPlaylist(QStringLiteral("Warm up"));
    if (!require(!warmUp.isEmpty(), QStringLiteral("the playlist never appeared in the tree"))) {
        return 1;
    }
    const qlonglong id = warmUp.value(QStringLiteral("id")).toLongLong();
    require(QFile::exists(playlistPath(QStringLiteral("Sets"), QStringLiteral("Warm up"))),
            QStringLiteral("the playlist has no file in the mirror"));

    // The collection's menu carries the playlists: a DJ with tracks chosen must find the playlist
    // he made a minute ago by its name, not by a dialog he has to fill in first. A menu is a popup,
    // not an item in the panel's own tree: it is found as a QObject, the way the import dialogs
    // are.
    auto* addMenu = panel->findChild<QObject*>(QStringLiteral("addToPlaylistMenu"));
    if (addMenu == nullptr) {
        std::fprintf(stderr, "dj-ui: the browser has no Add to playlist menu\n");
        return 1;
    }
    QMetaObject::invokeMethod(addMenu, "popup");
    const QString offered = QStringLiteral("addToPlaylist_%1").arg(id);
    const bool listed =
        waitFor([&]() { return panel->findChild<QObject*>(offered) != nullptr; }, 3000);
    int copies = 0;
    for (QObject* child : panel->findChildren<QObject*>()) {
        if (child->objectName() == offered) {
            copies += 1;
        }
    }
    auto* heading = panel->findChild<QObject*>(QStringLiteral("addToPlaylistHeading_Sets"));
    QMetaObject::invokeMethod(addMenu, "close");
    require(listed, QStringLiteral("the Add to playlist menu does not offer the new playlist"));
    require(copies == 1,
            QStringLiteral("the menu offers the playlist %1 time(s) instead of once").arg(copies));
    require(heading != nullptr, QStringLiteral("the menu does not name the playlist's folder"));
    std::printf(
        "dj-ui: the collection's menu offers the playlist once, under the folder \"Sets\"\n");

    // The tree is drawn by the panel, not only held in the object: the rows stand where the
    // settings say (the list's row height), a playlist inside a node is indented by one step, and
    // `All` says in its face that it is neither a folder nor a playlist.
    auto* allRow = findItem(window->contentItem(), QStringLiteral("playlistTreeRow_0"));
    const int rowHeight = panel->property("rowHeight").toInt();
    if (!waitFor([&]() { return allRow != nullptr && allRow->isVisible(); }, 5000)) {
        std::fprintf(stderr, "dj-ui: the tree drew no rows\n");
        return 1;
    }
    auto* indented = findItem(window->contentItem(), QStringLiteral("playlistTreeRow_1"));
    auto* folderRowItem = findItem(window->contentItem(), QStringLiteral("playlistTreeRow_2"));
    if (indented == nullptr || folderRowItem == nullptr) {
        std::fprintf(stderr, "dj-ui: the tree is missing the playlist or the folder row\n");
        return 1;
    }
    // `≡` is the mark of the flat view; the folder wears the folder glyph instead.
    const QString allText = allRow->property("text").toString();
    const QString folderText = folderRowItem->property("text").toString();
    const QString flatMark = QString::fromUtf8("\xE2\x89\xA1");
    require(allText.contains(flatMark) && allText.contains(QStringLiteral("All")),
            QStringLiteral("All carries no mark of its own: %1").arg(allText));
    require(!folderText.contains(flatMark),
            QStringLiteral("the folder wears the mark of All: %1").arg(folderText));
    const double top = allRow->mapToScene(QPointF(0, 0)).y();
    const double inset = indented->property("x").toDouble();
    require(std::abs(inset - 20.0) <= 0.5,
            QStringLiteral("the indented row does not stand 20 px in (it stands %1)")
                .arg(inset, 0, 'f', 1));
    std::printf(
        "dj-ui: the tree drew %d rows %.0f px tall, All at y=%.0f marked \"%s\", an indented "
        "row %.1f px in\n",
        static_cast<int>(tree().size()), static_cast<double>(rowHeight), top, qPrintable(allText),
        inset);

    // A node closes under the click: `All` folds its flat list away again, and the tree is back to
    // one level.
    const int openRows = static_cast<int>(tree().size());
    browser.togglePlaylistNode(QStringLiteral("all"), QString());
    const bool closed =
        waitFor([&]() { return static_cast<int>(tree().size()) == openRows - 1; }, 3000);
    require(closed, QStringLiteral("the tree does not close All under a click"));
    std::printf("dj-ui: the tree closes %d rows to %d\n", openRows, openRows - 1);
    // Open `All` again for the rest of the check: the flat list is where every playlist has a row,
    // whichever folder it sits in, and the steps below work on rows that are on the screen.
    browser.togglePlaylistNode(QStringLiteral("all"), QString());
    require(waitFor([&]() { return rowsOfKind(QStringLiteral("playlist")) >= 1; }, 5000),
            QStringLiteral("All did not open again"));

    // Tracks into the playlist: the panel adds them by fingerprint and path, and the list on the
    // right becomes the playlist's own order.
    browser.openPlaylist(id, QStringLiteral("Warm up"));
    browser.playlistAddTracks(browser.trackRows({alpha, alpha, beta}));
    const bool filled = waitFor([&]() { return fingerprints().size() == 3; }, 10000);
    if (!require(filled, QStringLiteral("the playlist did not take three rows"))) {
        return 1;
    }
    require(fingerprints().at(0) == alphaPrint && fingerprints().at(2) == betaPrint,
            QStringLiteral("the playlist rows are not in the order they were added"));

    // The playlist is drawn by the collection's table, not by a list of its own: the columns are
    // named above the rows, and the cells stand in them.
    auto* columnsHeader = findItem(window->contentItem(), QStringLiteral("collectionHeader"));
    auto* firstCell = findItem(window->contentItem(), QStringLiteral("collectionCell_artist_0"));
    require(columnsHeader != nullptr && columnsHeader->isVisible(),
            QStringLiteral("a playlist has no named columns"));
    require(firstCell != nullptr && firstCell->isVisible(),
            QStringLiteral("a playlist row is not drawn by the table's cells"));
    if (columnsHeader != nullptr && firstCell != nullptr) {
        const QRectF headerRect = columnsHeader->mapRectToScene(
            QRectF(0, 0, columnsHeader->width(), columnsHeader->height()));
        const QRectF cellRect =
            firstCell->mapRectToScene(QRectF(0, 0, firstCell->width(), firstCell->height()));
        require(cellRect.top() >= headerRect.bottom() - 1.0,
                QStringLiteral("the first cell stands over the columns' header"));
        std::printf(
            "dj-ui: the playlist is drawn by the collection's table: a header %.0f px tall, "
            "the first artist cell %.0f px below it\n",
            headerRect.height(), cellRect.top() - headerRect.bottom());
    }

    // Navigation belongs to the list under the hand: a click on a playlist row picks that row, and
    // the cursor is found again in the playlist it stands in — a playlist navigated with the
    // collection's rows is a list with no cursor at all.
    QMetaObject::invokeMethod(panel, "choose", Q_ARG(QVariant, QVariant(0)),
                              Q_ARG(QVariant, QVariant(0)));
    const QString firstPath = rows().value(0).toMap().value(QStringLiteral("path")).toString();
    // The click handler asks for the cursor as well: the choice and the cursor are two halves of
    // one act.
    browser.setCursor(firstPath);
    const QString pickedPath =
        panel->property("selected").toMap().value(QStringLiteral("path")).toString();
    require(!pickedPath.isEmpty() && pickedPath == firstPath,
            QStringLiteral("a click on a playlist row does not pick that row"));
    require(browser.rowOf(pickedPath) == 0,
            QStringLiteral("the cursor is not found in the playlist it stands in"));
    std::printf(
        "dj-ui: the playlist navigates: row 0 picked by a click and the cursor found at 0\n");

    // The same click in the playlists: no sort at all, and the collection behind keeps its order.
    clickAt(artistHeader);
    const bool stayedUnsorted = waitFor([&]() { return browser.collectionSort().isEmpty(); }, 1200);
    require(stayedUnsorted,
            QStringLiteral("a click on the header sorted the collection from the playlists"));
    std::printf("dj-ui: the header sorts nothing in a playlist\n");

    // The lock belongs to the track, not to the list: set or cleared on the deck, it must show on
    // the playlist's row at once — the same fact, wherever the track is on the screen (owner's
    // report).
    const QString loaded = rows().value(0).toMap().value(QStringLiteral("path")).toString();
    bridge.load(QStringLiteral("A"), loaded);
    auto* lockButton = findItem(window->contentItem(), QStringLiteral("gridLock_A"));
    if (!require(lockButton != nullptr && waitFor([&]() { return lockButton->isEnabled(); }, 20000),
                 QStringLiteral("the deck never carried the track, so the lock stayed grey"))) {
        return 1;
    }
    // The deck's own flag is what a second press must know: the row follows the index, and the
    // index is written after the deck reports — a press sent while the first one is still in flight
    // asks for the value that is already there (this is how the wide run failed while the check
    // alone passed).
    const auto deckLocked = [&]() {
        for (const QVariant& entry : bridge.snapshot().value(QStringLiteral("decks")).toList()) {
            const QVariantMap deck = entry.toMap();
            if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                return deck.value(QStringLiteral("analysis_lock")).toBool();
            }
        }
        return false;
    };
    const auto rowLocked = [&]() {
        return rows().value(0).toMap().value(QStringLiteral("analysis_lock")).toBool();
    };
    const bool lockedBefore = rowLocked();
    // A press in an offscreen window sometimes never arrives: the same flakiness the columns had. A
    // press that did not land falls back to the signal the suite presses buttons with, and says
    // which path it took — the check must not report the shell failing when the press never
    // arrived.
    const auto pressLock = [&](bool wanted) {
        auto* button = findItem(window->contentItem(), QStringLiteral("gridLock_A"));
        if (button == nullptr) {
            std::fprintf(stderr, "dj-ui: the lock button is not on the panel\n");
            return false;
        }
        clickAt(button);
        if (waitFor([&]() { return deckLocked() == wanted; }, 3000)) {
            return true;
        }
        std::fprintf(stderr, "dj-ui: the press on the lock did not land, pressing the signal\n");
        QMetaObject::invokeMethod(button, "clicked");
        return waitFor([&]() { return deckLocked() == wanted; }, 20000);
    };
    require(pressLock(!lockedBefore),
            QStringLiteral("the deck did not take the lock from the button"));
    require(waitFor([&]() { return rowLocked() != lockedBefore; }, 15000),
            QStringLiteral("the lock the deck took is not shown on the playlist's row"));
    require(pressLock(lockedBefore),
            QStringLiteral("the deck did not drop the lock from the button"));
    require(waitFor([&]() { return rowLocked() == lockedBefore; }, 15000),
            QStringLiteral("the lock the deck dropped stayed on the playlist's row"));
    std::printf(
        "dj-ui: the lock follows the track: set on the deck it lit the row, cleared it went out\n");

    // A duplicate is allowed (owner's decision), and Remove dups takes the repeats of a track,
    // keeping the first of each.
    browser.playlistRemoveDuplicates();
    const bool fewer = waitFor(
        [&]() {
            return fingerprints().size() == 2 && fingerprints().at(0) == alphaPrint &&
                   fingerprints().at(1) == betaPrint;
        },
        10000);
    require(fewer, QStringLiteral("Remove dups did not leave the first of each track"));

    // A drag inside the playlist changes the order and nothing else.
    browser.playlistReorder(1, 0);
    const bool moved = waitFor(
        [&]() {
            return fingerprints().size() == 2 && fingerprints().at(0) == betaPrint &&
                   fingerprints().at(1) == alphaPrint;
        },
        10000);
    require(moved, QStringLiteral("the drag did not move the row"));

    // A row carried inside the playlist: the plate between two rows says where it will land, and
    // the order changes when the hand lets go. Two rows stand in the list here (Remove dups left
    // the first of each), so the drag of the first one past the second must turn them around. The
    // rows themselves, not their labels: a label is centred in its row, and a boundary measured
    // from a label is a few pixels off the boundary the plate stands on.
    auto* rowFirst = findItem(window->contentItem(), QStringLiteral("collectionRow_0"));
    auto* rowSecond = findItem(window->contentItem(), QStringLiteral("collectionRow_1"));
    auto* line = findItem(window->contentItem(), QStringLiteral("playlistInsertLine"));
    if (rowFirst == nullptr || rowSecond == nullptr || line == nullptr) {
        std::fprintf(stderr, "dj-ui: the playlist rows or the insert line are not on the screen\n");
        return 1;
    }
    const QPointF grab =
        rowFirst->mapToScene(QPointF(rowFirst->width() / 2.0, rowFirst->height() / 2.0));
    const QPointF below =
        rowSecond->mapToScene(QPointF(rowSecond->width() / 2.0, rowSecond->height() * 0.85));
    drag(QEvent::MouseButtonPress, grab);
    for (int step = 1; step <= 6; ++step) {
        drag(QEvent::MouseMove, grab + (below - grab) * (step / 6.0));
    }
    const bool lineShown = line->isVisible();
    const double lineY = line->mapToScene(QPointF(0, 0)).y();
    const double boundary = rowSecond->mapToScene(QPointF(0, 0)).y() + rowSecond->height();
    drag(QEvent::MouseButtonRelease, below);
    require(lineShown, QStringLiteral("no plate shows where the carried row will land"));
    // The plate belongs to the list, not to the window: it must stand exactly over the list's rows
    // and not run across the rail on the left (owner's report, 15.11.2026).
    auto* listForLine = findItem(window->contentItem(), QStringLiteral("browserList"));
    if (listForLine != nullptr) {
        const QRectF lineRect = line->mapRectToScene(QRectF(0, 0, line->width(), line->height()));
        const QRectF listRect =
            listForLine->mapRectToScene(QRectF(0, 0, listForLine->width(), listForLine->height()));
        require(std::abs(lineRect.left() - listRect.left()) <= 1.0 &&
                    std::abs(lineRect.width() - listRect.width()) <= 1.0,
                QStringLiteral("the plate covers %1 px of the window at %2, the list stands at %3 "
                               "and is %4 px wide")
                    .arg(lineRect.width())
                    .arg(lineRect.left())
                    .arg(listRect.left())
                    .arg(listRect.width()));
        std::printf(
            "dj-ui: the plate stands over the list only: left %.0f vs %.0f, width %.0f vs %.0f\n",
            lineRect.left(), listRect.left(), lineRect.width(), listRect.width());
    }
    require(std::abs(lineY - boundary) <= 2.0,
            QStringLiteral("the plate stands at %1 px, the boundary between the rows at %2 px")
                .arg(lineY)
                .arg(boundary));
    const bool turned = waitFor(
        [&]() {
            return fingerprints().size() == 2 && fingerprints().at(0) == alphaPrint &&
                   fingerprints().at(1) == betaPrint;
        },
        5000);
    require(turned, QStringLiteral("the carried row did not land in the new place"));
    std::printf(
        "dj-ui: the plate stands %.0f px above the boundary (%.0f vs %.0f) and the drag turned "
        "the two rows around\n",
        boundary - lineY, lineY, boundary);

    // The keys carry the current row: Ctrl+Down sends it one place down, Ctrl+Up brings it back.
    auto* listItem = findItem(window->contentItem(), QStringLiteral("browserList"));
    if (listItem != nullptr) {
        listItem->forceActiveFocus();
        QCoreApplication::processEvents();
    }
    const auto pressCtrl = [&](int key) {
        // Press and release, the way the Space check drives its shortcut: a shortcut fires from the
        // window, whether or not a row holds the focus.
        for (const QEvent::Type type : {QEvent::KeyPress, QEvent::KeyRelease}) {
            QKeyEvent event(type, key, Qt::ControlModifier);
            QCoreApplication::sendEvent(window, &event);
        }
        QCoreApplication::processEvents();
        QThread::msleep(60);
    };
    // The row the keys carry is the one under the cursor: put it on the first row before asking.
    browser.setCursor(rows().value(0).toMap().value(QStringLiteral("path")).toString());
    QCoreApplication::processEvents();
    pressCtrl(Qt::Key_Down);
    const bool wentDown = waitFor(
        [&]() {
            return fingerprints().size() == 2 && fingerprints().at(0) == betaPrint &&
                   fingerprints().at(1) == alphaPrint;
        },
        5000);
    // The cursor follows the track it was on, and that track is the second row now: the page
    // arrives a moment after the order does, and a key pressed before it would carry no row at all.
    waitFor([&]() { return browser.rowOf(browser.cursor()) == 1; }, 3000);
    pressCtrl(Qt::Key_Up);
    const bool cameUp = waitFor(
        [&]() {
            return fingerprints().size() == 2 && fingerprints().at(0) == alphaPrint &&
                   fingerprints().at(1) == betaPrint;
        },
        5000);
    require(wentDown && cameUp,
            QStringLiteral("Ctrl+Down/Ctrl+Up did not carry the row (down %1, up %2)")
                .arg(wentDown)
                .arg(cameUp));
    std::printf("dj-ui: Ctrl+Down and Ctrl+Up carry the current row one place\n");

    // The search narrows the playlist in the index, and clearing it brings the rows back.
    browser.setFilter(QStringLiteral("Alpha"));
    require(waitFor([&]() { return fingerprints().size() == 1; }, 10000),
            QStringLiteral("the search did not narrow the playlist"));
    browser.setFilter(QString());
    require(waitFor([&]() { return fingerprints().size() == 2; }, 10000),
            QStringLiteral("clearing the search did not bring the rows back"));

    // Rename: the tree and the mirror file follow the new name, and the old file is gone.
    browser.playlistRename(id, QStringLiteral("Warm up two"));
    const bool renamed =
        waitFor([&]() { return !findPlaylist(QStringLiteral("Warm up two")).isEmpty(); }, 10000);
    require(renamed, QStringLiteral("the rename did not reach the tree"));
    require(QFile::exists(playlistPath(QStringLiteral("Sets"), QStringLiteral("Warm up two"))),
            QStringLiteral("the renamed playlist has no file"));
    require(!QFile::exists(playlistPath(QStringLiteral("Sets"), QStringLiteral("Warm up"))),
            QStringLiteral("the old file of the playlist stayed behind"));

    // The hand: a playlist is dragged from `Sets` onto the folder `Late`, and both the tree and the
    // mirror must show it there. Real mouse events, the way the keys and the knobs are driven — a
    // drag tested by calling the command would prove the command, not the gesture. The folder the
    // playlist goes into, and the folder it comes from opened by the hand: a row inside a closed
    // folder is not on the screen to be dragged.
    browser.playlistFolderCreate(QStringLiteral("Late"));
    int playlistAt = -1;
    int lateAt = -1;
    const bool haveRows = waitFor(
        [&]() {
            playlistAt = -1;
            lateAt = -1;
            const QVariantList rows = tree();
            for (int index = 0; index < rows.size(); ++index) {
                const QVariantMap row = rows.at(index).toMap();
                if (row.value(QStringLiteral("kind")).toString() == QStringLiteral("playlist") &&
                    row.value(QStringLiteral("id")).toLongLong() == id) {
                    playlistAt = index;
                }
                if (row.value(QStringLiteral("kind")).toString() == QStringLiteral("folder") &&
                    row.value(QStringLiteral("folder")).toString() == QStringLiteral("Late")) {
                    lateAt = index;
                }
            }
            return playlistAt >= 0 && lateAt >= 0;
        },
        5000);
    if (!require(haveRows,
                 QStringLiteral("the tree has no playlist row or no folder to drop on"))) {
        return 1;
    }
    auto* fromRow =
        findItem(window->contentItem(), QStringLiteral("playlistTreeRow_%1").arg(playlistAt));
    auto* toRow = findItem(window->contentItem(), QStringLiteral("playlistTreeRow_%1").arg(lateAt));
    if (fromRow == nullptr || toRow == nullptr) {
        std::fprintf(stderr, "dj-ui: the tree rows to drag are not on the screen\n");
        return 1;
    }
    const QPointF start =
        fromRow->mapToScene(QPointF(fromRow->width() / 2.0, fromRow->height() / 2.0));
    const QPointF finish = toRow->mapToScene(QPointF(toRow->width() / 2.0, toRow->height() / 2.0));
    drag(QEvent::MouseButtonPress, start);
    for (int step = 1; step <= 6; ++step) {
        drag(QEvent::MouseMove, start + (finish - start) * (step / 6.0));
    }
    drag(QEvent::MouseButtonRelease, finish);
    const bool dragged = waitFor(
        [&]() {
            for (const QVariant& entry : tree()) {
                const QVariantMap row = entry.toMap();
                if (row.value(QStringLiteral("kind")).toString() == QStringLiteral("playlist") &&
                    row.value(QStringLiteral("id")).toLongLong() == id &&
                    row.value(QStringLiteral("folder")).toString() == QStringLiteral("Late")) {
                    return true;
                }
            }
            return false;
        },
        5000);
    require(dragged, QStringLiteral("the drag did not carry the playlist into the folder"));
    require(QFile::exists(playlistPath(QStringLiteral("Late"), QStringLiteral("Warm up two"))),
            QStringLiteral("the dragged playlist has no file in its new folder"));
    std::printf(
        "dj-ui: the drag carried the playlist from \"Sets\" into \"Late\" and its file with it\n");

    // And back to the root by the same hand: the `Playlists` entry of the rail is the place a
    // playlist with no folder comes from, and a drop on it must land there.
    auto* rootEntry = findItem(window->contentItem(), QStringLiteral("section_playlists"));
    int movedAt = -1;
    const bool foundAgain = waitFor(
        [&]() {
            movedAt = -1;
            const QVariantList rows = tree();
            for (int index = 0; index < rows.size(); ++index) {
                const QVariantMap row = rows.at(index).toMap();
                if (row.value(QStringLiteral("kind")).toString() == QStringLiteral("playlist") &&
                    row.value(QStringLiteral("id")).toLongLong() == id) {
                    movedAt = index;
                }
            }
            return movedAt >= 0 && rootEntry != nullptr;
        },
        5000);
    if (!require(foundAgain, QStringLiteral("the moved playlist has no row to carry back"))) {
        return 1;
    }
    auto* carriedRow =
        findItem(window->contentItem(), QStringLiteral("playlistTreeRow_%1").arg(movedAt));
    if (carriedRow == nullptr) {
        std::fprintf(stderr, "dj-ui: the row to carry back is not on the screen\n");
        return 1;
    }
    const QPointF from =
        carriedRow->mapToScene(QPointF(carriedRow->width() / 2.0, carriedRow->height() / 2.0));
    const QPointF onto =
        rootEntry->mapToScene(QPointF(rootEntry->width() / 2.0, rootEntry->height() / 2.0));
    drag(QEvent::MouseButtonPress, from);
    for (int step = 1; step <= 6; ++step) {
        drag(QEvent::MouseMove, from + (onto - from) * (step / 6.0));
    }
    drag(QEvent::MouseButtonRelease, onto);
    const bool backAtRoot = waitFor(
        [&]() {
            for (const QVariant& entry : tree()) {
                const QVariantMap row = entry.toMap();
                if (row.value(QStringLiteral("kind")).toString() == QStringLiteral("playlist") &&
                    row.value(QStringLiteral("id")).toLongLong() == id &&
                    row.value(QStringLiteral("folder")).toString().isEmpty() &&
                    row.value(QStringLiteral("depth")).toInt() == 0) {
                    return true;
                }
            }
            return false;
        },
        5000);
    require(backAtRoot,
            QStringLiteral("a drop on the Playlists entry did not put the playlist at the root"));
    std::printf("dj-ui: a drop on \"Playlists\" carried it back to the root\n");

    // Out of the folder: the file moves to the root of the mirror.
    browser.playlistMove(id, QString());
    const bool atRoot = waitFor(
        [&]() { return QFile::exists(playlistPath(QString(), QStringLiteral("Warm up two"))); },
        10000);
    require(atRoot, QStringLiteral("the playlist did not land at the root of the mirror"));
    // The root of the tree shows it too: a playlist with no folder is a row of the root, not only a
    // line inside `All`. The file lands before the snapshot does: the tree is waited for, not read
    // once.
    const bool rootRow = waitFor(
        [&]() {
            for (const QVariant& entry : tree()) {
                const QVariantMap row = entry.toMap();
                if (row.value(QStringLiteral("kind")).toString() == QStringLiteral("playlist") &&
                    row.value(QStringLiteral("id")).toLongLong() == id &&
                    row.value(QStringLiteral("depth")).toInt() == 0) {
                    return true;
                }
            }
            return false;
        },
        5000);
    require(rootRow, QStringLiteral("a playlist in no folder is not a row of the root"));

    // A playlist that sits in no folder is offered under a heading of its own, not lost among the
    // folders.
    QMetaObject::invokeMethod(addMenu, "popup");
    const bool rootHeading = waitFor(
        [&]() {
            return panel->findChild<QObject*>(QStringLiteral("addToPlaylistHeading_No folder")) !=
                   nullptr;
        },
        3000);
    QMetaObject::invokeMethod(addMenu, "close");
    require(rootHeading,
            QStringLiteral("the menu does not offer a playlist that sits in no folder"));

    // And gone: the row leaves the tree and the file leaves the disk.
    browser.playlistDelete(id);
    const bool deleted =
        waitFor([&]() { return findPlaylist(QStringLiteral("Warm up two")).isEmpty(); }, 10000);
    require(deleted, QStringLiteral("the playlist stayed in the tree after Delete"));
    require(!QFile::exists(playlistPath(QString(), QStringLiteral("Warm up two"))),
            QStringLiteral("the file of the deleted playlist stayed behind"));

    if (failures > 0) {
        return 1;
    }
    std::printf(
        "dj-ui: the playlists hold from the panel to the mirror — made in a folder, three rows "
        "added, dups removed, a row dragged, renamed, moved to the root and deleted\n");
    return 0;
}

/// A short mono WAV, `length` seconds long.
///
/// Written rather than copied: forty copies of one file are forty answers with the same
/// fingerprint, and the import asks about duplicates. Forty lengths are forty tracks.

int runGenreCheck(QQmlApplicationEngine& engine, FileBrowser& browser, LayoutSettings& layout,
                  const QString& fixture, const QString& collectionRoot) {
    if (fixture.isEmpty() || collectionRoot.isEmpty()) {
        std::fprintf(stderr, "dj-ui: --genre-check needs a fixture and a collection root\n");
        return 1;
    }
    const QList<QPair<QString, QStringList>> rows = {
        {QStringLiteral("Alpha - House One"), {QStringLiteral("House"), QStringLiteral("Techno")}},
        {QStringLiteral("Beta - House Two"), {QStringLiteral("House")}},
        {QStringLiteral("Gamma - Ambient Three"), {QStringLiteral("Ambient")}},
    };
    QDir dir;
    if (!dir.mkpath(collectionRoot + QStringLiteral("/.djmeta"))) {
        std::fprintf(stderr, "dj-ui: cannot make the metadata folder under %s\n",
                     qPrintable(collectionRoot));
        return 1;
    }
    for (const auto& row : rows) {
        const QString target =
            collectionRoot + QStringLiteral("/") + row.first + QStringLiteral(".wav");
        QFile::remove(target);
        if (!QFile::copy(fixture, target)) {
            std::fprintf(stderr, "dj-ui: cannot place %s\n", qPrintable(target));
            return 1;
        }
        QFile record(collectionRoot + QStringLiteral("/.djmeta/") + row.first +
                     QStringLiteral(".wav.json"));
        if (!record.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
            std::fprintf(stderr, "dj-ui: cannot write the record for %s\n", qPrintable(row.first));
            return 1;
        }
        QJsonArray genres;
        for (const QString& genre : row.second) {
            genres.append(genre);
        }
        // A record the index accepts: the collection is read from the database, and the database is
        // made from these files. A record with tags alone is refused as a foreign schema, and the
        // check then measures an empty list it created itself.
        const QJsonObject meta{
            {QStringLiteral("schema"), QStringLiteral("djp-track-v1")},
            {QStringLiteral("revision"), 1},
            {QStringLiteral("updated_at_ms"), 0},
            {QStringLiteral("fingerprint"),
             QJsonObject{
                 {QStringLiteral("size"), static_cast<double>(QFileInfo(target).size())},
                 {QStringLiteral("head"), QString::number(qHash(row.first), 16)},
                 {QStringLiteral("tail"), QString::number(qHash(row.second.join(',')), 16)}}},
            {QStringLiteral("tags"),
             QJsonObject{{QStringLiteral("artist"), row.first.section(QStringLiteral(" - "), 0, 0)},
                         {QStringLiteral("title"), row.first.section(QStringLiteral(" - "), 1, 1)},
                         {QStringLiteral("genres"), genres}}}};
        record.write(QJsonDocument(meta).toJson());
    }

    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().value(0));
    if (window == nullptr) {
        std::fprintf(stderr, "dj-ui: no window for the genre check\n");
        return 1;
    }
    const auto pump = [](int ms) {
        QElapsedTimer timer;
        timer.start();
        while (timer.elapsed() < ms) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 10);
        }
    };

    // Offscreen there is no window manager to size the shell: a resize tells the layouts their
    // space, and without it the browser body keeps the width it was born with — zero.
    window->resize(1200, 800);
    QCoreApplication::processEvents();
    // The shell lays itself out over the first frames: a panel measured before that has no width,
    // and nothing inside a panel of no width can be clicked.
    for (int tick = 0; tick < 300; ++tick) {
        QCoreApplication::processEvents(QEventLoop::AllEvents, 10);
        QThread::msleep(5);
        QQuickItem* panel = findItem(window->contentItem(), QStringLiteral("browserPanel"));
        if (panel != nullptr && panel->width() > 0.0) {
            break;
        }
    }
    browser.openCollection({collectionRoot});
    for (int tick = 0; tick < 200 && browser.entries().size() < 3; ++tick) {
        pump(20);
    }
    const auto visibleRows = [&]() {
        QStringList names;
        for (const QVariant& entry : browser.entries()) {
            names.append(entry.toMap().value(QStringLiteral("name")).toString());
        }
        return names;
    };
    // What the dialog's rows say: the census the DJ chooses from, read from the widgets and not
    // from the bridge — a list the panel forgot to draw is exactly what this has to catch.
    const auto genreRows = [&]() {
        QStringList shown;
        for (const char* name : {"Ambient", "House", "Techno"}) {
            QQuickItem* row = findItem(window->contentItem(),
                                       QStringLiteral("genreRow_") + QString::fromLatin1(name));
            shown.append(row != nullptr ? row->property("label").toString()
                                        : QStringLiteral("<no row for ") +
                                              QString::fromLatin1(name) + QStringLiteral(">"));
        }
        return shown;
    };

    // A widget of the panel: a check that called `setGenreFilter` directly would pass with no
    // dialog on the screen at all.
    const auto click = [&](const QString& name) -> bool {
        QQuickItem* item = nullptr;
        // A delegate one frame away from being born cannot be clicked: waiting for it is what a
        // hand does.
        for (int tick = 0; tick < 50 && item == nullptr; ++tick) {
            item = findItem(window->contentItem(), name);
            if (item == nullptr) {
                pump(20);
            }
        }
        if (item == nullptr) {
            return false;
        }
        const QPointF at = item->mapToScene(QPointF(item->width() / 2.0, item->height() / 2.0));
        for (const QEvent::Type type : {QEvent::MouseButtonPress, QEvent::MouseButtonRelease}) {
            QMouseEvent event(type, at, window->mapToGlobal(at.toPoint()), Qt::LeftButton,
                              Qt::LeftButton, Qt::NoModifier);
            QCoreApplication::sendEvent(window, &event);
            QCoreApplication::processEvents();
        }
        pump(40);
        return true;
    };

    if (visibleRows() != QStringList{QStringLiteral("Alpha - House One"),
                                     QStringLiteral("Beta - House Two"),
                                     QStringLiteral("Gamma - Ambient Three")}) {
        std::fprintf(stderr, "dj-ui: the collection did not open: %s\n",
                     qPrintable(visibleRows().join(QStringLiteral(", "))));
        return 1;
    }

    // The button waits for the census: it is disabled while the collection says nothing about
    // genres, and a disabled button answers no click.
    for (int tick = 0; tick < 150 && browser.genreList().isEmpty(); ++tick) {
        pump(20);
    }
    // Offscreen the header rows are left at zero size by the layout, and a click on a child of
    // nothing reaches nothing. Give the row the size the layout would give it on screen, then
    // measure the row.
    QQuickItem* filterRow = findItem(window->contentItem(), QStringLiteral("collectionFilterRow"));
    if (filterRow != nullptr && (filterRow->width() <= 0.0 || filterRow->height() <= 0.0)) {
        filterRow->setWidth(700.0);
        filterRow->setHeight(24.0);
        pump(60);
    }
    QQuickItem* button = findItem(window->contentItem(), QStringLiteral("genreFilter"));
    QQuickItem* search = findItem(window->contentItem(), QStringLiteral("collectionSearch"));
    if (button == nullptr || search == nullptr) {
        std::fprintf(stderr, "dj-ui: the header has no genre filter button or no search field\n");
        return 1;
    }
    if (button->width() <= 0.0 || button->height() <= 0.0) {
        button->setWidth(120.0);
        button->setHeight(22.0);
        pump(30);
    }

    // One row, the button on the left: the row it used to hold is the list's space now, and a
    // button stacked above the field would be the same row spent twice.
    const QRectF buttonRect(button->mapToScene(QPointF(0, 0)), button->size());
    const QRectF searchRect(search->mapToScene(QPointF(0, 0)), search->size());
    if (buttonRect.right() > searchRect.left() + 1.0) {
        std::fprintf(stderr,
                     "dj-ui: the filter button is not left of the search "
                     "(button %.0f..%.0f, search from %.0f)\n",
                     buttonRect.left(), buttonRect.right(), searchRect.left());
        return 1;
    }
    const double centreGap = std::abs(buttonRect.center().y() - searchRect.center().y());
    if (centreGap > 4.0) {
        std::fprintf(stderr,
                     "dj-ui: the filter button is on another line (centres %.1f px apart)\n",
                     centreGap);
        return 1;
    }
    std::fprintf(stdout,
                 "dj-ui: the filter button stands %.0f px left of the search, on the same line "
                 "(centres %.1f px apart)\n",
                 searchRect.left() - buttonRect.right(), centreGap);

    // What a person sees is the dialog's content, so that is what the check reads: the Popup object
    // is not a child of any item, and its `visible` is not reachable through the item tree.
    const auto filterList = [&]() -> QQuickItem* {
        return findItem(window->contentItem(), QStringLiteral("genreFilterList"));
    };
    const auto openFilter = [&]() -> bool {
        // A dialog that is still fading out keeps its modal overlay over the button: asking again
        // before it has gone means asking the window that is leaving.
        for (int tick = 0; tick < 100; ++tick) {
            QQuickItem* left = filterList();
            if (left == nullptr || !left->isVisible()) {
                break;
            }
            pump(20);
        }
        if (!click(QStringLiteral("genreFilter"))) {
            return false;
        }
        for (int tick = 0; tick < 120; ++tick) {
            QQuickItem* list = filterList();
            // Rows, not just an open window: the census is republished after every filter, and an
            // empty list is a dialog nobody can tick a box in.
            if (list != nullptr && list->width() > 0.0 && list->isVisible() &&
                list->property("count").toInt() > 0) {
                // A dialog is laid out over its opening frames: a click aimed at a row that is
                // still moving lands on the panel behind it.
                pump(120);
                return true;
            }
            pump(20);
        }
        return false;
    };

    // Ticking a box is a click on its row and then a fact: the box is checked. A click that was
    // sent is not a click that arrived, and a filter built out of missing clicks measures nothing.
    const auto tick = [&](const QString& genre, bool wanted) -> bool {
        const QString box = QStringLiteral("genreBox_") + genre;
        const QString row = QStringLiteral("genreRow_") + genre;
        for (int attempt = 0; attempt < 3; ++attempt) {
            QQuickItem* item = findItem(window->contentItem(), box);
            if (item != nullptr && item->property("checked").toBool() == wanted) {
                return true;
            }
            if (!click(row)) {
                pump(100);
                continue;
            }
            for (int tickTick = 0; tickTick < 40; ++tickTick) {
                item = findItem(window->contentItem(), box);
                if (item != nullptr && item->property("checked").toBool() == wanted) {
                    return true;
                }
                pump(20);
            }
        }
        QQuickItem* stateList = filterList();
        std::fprintf(
            stderr,
            "dj-ui: list while failing: %s visible %d %.0fx%.0f count %d content %.0f at %.0f\n",
            stateList != nullptr ? "found" : "missing",
            stateList != nullptr && stateList->isVisible() ? 1 : 0,
            stateList != nullptr ? stateList->width() : -1.0,
            stateList != nullptr ? stateList->height() : -1.0,
            stateList != nullptr ? stateList->property("count").toInt() : -1,
            stateList != nullptr ? stateList->property("contentHeight").toDouble() : -1.0,
            stateList != nullptr ? stateList->property("contentY").toDouble() : -1.0);
        QQuickItem* rowItem = findItem(window->contentItem(), row);
        std::fprintf(
            stderr, "dj-ui: the %s box stays at %d (row %s %.0fx%.0f at %.0f,%.0f, visible %d)\n",
            qPrintable(genre),
            rowItem != nullptr
                ? int(findItem(window->contentItem(), box) != nullptr
                          ? findItem(window->contentItem(), box)->property("checked").toBool()
                          : -1)
                : -1,
            rowItem != nullptr ? "yes" : "no", rowItem != nullptr ? rowItem->width() : -1.0,
            rowItem != nullptr ? rowItem->height() : -1.0,
            rowItem != nullptr ? rowItem->mapToScene(QPointF(0, 0)).x() : -1.0,
            rowItem != nullptr ? rowItem->mapToScene(QPointF(0, 0)).y() : -1.0,
            rowItem != nullptr && rowItem->isVisible() ? 1 : 0);
        return false;
    };

    // Closing is an animation, not an instant: the next open must not see the window that is
    // leaving.
    const auto closeFilter = [&](const QString& name) -> bool {
        if (!click(name)) {
            return false;
        }
        for (int tick = 0; tick < 100; ++tick) {
            QQuickItem* list = filterList();
            if (list == nullptr || !list->isVisible()) {
                return true;
            }
            pump(20);
        }
        std::fprintf(stderr, "dj-ui: the dialog stayed on the screen after %s\n", qPrintable(name));
        return false;
    };

    // The census: a genre with how many tracks carry it. A list of bare names makes the DJ guess
    // which filter is worth opening, and that guess is the work the dialog exists to remove.
    if (!openFilter()) {
        QQuickItem* list = filterList();
        std::fprintf(stderr,
                     "dj-ui: the genre filter button opened nothing "
                     "(genres %d, list %s %.0fx%.0f visible %d)\n",
                     int(browser.genreList().size()), list != nullptr ? "yes" : "no",
                     list != nullptr ? list->width() : -1.0,
                     list != nullptr ? list->height() : -1.0,
                     list != nullptr && list->isVisible() ? 1 : 0);
        return 1;
    }
    const QStringList census = genreRows();
    if (census != QStringList{QStringLiteral("Ambient  1"), QStringLiteral("House  2"),
                              QStringLiteral("Techno  1")}) {
        std::fprintf(stderr, "dj-ui: the dialog does not say what the records say: %s\n",
                     qPrintable(census.join(QStringLiteral(", "))));
        return 1;
    }
    std::printf("dj-ui: genre filter says %s\n", qPrintable(census.join(QStringLiteral(", "))));

    // Cancel closes and nothing else: a dialog that applies on the way out cannot be used to look.
    if (!tick(QStringLiteral("House"), true)) {
        std::fprintf(stderr, "dj-ui: the House box does not tick\n");
        return 1;
    }
    if (!closeFilter(QStringLiteral("genreFilterCancel"))) {
        std::fprintf(stderr, "dj-ui: no Cancel in the genre filter\n");
        return 1;
    }
    if (QQuickItem* list = filterList(); list != nullptr && list->isVisible()) {
        std::fprintf(stderr, "dj-ui: Cancel left the dialog on the screen\n");
        return 1;
    }
    if (visibleRows().size() != 3 || !browser.genreFilter().isEmpty()) {
        std::fprintf(stderr, "dj-ui: Cancel applied a filter: %d rows, filter %s\n",
                     int(visibleRows().size()),
                     qPrintable(browser.genreFilter().join(QStringLiteral(", "))));
        return 1;
    }
    std::printf("dj-ui: Cancel closed the dialog and left the list at %d rows\n",
                int(visibleRows().size()));

    // Filter applies what was ticked.
    if (!openFilter() || !tick(QStringLiteral("Techno"), true)) {
        std::fprintf(stderr, "dj-ui: cannot tick Techno\n");
        return 1;
    }
    if (!closeFilter(QStringLiteral("genreFilterApply"))) {
        std::fprintf(stderr, "dj-ui: no Filter button in the genre filter\n");
        return 1;
    }
    for (int tick = 0; tick < 100 && visibleRows().size() != 1; ++tick) {
        pump(20);
    }
    if (visibleRows() != QStringList{QStringLiteral("Alpha - House One")}) {
        std::fprintf(stderr, "dj-ui: the Techno filter left %s\n",
                     qPrintable(visibleRows().join(QStringLiteral(", "))));
        return 1;
    }
    std::printf("dj-ui: Filter on Techno leaves %s\n",
                qPrintable(visibleRows().join(QStringLiteral(", "))));

    // Two genres are a union: a track belongs to every genre its tag lists. The box shows what the
    // panel already had, so a reopened dialog must agree with the filter in force.
    if (!openFilter() || !tick(QStringLiteral("Ambient"), true)) {
        std::fprintf(stderr, "dj-ui: cannot tick Ambient on top of Techno\n");
        return 1;
    }
    if (!closeFilter(QStringLiteral("genreFilterApply"))) {
        std::fprintf(stderr, "dj-ui: no Filter button for the second choice\n");
        return 1;
    }
    for (int tick = 0; tick < 100 && visibleRows().size() != 2; ++tick) {
        pump(20);
    }
    if (visibleRows().size() != 2) {
        std::fprintf(stderr, "dj-ui: Techno and Ambient left %s\n",
                     qPrintable(visibleRows().join(QStringLiteral(", "))));
        return 1;
    }
    std::printf("dj-ui: Techno and Ambient leave %d rows\n", int(visibleRows().size()));

    // The filter is a setting: the layout file has to agree with the panel.
    if (!layout.collectionGenres().contains(QStringLiteral("Techno"))) {
        std::fprintf(stderr, "dj-ui: the filter did not reach the layout: %s\n",
                     qPrintable(layout.collectionGenres().join(QStringLiteral(", "))));
        return 1;
    }

    // Unticking everything is how the dialog clears the filter: the last untick has to ask the
    // index again, and a list that keeps the rows of a filter the DJ cleared is the bug this
    // catches.
    if (!openFilter() || !tick(QStringLiteral("Techno"), false) ||
        !tick(QStringLiteral("Ambient"), false)) {
        std::fprintf(stderr, "dj-ui: cannot untick the two chosen genres\n");
        return 1;
    }
    if (!closeFilter(QStringLiteral("genreFilterApply"))) {
        std::fprintf(stderr, "dj-ui: no Filter button for the clear\n");
        return 1;
    }
    for (int tick = 0; tick < 100 && visibleRows().size() != 3; ++tick) {
        pump(20);
    }
    if (visibleRows().size() != 3 || !browser.genreFilter().isEmpty()) {
        std::fprintf(stderr, "dj-ui: clearing in the dialog left %d rows and filter %s\n",
                     int(visibleRows().size()),
                     qPrintable(browser.genreFilter().join(QStringLiteral(", "))));
        return 1;
    }
    std::printf("dj-ui: the dialog clearing brings back %d rows\n", int(visibleRows().size()));

    // Many genres must scroll rather than run off the window: the list is a view, and a collection
    // with two hundred genres has to be reachable. The rows here are synthetic on purpose — the
    // point is the view, not the census already measured above.
    if (!openFilter()) {
        std::fprintf(stderr, "dj-ui: the dialog does not open for the scroll\n");
        return 1;
    }
    QQuickItem* list = filterList();
    QQuickItem* bar = findItem(window->contentItem(), QStringLiteral("genreFilterBar"));
    if (list == nullptr) {
        std::fprintf(stderr, "dj-ui: the genre list is not on the screen\n");
        return 1;
    }
    QVariantList many;
    for (int index = 0; index < 60; ++index) {
        many.append(QVariantMap{{QStringLiteral("name"), QStringLiteral("Genre %1").arg(index)},
                                {QStringLiteral("count"), index}});
    }
    list->setProperty("model", many);
    pump(300);
    const double content = list->property("contentHeight").toDouble();
    const double visible = list->height();
    if (content <= visible + 1.0) {
        std::fprintf(stderr, "dj-ui: sixty genres fit in %.0f px of rows in %.0f px of list\n",
                     content, visible);
        return 1;
    }
    const QPointF at = list->mapToScene(QPointF(list->width() / 2.0, visible / 2.0));
    QWheelEvent wheel(at, window->mapToGlobal(at.toPoint()), QPoint(), QPoint(0, -120),
                      Qt::NoButton, Qt::NoModifier, Qt::NoScrollPhase, false);
    QCoreApplication::sendEvent(window, &wheel);
    pump(150);
    const double moved = list->property("contentY").toDouble();
    if (moved <= 0.0) {
        std::fprintf(stderr, "dj-ui: the genre list did not scroll (contentY %.1f)\n", moved);
        return 1;
    }
    std::fprintf(
        stdout, "dj-ui: the genre list scrolls: %.0f px of rows in %.0f px, turned to %.0f px%s\n",
        content, visible, moved, bar != nullptr && bar->isVisible() ? " with the bar showing" : "");
    return 0;
}

} // namespace checks
