// Проверки: BrowserChecks.
//
// Каждая функция — один прогон оболочки: окно, нажатия, требования к движку.

#include "Checks.h"

namespace checks {

int runBrowserCheck(QQmlApplicationEngine &engine, FileBrowser &browser,
                          LayoutSettings &layout, UiSettings &settings)
{
    // A list you cannot pick from is a picture of a list, so the check goes through the same
    // function the mouse does: select a track, activate it, read what the panel says happened.
    QTemporaryDir folder;
    if (!folder.isValid()) {
        std::fprintf(stderr, "dj-ui: cannot make a temporary folder\n");
        return 1;
    }
    QDir(folder.path()).mkpath(QStringLiteral("subfolder"));
    const QString track = folder.path() + QStringLiteral("/track.wav");
    QFile file(track);
    if (!file.open(QIODevice::WriteOnly)) {
        std::fprintf(stderr, "dj-ui: cannot write %s\n", qPrintable(track));
        return 1;
    }
    file.write("RIFF", 4);
    file.close();
    browser.open(folder.path());

    auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
    if (window == nullptr) {
        std::fprintf(stderr, "dj-ui: the root object is not a window\n");
        return 1;
    }
    QEventLoop settle;
    QTimer::singleShot(600, &settle, &QEventLoop::quit);
    settle.exec();

    QQuickItem *panel = findItem(window->contentItem(), QStringLiteral("browserPanel"));
    QQuickItem *list = findItem(window->contentItem(), QStringLiteral("browserList"));
    if (panel == nullptr || list == nullptr) {
        std::fprintf(stderr, "dj-ui: the shell has no browser\n");
        return 1;
    }
    const QVariantList entries = browser.entries();
    int fileIndex = -1;
    int folderIndex = -1;
    for (int index = 0; index < entries.size(); ++index) {
        const QVariantMap entry = entries.at(index).toMap();
        if (entry.value(QStringLiteral("dir")).toBool()) {
            folderIndex = index;
        } else if (fileIndex < 0) {
            fileIndex = index;
        }
    }
    if (fileIndex < 0 || folderIndex < 0) {
        std::fprintf(stderr, "dj-ui: the browser did not list the track and the folder\n");
        return 1;
    }

    // The list's index is bound to the remembered cursor: selecting a row is `setCursor`, not an
    // assignment to the index, which would break the binding and leave the list with no cursor at all.
    const auto selectRow = [&](int index) {
        const QVariantList rows = browser.entries();
        if (index >= 0 && index < rows.size()) {
            browser.setCursor(rows.at(index).toMap().value(QStringLiteral("path")).toString());
        }
        QCoreApplication::processEvents();
    };
    const auto invoke = [&](const char *method, int index) {
        selectRow(index);
        QVariant returned;
        QMetaObject::invokeMethod(panel, method, Q_RETURN_ARG(QVariant, returned),
                                  Q_ARG(QVariant, index));
        return panel->property("lastAction").toString();
    };
    const auto loadInto = [&](const QString &deck) {
        selectRow(fileIndex);
        QVariant returned;
        QMetaObject::invokeMethod(panel, "loadInto", Q_RETURN_ARG(QVariant, returned),
                                  Q_ARG(QVariant, deck));
        // The deck is the witness: the panel answers "loaded" only once the engine shows the track, so
        // the answer is waited for instead of read on the next line.
        if (!panel->property("lastAction").toString().startsWith(QStringLiteral("loading "))) {
            return panel->property("lastAction").toString();
        }
        QElapsedTimer wait;
        wait.start();
        while (wait.elapsed() < 60000) {
            QCoreApplication::processEvents();
            QThread::msleep(50);
            const QString now = panel->property("lastAction").toString();
            if (now.startsWith(QStringLiteral("loaded "))
                || now.contains(QStringLiteral("did not take"))) {
                return now;
            }
        }
        return panel->property("lastAction").toString();
    };

    const QString onTrack = loadInto(QStringLiteral("A"));
    // The path in the answer is the path of the row that was chosen: a load that moves the folder
    // must not make the panel report a row it never sent.
    const QString chosenPath = entries.at(fileIndex).toMap().value(QStringLiteral("path")).toString();
    const bool hasEngine = !onTrack.startsWith(QStringLiteral("no engine for "));
    if (!hasEngine) {
        // --smoke runs without an engine: the panel must refuse and say why, not claim a load.
        if (onTrack != QStringLiteral("no engine for A")) {
            std::fprintf(stderr, "dj-ui: without an engine the browser said '%s'\n",
                         qPrintable(onTrack));
            return 1;
        }
        std::fprintf(stdout, "dj-ui: no engine, the load is refused and said so\n");
    } else if (onTrack != QStringLiteral("loaded ") + chosenPath + QStringLiteral(" into A")) {
        std::fprintf(stderr, "dj-ui: the browser reported a path it did not choose (%s, wanted %s)\n",
                     qPrintable(onTrack), qPrintable(chosenPath));
        return 1;
    }
    // The selection has to hold too: a highlight that never lights is the same bug as a click
    // that never lands. Checked before the folder, because opening a folder reloads the model
    // and a fresh list correctly starts with nothing selected.
    const int selected = list->property("currentIndex").toInt();
    const QString onFolder = invoke("openFolder", folderIndex);
    std::fprintf(stdout, "dj-ui: browser track  -> %s\n", qPrintable(onTrack));
    std::fprintf(stdout, "dj-ui: browser folder -> %s\n", qPrintable(onFolder));
    if (hasEngine
        && (!onTrack.startsWith(QStringLiteral("loaded "))
            || !onTrack.endsWith(QStringLiteral("into A")))) {
        std::fprintf(stderr, "dj-ui: a track click did not load into the chosen deck\n");
        return 1;
    }
    if (!onFolder.startsWith(QStringLiteral("opened "))) {
        std::fprintf(stderr, "dj-ui: a folder click did not open the folder\n");
        return 1;
    }
    if (selected != fileIndex) {
        std::fprintf(stderr, "dj-ui: the selection did not follow the click (%d, wanted %d)\n",
                     selected, fileIndex);
        return 1;
    }
    // The click itself, delivered to the window the way a mouse delivers it: calling the action
    // proves the panel works, sending the event proves the row is reachable.
    browser.open(folder.path());
    QCoreApplication::processEvents();
    const QPointF rowPoint =
        list->mapToScene(QPointF(20.0, fileIndex * 18.0 + 9.0));
    for (const QEvent::Type type : {QEvent::MouseButtonPress, QEvent::MouseButtonRelease}) {
        QMouseEvent event(type, rowPoint, window->mapToGlobal(rowPoint.toPoint()),
                          Qt::LeftButton, Qt::LeftButton, Qt::NoModifier);
        QCoreApplication::sendEvent(window, &event);
    }
    QCoreApplication::processEvents();
    const int clicked = list->property("currentIndex").toInt();
    std::fprintf(stdout, "dj-ui: browser click at row %d selected %d\n", fileIndex, clicked);
    if (clicked != fileIndex) {
        std::fprintf(stderr, "dj-ui: a click on the browser row did not select it\n");
        return 1;
    }
    // A double click must not load: it is the gesture that ruins a transition when the hand
    // slips. Verified with real events at the row, not by reading the QML.
    browser.open(folder.path());
    QCoreApplication::processEvents();
    panel->setProperty("lastAction", QString());
    const QPointF rowCentre = list->mapToScene(QPointF(20.0, fileIndex * 18.0 + 9.0));
    for (int repeat = 0; repeat < 2; ++repeat) {
        for (const QEvent::Type type : {QEvent::MouseButtonPress, QEvent::MouseButtonRelease}) {
            QMouseEvent event(type, rowCentre, window->mapToGlobal(rowCentre.toPoint()),
                              Qt::LeftButton, Qt::LeftButton, Qt::NoModifier);
            QCoreApplication::sendEvent(window, &event);
        }
    }
    QCoreApplication::processEvents();
    const QString afterDoubleClick = panel->property("lastAction").toString();
    std::fprintf(stdout, "dj-ui: browser double click -> %s\n",
                 afterDoubleClick.isEmpty() ? "nothing" : qPrintable(afterDoubleClick));
    if (afterDoubleClick.startsWith(QStringLiteral("loaded "))) {
        std::fprintf(stderr, "dj-ui: a double click loaded a track\n");
        return 1;
    }

    // A deck button is also a load button when something is chosen.
    selectRow(fileIndex);
    auto *buttonB = findItem(window->contentItem(), QStringLiteral("deckButton_B"));
    if (buttonB == nullptr) {
        std::fprintf(stderr, "dj-ui: the browser has no deck buttons\n");
        return 1;
    }
    QMetaObject::invokeMethod(buttonB, "clicked");
    QCoreApplication::processEvents();
    // The button answers "loading" and the deck answers later: wait for the deck's word, not for the
    // request.
    QElapsedTimer deckWait;
    deckWait.start();
    while (deckWait.elapsed() < 60000
           && panel->property("lastAction").toString().startsWith(QStringLiteral("loading "))) {
        QCoreApplication::processEvents();
        QThread::msleep(50);
    }
    const QString onDeckButton = panel->property("lastAction").toString();
    std::fprintf(stdout, "dj-ui: browser deck B button -> %s\n", qPrintable(onDeckButton));
    if (hasEngine) {
        if (!onDeckButton.startsWith(QStringLiteral("loaded "))
            || !onDeckButton.endsWith(QStringLiteral("into B"))) {
            std::fprintf(stderr, "dj-ui: a deck button did not load the chosen track\n");
            return 1;
        }
    } else if (onDeckButton != QStringLiteral("no engine for B")) {
        std::fprintf(stderr, "dj-ui: without an engine the deck button said '%s'\n",
                     qPrintable(onDeckButton));
        return 1;
    }
    // The way up is a row, not a button: it is first, it is called "..", and it goes up.
    const QVariantList rows = browser.entries();
    if (rows.isEmpty() || !rows.first().toMap().value(QStringLiteral("parent")).toBool()
        || rows.first().toMap().value(QStringLiteral("name")).toString() != QStringLiteral("..")) {
        std::fprintf(stderr, "dj-ui: the browser has no \"..\" row to go up with\n");
        return 1;
    }
    const QString inside = browser.folder();
    QDir up(inside);
    const bool hasParent = up.cdUp();
    QVariant opened;
    QMetaObject::invokeMethod(panel, "openFolder", Q_RETURN_ARG(QVariant, opened),
                              Q_ARG(QVariant, 0));
    QCoreApplication::processEvents();
    if (hasParent && browser.folder() != up.absolutePath()) {
        std::fprintf(stderr, "dj-ui: \"..\" did not go up (%s, wanted %s)\n",
                     qPrintable(browser.folder()), qPrintable(up.absolutePath()));
        return 1;
    }

    // The text size is a control in the browser, and the row grows with it.
    const int small = panel->property("rowHeight").toInt();
    QQuickItem *plus = panel->findChild<QQuickItem *>(QStringLiteral("fontPlus"));
    QQuickItem *minus = panel->findChild<QQuickItem *>(QStringLiteral("fontMinus"));
    if (plus == nullptr || minus == nullptr) {
        std::fprintf(stderr, "dj-ui: the browser has no text-size control\n");
        return 1;
    }
    QMetaObject::invokeMethod(plus, "clicked");
    QCoreApplication::processEvents();
    const int bigger = panel->property("rowHeight").toInt();
    QMetaObject::invokeMethod(minus, "clicked");
    QCoreApplication::processEvents();
    const int back = panel->property("rowHeight").toInt();
    if (bigger != small + 1 || back != small) {
        std::fprintf(stderr, "dj-ui: the text size did not follow the buttons (%d, %d, %d)\n",
                     small, bigger, back);
        return 1;
    }
    // The rail's width is adjustable, and the divider is the only thing that adjusts it.
    QQuickItem *rail = panel->findChild<QQuickItem *>(QStringLiteral("browserSections"));
    QQuickItem *splitterItem = panel->findChild<QQuickItem *>(QStringLiteral("browserRailSplitter"));
    if (rail == nullptr || splitterItem == nullptr) {
        std::fprintf(stderr, "dj-ui: the browser has no sections rail or no divider\n");
        return 1;
    }
    // A layout re-polishes on the next frame: the check waits for the width instead of reading a
    // value that may not have been laid out yet.
    const auto railWidthBecomes = [&](int want) {
        for (int attempt = 0; attempt < 60; ++attempt) {
            QCoreApplication::processEvents();
            QThread::msleep(10);
            if (qAbs(rail->width() - static_cast<qreal>(want)) < 2.0) {
                return true;
            }
        }
        return false;
    };
    // The width to come back to is whatever the layout held, not a number written into the check: a
    // hand that set 130 px must not turn a red check green, and the check must not go red because the
    // layout it was handed is not the one a fresh install has.
    const qreal railBefore = rail->width();
    const int railHome = qRound(railBefore);
    panel->setProperty("railWidth", railHome + 40);
    const bool grew = railWidthBecomes(railHome + 40);
    const qreal railAfter = rail->width();
    panel->setProperty("railWidth", railHome);
    const bool shrank = railWidthBecomes(railHome);
    const qreal railBack = rail->width();
    if (!grew || !shrank || !(railAfter > railBefore + 20.0 && qAbs(railBack - railBefore) < 2.0)) {
        std::fprintf(stderr, "dj-ui: the rail width did not follow the setting (%.0f, %.0f, "
                             "%.0f)\n",
                     railBefore, railAfter, railBack);
        return 1;
    }

    // The section text is the same type as the list: one control for the browser, not two.
    // The section text is the same type as the list: one control for the browser, not two.
    // Measured on the label the rail actually renders, not on the property that feeds it.
    // Delegates of a Repeater are item children of the rail but not QObject children of the
    // panel, so the search walks the item tree (findItem) instead of findChild.
    QQuickItem *sectionRow = findItem(panel, QStringLiteral("section_files"));
    if (sectionRow == nullptr || sectionRow->childItems().isEmpty()) {
        std::fprintf(stderr, "dj-ui: the sections rail has no label to measure\n");
        return 1;
    }
    QQuickItem *sectionLabel = sectionRow->childItems().first();
    const int sectionBefore = sectionLabel->property("font").value<QFont>().pixelSize();
    QMetaObject::invokeMethod(plus, "clicked");
    QCoreApplication::processEvents();
    const int sectionAfter = sectionLabel->property("font").value<QFont>().pixelSize();
    QMetaObject::invokeMethod(minus, "clicked");
    QCoreApplication::processEvents();
    const int sectionBack = sectionLabel->property("font").value<QFont>().pixelSize();
    if (sectionAfter != sectionBefore + 1 || sectionBack != sectionBefore) {
        std::fprintf(stderr, "dj-ui: the section text did not follow the preset (%d, %d, %d)\n",
                     sectionBefore, sectionAfter, sectionBack);
        return 1;
    }

    // The header is the same browser as the list. Its title, the path, the two text-size buttons and
    // the deck buttons all follow the type size, and the buttons grow enough to keep their labels
    // inside: a list that grows under controls too small to hit is a browser only half scaled.
    QQuickItem *pathLabel = findItem(panel, QStringLiteral("browserPath"));
    QQuickItem *deckButton = findItem(panel, QStringLiteral("deckButton_A"));
    if (pathLabel == nullptr || deckButton == nullptr) {
        std::fprintf(stderr, "dj-ui: the browser header has no path or no deck button to measure\n");
        return 1;
    }
    const auto pixelSizeOf = [](QQuickItem *item) {
        return item->property("font").value<QFont>().pixelSize();
    };
    const int pathBefore = pixelSizeOf(pathLabel);
    const int deckBefore = pixelSizeOf(deckButton);
    const double buttonBefore = plus->height();
    const auto heightBecomes = [&](double want) {
        for (int attempt = 0; attempt < 60; ++attempt) {
            QCoreApplication::processEvents();
            QThread::msleep(10);
            if (plus->height() > want) {
                return true;
            }
        }
        return false;
    };
    QMetaObject::invokeMethod(plus, "clicked");
    const bool buttonsGrew = heightBecomes(buttonBefore);
    const int pathAfter = pixelSizeOf(pathLabel);
    const int deckAfter = pixelSizeOf(deckButton);
    const double buttonAfter = plus->height();
    QMetaObject::invokeMethod(minus, "clicked");
    QCoreApplication::processEvents();
    if (pathAfter != pathBefore + 1 || deckAfter != deckBefore + 1 || !buttonsGrew) {
        std::fprintf(stderr,
                     "dj-ui: the browser header did not follow the type (path %d -> %d, deck button "
                     "%d -> %d, height %.0f -> %.0f)\n",
                     pathBefore, pathAfter, deckBefore, deckAfter, buttonBefore, buttonAfter);
        return 1;
    }

    // The height of the browser is a hand's decision too. A drop writes percentages, and the two
    // numbers must not move when the type changes size: a panel that snaps back the moment the
    // font grows is a panel that never remembers how tall it was.
    QQuickItem *shellItem = findItem(window->contentItem(), QStringLiteral("shellLayout"));
    if (shellItem == nullptr || shellItem->height() <= 0.0) {
        std::fprintf(stderr, "dj-ui: the shell has no height to divide\n");
        return 1;
    }
    const double shellHeight = shellItem->height();
    panel->setHeight(shellHeight * 0.45);
    QMetaObject::invokeMethod(shellItem, "publishRowSizes");
    QCoreApplication::processEvents();
    const int browserPercent = qRound(layout.browserPercent());
    if (layout.preset() != QStringLiteral("custom") || qAbs(browserPercent - 45) > 2) {
        std::fprintf(stderr,
                     "dj-ui: dropping the browser splitter did not reach the settings (preset "
                     "\"%s\", browser %d%%)\n",
                     qPrintable(layout.preset()), browserPercent);
        return 1;
    }
    const double browserBeforeFont = panel->height();
    QMetaObject::invokeMethod(plus, "clicked");
    QCoreApplication::processEvents();
    const double browserAfterFont = panel->height();
    if (qAbs(browserAfterFont - browserBeforeFont) > 0.02 * shellHeight) {
        std::fprintf(stderr,
                     "dj-ui: the browser changed height when only the text grew (%.0f → %.0f of "
                     "%.0f)\n",
                     browserBeforeFont, browserAfterFont, shellHeight);
        return 1;
    }

    // The view numbers outlive the run: they are written to the layout file and read back by a
    // fresh instance, which is exactly what the next start does (DJP-UI-001).
    panel->setProperty("railWidth", 130);
    panel->setProperty("listFont", 14);
    QQuickItem *detail = findItem(window->contentItem(), QStringLiteral("deckDetail_A"));
    if (detail == nullptr) {
        std::fprintf(stderr, "dj-ui: the deck has no main waveform to zoom\n");
        return 1;
    }
    detail->setProperty("zoomSpan", 0.25);
    QCoreApplication::processEvents();
    if (!layout.save()) {
        std::fprintf(stderr, "dj-ui: the layout file could not be written (%s)\n",
                     qPrintable(layout.file()));
        return 1;
    }
    LayoutSettings reloaded;
    reloaded.setFile(layout.file());
    if (!reloaded.load()) {
        std::fprintf(stderr, "dj-ui: the layout file could not be read back\n");
        return 1;
    }
    if (reloaded.browserRailWidth() != 130 || reloaded.browserFont() != 14
        || qAbs(reloaded.waveformSpan() - 0.25) > 1e-9) {
        std::fprintf(stderr,
                     "dj-ui: the view did not come back as it was left (rail %d, font %d, span "
                     "%.3f)\n",
                     reloaded.browserRailWidth(), reloaded.browserFont(),
                     reloaded.waveformSpan());
        return 1;
    }

    // The Collection section: one root, every track below it, named as the import named it, with
    // the tempo and the key its metadata records. The tree here is real, with a real .djmeta, and
    // it is read through the same object the section uses.
    QTemporaryDir collection;
    if (!collection.isValid()) {
        std::fprintf(stderr, "dj-ui: cannot make a temporary collection\n");
        return 1;
    }
    const QString collectionRoot = collection.path();
    QDir(collectionRoot).mkpath(QStringLiteral("A/N/.djmeta"));
    QDir(collectionRoot).mkpath(QStringLiteral("B/B"));
    // The import parks a rejected copy here; the collection is what its owner kept.
    QDir(collectionRoot).mkpath(QStringLiteral("_duplicates/C/C"));
    const auto writeStub = [](const QString &path) {
        QFile file(path);
        if (!file.open(QIODevice::WriteOnly)) {
            return false;
        }
        file.write("RIFF", 4);
        return true;
    };
    if (!writeStub(collectionRoot + QStringLiteral("/A/N/ANH - tonight.flac"))
        || !writeStub(collectionRoot + QStringLiteral("/B/B/Bbno$ - ADD.flac"))
        || !writeStub(collectionRoot + QStringLiteral("/_duplicates/C/C/Bbno$ - ADD.mp3"))) {
        std::fprintf(stderr, "dj-ui: cannot write the collection's tracks\n");
        return 1;
    }
    QFile record(collectionRoot + QStringLiteral("/A/N/.djmeta/ANH - tonight.flac.json"));
    if (record.open(QIODevice::WriteOnly)) {
        record.write(R"({"schema":"djmeta/1","revision":1,"updated_at_ms":0,"fingerprint":{},"beatgrid":{"bpm":128.0,"first_beat_frame":0},"key":"8m"})");
        record.close();
    }
    // No root at all is the platform's own collection folder, not an empty section.
    browser.openCollection(QStringList());
    QCoreApplication::processEvents();
    if (browser.mode() != QStringLiteral("collection")
        || browser.folder() != browser.defaultCollectionRoot() || !browser.error().isEmpty()) {
        std::fprintf(stderr, "dj-ui: an unset collection did not fall back to %s (it is %s, "
                             "error «%s»)\n",
                     qPrintable(browser.defaultCollectionRoot()), qPrintable(browser.folder()),
                     qPrintable(browser.error()));
        return 1;
    }
    if (!browser.defaultCollectionRoot().endsWith(QStringLiteral("/hauler-dj/collection"))) {
        std::fprintf(stderr, "dj-ui: the default collection root is not hauler-dj/collection (%s)\n",
                     qPrintable(browser.defaultCollectionRoot()));
        return 1;
    }
    browser.openCollection({collectionRoot});
    QCoreApplication::processEvents();
    const QVariantList collected = browser.entries();
    // How many the list held before the search narrowed it: printed with the summary at the end.
    int unfiltered = 0;
    if (!hasEngine) {
        // The collection is the index, and this check runs without an engine to ask: an empty list is the
        // honest answer, not a lost collection. What the list contains is measured by
        // --collection-page-check, which brings its own engine and index.
        std::fprintf(stdout,
                     "dj-ui: without an engine the collection is empty (%d of %d), as the index says\n",
                     static_cast<int>(collected.size()), browser.total());
        if (!collected.isEmpty() || browser.mode() != QStringLiteral("collection")) {
            std::fprintf(stderr, "dj-ui: without an engine the collection is not empty (%d, mode %s)\n",
                         static_cast<int>(collected.size()), qPrintable(browser.mode()));
            return 1;
        }
    } else {
    // A rejected copy is not part of the collection: three files on disk, two in the list.
    std::fprintf(stdout, "dj-ui: collection %s -> %d tracks in mode %s\n",
                 qPrintable(browser.folder()), static_cast<int>(collected.size()),
                 qPrintable(browser.mode()));
    if (browser.mode() != QStringLiteral("collection") || browser.total() != 2
        || collected.size() != 2) {
        std::fprintf(stderr, "dj-ui: the Collection section did not list the collection (%d of %d)\n",
                     static_cast<int>(collected.size()), browser.total());
        return 1;
    }
    // Named as the import named it, and sorted by that name: the file name is the name.
    if (collected.at(0).toMap().value(QStringLiteral("name")).toString()
        != QStringLiteral("ANH - tonight")) {
        std::fprintf(stderr, "dj-ui: the collection is not sorted by name (%s)\n",
                     qPrintable(collected.at(0).toMap().value(QStringLiteral("name")).toString()));
        return 1;
    }
    // The metadata beside the track reaches its row: a tempo and a key, or neither of them.
    const QVariantMap withMeta = collected.at(0).toMap();
    const QVariantMap withoutMeta = collected.at(1).toMap();
    if (qAbs(withMeta.value(QStringLiteral("bpm")).toDouble() - 128.0) > 1e-6
        || withMeta.value(QStringLiteral("key")).toString() != QStringLiteral("8m")
        || withoutMeta.contains(QStringLiteral("bpm"))
        || withoutMeta.contains(QStringLiteral("key"))) {
        std::fprintf(stderr, "dj-ui: the metadata beside a track did not reach its row\n");
        return 1;
    }
    // One collection may live on several disks: every root is listed, in one list.
    QTemporaryDir secondRoot;
    if (!secondRoot.isValid()) {
        std::fprintf(stderr, "dj-ui: cannot make a second collection root\n");
        return 1;
    }
    QDir(secondRoot.path()).mkpath(QStringLiteral("C/C"));
    if (!writeStub(secondRoot.path() + QStringLiteral("/C/C/Cazz - one more.flac"))) {
        std::fprintf(stderr, "dj-ui: cannot write the second root's track\n");
        return 1;
    }
    browser.openCollection({collectionRoot, secondRoot.path()});
    QCoreApplication::processEvents();
    const int bothRoots = static_cast<int>(browser.entries().size());
    const QString lastOfBoth =
        browser.entries().isEmpty()
            ? QString()
            : browser.entries().last().toMap().value(QStringLiteral("name")).toString();
    std::fprintf(stdout, "dj-ui: two roots -> %d tracks, last «%s»\n", bothRoots,
                 qPrintable(lastOfBoth));
    if (bothRoots != 3 || lastOfBoth != QStringLiteral("Cazz - one more")) {
        std::fprintf(stderr, "dj-ui: two collection roots did not land in one list (%d tracks, last "
                             "«%s»)\n",
                     bothRoots, qPrintable(lastOfBoth));
        return 1;
    }
    browser.openCollection({collectionRoot});
    QCoreApplication::processEvents();

    // The search keeps what matches and the count says how much it kept.
    browser.setFilter(QStringLiteral("bbno"));
    QCoreApplication::processEvents();
    const int filtered = static_cast<int>(browser.entries().size());
    browser.setFilter(QString());
    QCoreApplication::processEvents();
    unfiltered = static_cast<int>(browser.entries().size());
    std::fprintf(stdout, "dj-ui: collection search -> %d of %d, then %d\n", filtered,
                 browser.total(), unfiltered);
    if (filtered != 1 || unfiltered != 2) {
        std::fprintf(stderr, "dj-ui: the collection search did not filter the list (%d, %d)\n",
                     filtered, unfiltered);
        return 1;
    }
    }

    // A track from the collection loads the way a file from a folder does.
    selectRow(0);
    const QVariantMap chosen = panel->property("selected").toMap();
    QVariant fromCollection;
    QMetaObject::invokeMethod(panel, "loadInto", Q_RETURN_ARG(QVariant, fromCollection),
                              Q_ARG(QVariant, QStringLiteral("C")));
    QCoreApplication::processEvents();
    QElapsedTimer collectionWait;
    collectionWait.start();
    while (collectionWait.elapsed() < 60000
           && panel->property("lastAction").toString().startsWith(QStringLiteral("loading "))) {
        QCoreApplication::processEvents();
        QThread::msleep(50);
    }
    const QString onCollection = panel->property("lastAction").toString();
    std::fprintf(stdout, "dj-ui: collection track -> %s\n", qPrintable(onCollection));
    const QString expected = QStringLiteral("loaded ")
        + chosen.value(QStringLiteral("path")).toString() + QStringLiteral(" into C");
    if (hasEngine && onCollection != expected) {
        std::fprintf(stderr, "dj-ui: a collection track did not load into the chosen deck (%s, "
                             "wanted %s)\n",
                     qPrintable(onCollection), qPrintable(expected));
        return 1;
    }
    // Without an engine there is no index, so the list is empty and there is no row to choose: the press
    // on an empty list says exactly that, and the refusal of a real track is checked above.
    if (!hasEngine && onCollection != QStringLiteral("no engine for C")
        && onCollection != QStringLiteral("nothing to load")) {
        std::fprintf(stderr, "dj-ui: without an engine the collection said '%s'\n",
                     qPrintable(onCollection));
        return 1;
    }
    // A load from the collection leaves the list in the collection: the section is a place the DJ
    // chose, not a temporary detour into the folder of the track he took out of it.
    if (browser.mode() != QStringLiteral("collection")) {
        std::fprintf(stderr, "dj-ui: a load from the collection threw the list into %s\n",
                     qPrintable(browser.mode()));
        return 1;
    }
    // And the way back: Files is the folder it was showing, with its ".." row.
    browser.showFiles();
    QCoreApplication::processEvents();
    if (browser.mode() != QStringLiteral("files") || browser.entries().isEmpty()
        || !browser.entries().first().toMap().value(QStringLiteral("parent")).toBool()) {
        std::fprintf(stderr, "dj-ui: the Files section did not come back\n");
        return 1;
    }

    // The rail is a set of places, and pressing one moves the list. The root comes from the layout
    // settings, the way the running shell takes it, and the press is a real event on the row.
    layout.setCollectionRoots({collectionRoot});
    // The file is written when the shell exits, so the check writes it the way that hook does
    // before reading it back with a fresh reader.
    if (!layout.save()) {
        std::fprintf(stderr, "dj-ui: the layout file could not be written\n");
        return 1;
    }
    QQuickItem *collectionRow = findItem(panel, QStringLiteral("section_collection"));
    QQuickItem *filesRow = findItem(panel, QStringLiteral("section_files"));
    if (collectionRow == nullptr || filesRow == nullptr) {
        std::fprintf(stderr, "dj-ui: the sections rail has no Collection row\n");
        return 1;
    }
    const auto pressSection = [&](QQuickItem *item) {
        const QPointF centre =
            item->mapToScene(QPointF(item->width() / 2.0, item->height() / 2.0));
        for (const QEvent::Type type : {QEvent::MouseButtonPress, QEvent::MouseButtonRelease}) {
            QMouseEvent event(type, centre, window->mapToGlobal(centre.toPoint()),
                              Qt::LeftButton, Qt::LeftButton, Qt::NoModifier);
            QCoreApplication::sendEvent(window, &event);
        }
        QCoreApplication::processEvents();
    };
    // The root is a setting, so it outlives the run: a fresh reader of the same file sees it.
    LayoutSettings reread;
    reread.setFile(layout.file());
    if (!reread.load() || reread.collectionRoots() != QStringList{collectionRoot}) {
        std::fprintf(stderr, "dj-ui: the collection roots were not remembered (%s)\n",
                     qPrintable(reread.collectionRoots().join(QStringLiteral(", "))));
        return 1;
    }
    pressSection(collectionRow);
    const QString afterCollection = browser.mode();
    const int pressedCount = static_cast<int>(browser.entries().size());
    pressSection(filesRow);
    const QString afterFiles = browser.mode();
    std::fprintf(stdout, "dj-ui: sections -> %s (%d tracks), then %s\n",
                 qPrintable(afterCollection), pressedCount, qPrintable(afterFiles));
    // Two tracks from the index when there is an engine, none when there is not.
    const int wantedPressed = hasEngine ? 2 : 0;
    if (afterCollection != QStringLiteral("collection") || pressedCount != wantedPressed
        || afterFiles != QStringLiteral("files")) {
        std::fprintf(stderr, "dj-ui: pressing a section did not move the list\n");
        return 1;
    }
    // Real material, when the run names it: how long a root takes to list and how much of it
    // carries a tempo. A measurement, not a gate — the tree above is what the check asserts.
    const QString realRoot = qEnvironmentVariable("DJ_COLLECTION_CHECK_ROOT");
    if (!realRoot.isEmpty()) {
        QElapsedTimer timer;
        timer.start();
        browser.openCollection({realRoot});
        const qint64 elapsed = timer.elapsed();
        int withTempo = 0;
        for (const QVariant &entry : browser.entries()) {
            if (entry.toMap().contains(QStringLiteral("bpm"))) {
                ++withTempo;
            }
        }
        std::fprintf(stdout, "dj-ui: collection %s -> %d tracks in %lld ms, %d with a tempo\n",
                     qPrintable(realRoot), static_cast<int>(browser.entries().size()),
                     static_cast<long long>(elapsed), withTempo);
    }

    std::fprintf(stdout,
                 "dj-ui: the Collection section lists %d tracks by name, carries the tempo and "
                 "the key from the metadata, filters by name and loads into a deck\n",
                 unfiltered);

    std::fprintf(stdout,
                 "dj-ui: browser selects and acts, selection follows the click, \"..\" goes up, "
                 "the text size moves the row, the sections (%d px) and the header — title, path, "
                 "the text-size buttons and the deck buttons — with it; the rail is %.0f px wide, "
                 "the browser keeps %d%% of the window when the type grows, and the rail width, the "
                 "font and the deck zoom come back after a restart (130 px, 14 px, 0.25)\n",
                 small, railAfter, browserPercent);
    // The deck's modes are settings as well, and a restart has to bring them back. This run leaves snap
    // and quantize off through the switches themselves — the write-through the hand uses is the one under
    // test — and the scrub mode audible, and the restore check reads all three in a new process.
    for (const char *name : {"snapGlobal", "quantizeGlobal"}) {
        auto *sw = findItem(window->contentItem(), QString::fromLatin1(name));
        if (sw == nullptr) {
            std::fprintf(stderr, "dj-ui: the mixer has no %s switch to leave a state in\n", name);
            return 1;
        }
        const QPointF centre = sw->mapToScene(QPointF(sw->width() / 2.0, sw->height() / 2.0));
        for (const QEvent::Type type : {QEvent::MouseMove, QEvent::MouseButtonPress,
                                        QEvent::MouseButtonRelease}) {
            const Qt::MouseButtons buttons = type == QEvent::MouseButtonPress ? Qt::LeftButton
                                                                             : Qt::NoButton;
            const Qt::MouseButton button = type == QEvent::MouseMove ? Qt::NoButton : Qt::LeftButton;
            QMouseEvent event(type, centre, window->mapToGlobal(centre.toPoint()), button, buttons,
                              Qt::NoModifier);
            QCoreApplication::sendEvent(window, &event);
            QCoreApplication::processEvents();
            QThread::msleep(40);
        }
    }
    settings.setScrubAudible(true);
    layout.setScrubAudible(true);
    // The file is written once the hand stops (a debounced save): leave it time to land.
    QElapsedTimer flushed;
    flushed.start();
    while (flushed.elapsed() < 1200) {
        QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
        QThread::msleep(20);
    }
    std::fprintf(stdout,
                 "dj-ui: this run leaves snap off, quantize off and the scrub audible for the next one\n");
    return 0;
}

// syncCheckOption: one check of the shell, out of main.
//
// Split out of main: a function of ten thousand lines loses its debug information, and GCC
// says so (`variable tracking size limit exceeded`). The checks are the tests; a named
// function is where a failure is read.

int runSortCheck(QQmlApplicationEngine &engine, FileBrowser &browser, LayoutSettings &layout,
                        const QString &fixture, const QString &collectionRoot)
{
    if (fixture.isEmpty() || collectionRoot.isEmpty()) {
        std::fprintf(stderr, "dj-ui: --sort-check needs --import-fixture and --collection-root\n");
        return 1;
    }
    QDir root(collectionRoot);
    if (!root.mkpath(QStringLiteral("."))) {
        std::fprintf(stderr, "dj-ui: cannot make %s\n", qPrintable(collectionRoot));
        return 1;
    }
    // Four names that parse as artist and title, so the list has something to sort by without tags.
    // The keys are what a secondary sort by tone has to see: `2d` before `10d`, and `8d` before `8m`.
    // The tempo is written with the precision the measurement has — `142.004` against `142.0` — and
    // the tone has to decide between them: that is the case the DJ hit with BPM first and Key second.
    const QList<QPair<QString, QString>> toneKeys = {
        {QStringLiteral("Alpha - Zebra"), QStringLiteral("2d")},
        {QStringLiteral("Beta - Apple"), QStringLiteral("8m")},
        {QStringLiteral("Alpha - Apple"), QStringLiteral("10d")},
        {QStringLiteral("Gamma - Mango"), QStringLiteral("8d")},
    };
    const QList<QPair<QString, double>> tempos = {
        {QStringLiteral("Alpha - Zebra"), 142.0},
        {QStringLiteral("Beta - Apple"), 143.0},
        {QStringLiteral("Alpha - Apple"), 142.004},
        {QStringLiteral("Gamma - Mango"), 141.5},
    };
    if (!root.mkpath(QStringLiteral(".djmeta"))) {
        std::fprintf(stderr, "dj-ui: cannot make the metadata folder under %s\n",
                     qPrintable(collectionRoot));
        return 1;
    }
    int index = 0;
    for (const auto &pair : toneKeys) {
        ++index;
        const QString target = root.filePath(pair.first + QStringLiteral(".wav"));
        QFile::remove(target);
        if (!QFile::copy(fixture, target)) {
            std::fprintf(stderr, "dj-ui: cannot place %s\n", qPrintable(target));
            return 1;
        }
        QFile record(root.filePath(QStringLiteral(".djmeta/") + pair.first
                                   + QStringLiteral(".wav.json")));
        if (!record.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
            std::fprintf(stderr, "dj-ui: cannot write the record for %s\n", qPrintable(pair.first));
            return 1;
        }
        double bpm = 0.0;
        for (const auto &tempo : tempos) {
            if (tempo.first == pair.first) {
                bpm = tempo.second;
            }
        }
        // A record the index can read: the schema, the revision, when it was written and what identifies
        // the file. The fingerprint is the row's key, so four tracks sharing one file's bytes would be
        // one row — the size stands in for the bytes this check did not measure.
        const QJsonObject fingerprint{
            {QStringLiteral("size"), 1000 + index},
            {QStringLiteral("head"), QString(16, QLatin1Char(char('a' + index)))},
            {QStringLiteral("tail"), QString(16, QLatin1Char(char('p' + index)))},
        };
        const QJsonObject grid{{QStringLiteral("bpm"), bpm},
                               {QStringLiteral("first_beat_frame"), 0},
                               {QStringLiteral("anchors"), QJsonArray{}},
                               {QStringLiteral("beats_per_bar"), 4}};
        // The names the columns show are the tags the import read, and the sort reads the same tags: a
        // record without them is four rows the artist column cannot order, whatever the header asks for.
        const QStringList parts = pair.first.split(QStringLiteral(" - "));
        const QJsonObject tags{{QStringLiteral("artist"), parts.value(0)},
                               {QStringLiteral("album"), QStringLiteral("Album")},
                               {QStringLiteral("title"), parts.value(1)},
                               {QStringLiteral("genres"), QJsonArray{}}};
        const QJsonObject meta{{QStringLiteral("schema"), QStringLiteral("djp-track-v1")},
                               {QStringLiteral("tags"), tags},
                               {QStringLiteral("revision"), 1},
                               {QStringLiteral("updated_at_ms"), 1},
                               {QStringLiteral("fingerprint"), fingerprint},
                               {QStringLiteral("key"), pair.second},
                               {QStringLiteral("beatgrid"), grid}};
        record.write(QJsonDocument(meta).toJson());
    }

    auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().value(0));
    if (window == nullptr) {
        std::fprintf(stderr, "dj-ui: no shell window\n");
        return 1;
    }
    auto *panel = findItem(window->contentItem(), QStringLiteral("browserPanel"));
    if (panel == nullptr) {
        std::fprintf(stderr, "dj-ui: the shell has no browser\n");
        return 1;
    }
    // The header's geometry comes from the column settings: a check that runs before the shell has
    // read them would click on nothing.
    if (layout.collectionColumns().isEmpty()) {
        layout.resetCollectionColumns();
    }
    browser.openCollection({collectionRoot});
    QElapsedTimer clock;
    clock.start();
    while (browser.entries().size() < 4 && clock.elapsed() < 5000) {
        QCoreApplication::processEvents();
        QThread::msleep(20);
    }
    if (browser.entries().size() != 4) {
        std::fprintf(stderr, "dj-ui: the collection lists %d rows, wanted 4\n",
                     static_cast<int>(browser.entries().size()));
        return 1;
    }

    // The rail: two places, and the one a DJ starts in is above the one he digs in.
    auto *collectionRow = findItem(window->contentItem(), QStringLiteral("section_collection"));
    auto *filesRow = findItem(window->contentItem(), QStringLiteral("section_files"));
    if (collectionRow == nullptr || filesRow == nullptr) {
        std::fprintf(stderr, "dj-ui: the rail has no Collection or Files row\n");
        return 1;
    }
    if (!(collectionRow->y() < filesRow->y())) {
        std::fprintf(stderr, "dj-ui: the rail puts Collection at %.1f and Files at %.1f\n",
                     collectionRow->y(), filesRow->y());
        return 1;
    }
    std::fprintf(stderr, "dj-ui: rail: Collection at %.1f, Files at %.1f\n", collectionRow->y(),
                 filesRow->y());

    auto *header = findItem(window->contentItem(), QStringLiteral("collectionHeaderDrag"));
    if (header == nullptr) {
        std::fprintf(stderr, "dj-ui: the collection has no header\n");
        return 1;
    }
    // The header has no width until the shell has laid itself out, and a click at zero lands on the
    // first column's edge, which is a resize: wait for the geometry before aiming at a name.
    {
        QElapsedTimer ready;
        ready.start();
        while (header->width() <= 1.0 && ready.elapsed() < 3000) {
            QCoreApplication::processEvents();
            QThread::msleep(20);
        }
    }
    // A column's middle, in the header's own coordinates: the click is a real mouse event, because a
    // sort nobody can reach with the pointer is a sort that only exists in the code.
    const auto clickColumn = [&](const QString &column, Qt::KeyboardModifiers modifiers) {
        const QVariantList columns = layout.collectionColumns();
        int index = -1;
        for (int at = 0; at < columns.size(); ++at) {
            if (columns.at(at).toMap().value(QStringLiteral("id")).toString() == column) {
                index = at;
                break;
            }
        }
        if (index < 0) {
            return false;
        }
        const double width = header->width();
        // The columns start after the row number and the lock that sits beside it, and that gutter is
        // part of the header: a left edge summed from the shares alone lands tens of pixels to the left
        // and clicks the gutter, which sorts nothing. The panel is asked for the edge it draws at.
        QVariant asked;
        const bool knows = QMetaObject::invokeMethod(panel, "columnX", Q_RETURN_ARG(QVariant, asked),
                                                     Q_ARG(QVariant, index), Q_ARG(QVariant, width));
        const double left = knows ? asked.toDouble() : 0.0;
        // The column's own middle: its share times the width the columns actually share, which the panel
        // answers with. Multiplying the share by the whole header leans right by the gutter, and a check
        // that leans can miss the column it aims at.
        QVariant askedSpan;
        const bool knowsSpan = QMetaObject::invokeMethod(panel, "columnWidth",
                                                         Q_RETURN_ARG(QVariant, askedSpan),
                                                         Q_ARG(QVariant, index), Q_ARG(QVariant, width));
        const double share = columns.at(index).toMap().value(QStringLiteral("width")).toDouble();
        const double span = knowsSpan ? askedSpan.toDouble() : share * width;
        const QPointF at(left + span / 2.0, header->height() / 2.0);
        const QPointF scene = header->mapToScene(at);
        for (const QEvent::Type type : {QEvent::MouseButtonPress, QEvent::MouseButtonRelease}) {
            QMouseEvent event(type, scene, window->mapToGlobal(scene.toPoint()), Qt::LeftButton,
                              Qt::LeftButton, modifiers);
            QCoreApplication::sendEvent(window, &event);
        }
        // A sort is asked of the index as well as applied here, and the answer carries the order that was
        // asked for. Reading the list in the same breath reads the answer to the previous question, so the
        // check waits the way a hand would: the row order settles before the next look.
        QElapsedTimer after;
        after.start();
        while (after.elapsed() < 400) {
            QCoreApplication::processEvents();
            QThread::msleep(10);
        }
        return true;
    };
    const auto order = [&]() {
        QStringList names;
        for (const QVariant &row : browser.entries()) {
            names.append(row.toMap().value(QStringLiteral("name")).toString());
        }
        return names;
    };
    const auto keys = [&]() {
        QStringList list;
        for (const QVariant &key : browser.collectionSort()) {
            const QVariantMap entry = key.toMap();
            list.append(entry.value(QStringLiteral("column")).toString()
                        + (entry.value(QStringLiteral("ascending")).toBool() ? "+" : "-"));
        }
        return list.join(QStringLiteral(","));
    };
    const auto require = [&](const QString &what, const QStringList &wanted) {
        const QStringList got = order();
        if (got != wanted) {
            // The keys say what was asked for; the rows say what the list did about it. A key that
            // turned and rows that did not are two different faults, and the message must tell them apart.
            std::fprintf(stderr, "dj-ui: %s -> %s, wanted %s (keys %s)\n", qPrintable(what),
                         qPrintable(got.join(QStringLiteral(" | "))),
                         qPrintable(wanted.join(QStringLiteral(" | "))), qPrintable(keys()));
            return false;
        }
        std::fprintf(stderr, "dj-ui: %s -> %s\n", qPrintable(what),
                     qPrintable(got.join(QStringLiteral(" | "))));
        return true;
    };

    // The order the list carries before any click: the order its own source gives it. Every line below
    // turns a sort on, so this is the only place it can be read — and it is what a click on the number
    // column has to give back.
    const QStringList sourceOrder = order();
    if (!clickColumn(QStringLiteral("artist"), Qt::NoModifier)) {
        std::fprintf(stderr, "dj-ui: the header has no artist column\n");
        return 1;
    }
    if (!require(QStringLiteral("click artist"),
                 {QStringLiteral("Alpha - Apple"), QStringLiteral("Alpha - Zebra"),
                  QStringLiteral("Beta - Apple"), QStringLiteral("Gamma - Mango")})) {
        return 1;
    }
    if (keys() != QStringLiteral("artist+")) {
        std::fprintf(stderr, "dj-ui: the sort keys read «%s», wanted artist+\n", qPrintable(keys()));
        return 1;
    }

    clickColumn(QStringLiteral("artist"), Qt::NoModifier);
    // Equal artists keep the name order: a list that reshuffles between equal rows jumps under the hand.
    if (!require(QStringLiteral("click artist again"),
                 {QStringLiteral("Gamma - Mango"), QStringLiteral("Beta - Apple"),
                  QStringLiteral("Alpha - Apple"), QStringLiteral("Alpha - Zebra")})) {
        return 1;
    }
    if (keys() != QStringLiteral("artist-")) {
        std::fprintf(stderr, "dj-ui: the second click left the keys at «%s»\n", qPrintable(keys()));
        return 1;
    }

    clickColumn(QStringLiteral("title"), Qt::NoModifier);
    if (!require(QStringLiteral("click title"),
                 {QStringLiteral("Alpha - Apple"), QStringLiteral("Beta - Apple"),
                  QStringLiteral("Gamma - Mango"), QStringLiteral("Alpha - Zebra")})) {
        return 1;
    }
    if (keys() != QStringLiteral("title+")) {
        std::fprintf(stderr, "dj-ui: a plain click did not replace the key: «%s»\n", qPrintable(keys()));
        return 1;
    }

    clickColumn(QStringLiteral("title"), Qt::NoModifier);
    if (!require(QStringLiteral("click title again"),
                 {QStringLiteral("Alpha - Zebra"), QStringLiteral("Gamma - Mango"),
                  QStringLiteral("Alpha - Apple"), QStringLiteral("Beta - Apple")})) {
        return 1;
    }

    // Shift: the title keeps deciding, the artist decides between the rows it could not. Ascending it
    // agrees with the name order, so the change is checked by turning that second key around.
    clickColumn(QStringLiteral("artist"), Qt::ShiftModifier);
    if (keys() != QStringLiteral("title-,artist+")) {
        std::fprintf(stderr, "dj-ui: the shift click left the keys at «%s»\n", qPrintable(keys()));
        return 1;
    }
    if (!require(QStringLiteral("shift click artist"),
                 {QStringLiteral("Alpha - Zebra"), QStringLiteral("Gamma - Mango"),
                  QStringLiteral("Alpha - Apple"), QStringLiteral("Beta - Apple")})) {
        return 1;
    }
    clickColumn(QStringLiteral("artist"), Qt::ShiftModifier);
    if (!require(QStringLiteral("shift click artist again"),
                 {QStringLiteral("Alpha - Zebra"), QStringLiteral("Gamma - Mango"),
                  QStringLiteral("Beta - Apple"), QStringLiteral("Alpha - Apple")})) {
        return 1;
    }
    if (keys() != QStringLiteral("title-,artist-")) {
        std::fprintf(stderr, "dj-ui: the second shift click left the keys at «%s»\n",
                     qPrintable(keys()));
        return 1;
    }
    // The tone: numbers on the wheel, not letters. A text sort would put 10d before 2d.
    clickColumn(QStringLiteral("key"), Qt::NoModifier);
    if (!require(QStringLiteral("click key"),
                 {QStringLiteral("Alpha - Zebra"), QStringLiteral("Gamma - Mango"),
                  QStringLiteral("Beta - Apple"), QStringLiteral("Alpha - Apple")})) {
        return 1;
    }
    if (keys() != QStringLiteral("key+")) {
        std::fprintf(stderr, "dj-ui: the key click left the keys at «%s»\n", qPrintable(keys()));
        return 1;
    }

    // And as the second key: the artist decides the groups, the tone decides inside one.
    clickColumn(QStringLiteral("artist"), Qt::NoModifier);
    clickColumn(QStringLiteral("key"), Qt::ShiftModifier);
    if (!require(QStringLiteral("artist then shift key"),
                 {QStringLiteral("Alpha - Zebra"), QStringLiteral("Alpha - Apple"),
                  QStringLiteral("Beta - Apple"), QStringLiteral("Gamma - Mango")})) {
        return 1;
    }
    if (keys() != QStringLiteral("artist+,key+")) {
        std::fprintf(stderr, "dj-ui: the shift click on the key left «%s»\n", qPrintable(keys()));
        return 1;
    }
    // The tempo first, the tone second: `142.0` and `142.004` are one tempo on the display, and the
    // tone decides between them. `141.5` and `143.0` stay where the tempo puts them.
    clickColumn(QStringLiteral("bpm"), Qt::NoModifier);
    if (!require(QStringLiteral("click bpm"),
                 {QStringLiteral("Gamma - Mango"), QStringLiteral("Alpha - Apple"),
                  QStringLiteral("Alpha - Zebra"), QStringLiteral("Beta - Apple")})) {
        return 1;
    }
    clickColumn(QStringLiteral("key"), Qt::ShiftModifier);
    if (!require(QStringLiteral("bpm then shift key"),
                 {QStringLiteral("Gamma - Mango"), QStringLiteral("Alpha - Zebra"),
                  QStringLiteral("Alpha - Apple"), QStringLiteral("Beta - Apple")})) {
        return 1;
    }
    if (keys() != QStringLiteral("bpm+,key+")) {
        std::fprintf(stderr, "dj-ui: the shift click on the key left «%s»\n", qPrintable(keys()));
        return 1;
    }
    // The tempo reads to a hundredth, and a whole number is padded: a column of rounded tempi cannot be
    // sorted by eye, and 128 must not read as something between 127 and 129.
    // The list is re-sorted by the last click, and a delegate is laid out a frame later: the cell is read
    // after the rows have settled, the way the eye reads it.
    for (int tick = 0; tick < 25; ++tick) {
        QCoreApplication::processEvents();
        QThread::msleep(20);
    }
    if (QQuickItem *cell = findItem(window->contentItem(), QStringLiteral("collectionCell_bpm_0"))) {
        const QString shown = cell->property("text").toString();
        const QRegularExpression wanted(QStringLiteral("^\\d+\\.\\d\\d$"));
        if (!wanted.match(shown).hasMatch()) {
            std::fprintf(stderr, "dj-ui: the BPM cell reads \"%s\", not a hundredth\n",
                         qPrintable(shown));
            return 1;
        }
        std::fprintf(stderr, "dj-ui: the BPM cell reads \"%s\"\n", qPrintable(shown));
    } else {
        std::fprintf(stderr, "dj-ui: no BPM cell to read\n");
        return 1;
    }

    // The number column's header: a click there drops every key at once. Two keys are in force here (the
    // tempo and the tone), so the case says "all of them" and not "the last one" — and the order has to
    // come back to the one the list had before the first click, not merely to something unsorted.
    {
        const double numberWidth = panel->property("numberWidth").toDouble();
        if (numberWidth <= 1.0) {
            std::fprintf(stderr, "dj-ui: the number column is %.1f px wide, nothing to click\n",
                         numberWidth);
            return 1;
        }
        const QPointF at(numberWidth / 2.0, header->height() / 2.0);
        const QPointF scene = header->mapToScene(at);
        for (const QEvent::Type type : {QEvent::MouseButtonPress, QEvent::MouseButtonRelease}) {
            QMouseEvent event(type, scene, window->mapToGlobal(scene.toPoint()), Qt::LeftButton,
                              Qt::LeftButton, Qt::NoModifier);
            QCoreApplication::sendEvent(window, &event);
        }
        // The index is asked again with no order at all, and its answer carries the rows: give it the
        // frames a hand would take before reading the list.
        QElapsedTimer asked;
        asked.start();
        while (asked.elapsed() < 500) {
            QCoreApplication::processEvents();
            QThread::msleep(10);
        }
        if (!keys().isEmpty()) {
            std::fprintf(stderr, "dj-ui: the click on the number column left the keys at «%s»\n",
                         qPrintable(keys()));
            return 1;
        }
        if (!require(QStringLiteral("click the number column"), sourceOrder)) {
            return 1;
        }
        // And the marks go with the keys: an arrow over a list that no column decides is the picture of a
        // sort that is not there.
        const QString marks = QStringLiteral("▲▼△▽");
        QStringList stillMarked;
        for (int index = 0; index < 6; ++index) {
            auto *cell = findItem(window->contentItem(),
                                  QStringLiteral("collectionHeaderCell_%1").arg(index));
            if (cell == nullptr) {
                continue;
            }
            const QString text = cell->property("text").toString();
            for (const QChar &glyph : text) {
                if (marks.contains(glyph)) {
                    stillMarked.append(text.trimmed());
                }
            }
        }
        if (!stillMarked.isEmpty()) {
            std::fprintf(stderr, "dj-ui: headers still carry a sort mark after the reset: %s\n",
                         qPrintable(stillMarked.join(QStringLiteral(" | "))));
            return 1;
        }
        std::fprintf(stderr, "dj-ui: the number column reset the sort, and no header keeps a mark\n");
    }

    // A list long enough to scroll: the cursor is driven by the keyboard, and the view must be at the
    // row at once — a window that glides to it reads as lag, not as motion.
    // A folder of its own: the index fills one it has never seen. Records that appear in a folder it
    // already knows are a different question — records arriving from outside — and they wait for a rescan.
    const QString scrollRoot = collectionRoot + QStringLiteral("-scroll");
    if (!QDir().mkpath(scrollRoot + QStringLiteral("/.djmeta"))) {
        std::fprintf(stderr, "dj-ui: cannot make %s\n", qPrintable(scrollRoot));
        return 1;
    }
    const QDir scroll(scrollRoot);
    for (int index = 0; index < 40; ++index) {
        const QString name = QStringLiteral("Scroll %1.wav").arg(index, 2, 10, QLatin1Char('0'));
        const QString target = scroll.filePath(name);
        if (!QFile::exists(target) && !QFile::copy(fixture, target)) {
            std::fprintf(stderr, "dj-ui: cannot place %s\n", qPrintable(target));
            return 1;
        }
        // A row is a record: forty files without one are forty files the list never shows.
        QFile record(scroll.filePath(QStringLiteral(".djmeta/") + name + QStringLiteral(".json")));
        if (!record.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
            std::fprintf(stderr, "dj-ui: cannot write the record for %s\n", qPrintable(name));
            return 1;
        }
        const QJsonObject fingerprint{
            {QStringLiteral("size"), 2000 + index},
            {QStringLiteral("head"), QString(16, QLatin1Char(char('A' + index % 26)))},
            {QStringLiteral("tail"), QString(16, QLatin1Char(char('Q' + index % 26)))},
        };
        const QJsonObject tags{{QStringLiteral("artist"), QStringLiteral("Scroll")},
                               {QStringLiteral("album"), QStringLiteral("Album")},
                               {QStringLiteral("title"), QString::number(index)},
                               {QStringLiteral("genres"), QJsonArray{}}};
        const QJsonObject meta{{QStringLiteral("schema"), QStringLiteral("djp-track-v1")},
                               {QStringLiteral("revision"), 1},
                               {QStringLiteral("updated_at_ms"), 1},
                               {QStringLiteral("fingerprint"), fingerprint},
                               {QStringLiteral("tags"), tags}};
        record.write(QJsonDocument(meta).toJson());
        record.close();
    }
    browser.openCollection({scrollRoot});
    clock.restart();
    while (browser.entries().size() < 40 && clock.elapsed() < 15000) {
        QCoreApplication::processEvents();
        QThread::msleep(20);
    }
    auto *list = findItem(window->contentItem(), QStringLiteral("browserList"));
    if (list == nullptr) {
        std::fprintf(stderr, "dj-ui: the browser has no list\n");
        return 1;
    }
    const double rowHeight = panel->property("rowHeight").toDouble();
    if (rowHeight <= 0.0) {
        std::fprintf(stderr, "dj-ui: the list has no row height\n");
        return 1;
    }
    // Probe: a cursor that arrives without a key — the remembered row of a section left behind, or a
    // row the DJ clicked. Where is the window then?
    const QVariantList rows = browser.entries();
    if (rows.size() < 31) {
        std::fprintf(stderr, "dj-ui: the collection lists %d rows, wanted at least 31 to scroll\n",
                     static_cast<int>(rows.size()));
        return 1;
    }
    const QString deep = rows.at(30).toMap().value(QStringLiteral("path")).toString();
    browser.setCursor(deep);
    for (int tick = 0; tick < 8; ++tick) {
        QCoreApplication::processEvents();
        QThread::msleep(20);
    }
    // A row that arrives without a key must land in the middle too: that is what the DJ sees when a
    // section opens at the remembered row, or when a click lands on a row the window had lost.
    {
        const int restoreRow = browser.rowOf(deep);
        const double windowCentre = list->property("contentY").toDouble() + list->height() / 2.0;
        const double rowCentre = restoreRow * rowHeight + rowHeight / 2.0;
        if (std::abs(windowCentre - rowCentre) > rowHeight) {
            std::fprintf(stderr,
                         "dj-ui: the restored row %d is not in the middle (row centre %.1f, "
                         "window centre %.1f)\n",
                         restoreRow, rowCentre, windowCentre);
            return 1;
        }
        std::fprintf(stderr, "dj-ui: the restored row %d is in the middle (contentY %.1f)\n",
                     restoreRow, list->property("contentY").toDouble());
    }

    if (browser.entries().isEmpty()) {
        std::fprintf(stderr, "dj-ui: the list is empty, so there is no first row to stand on\n");
        return 1;
    }
    browser.setCursor(browser.entries().first().toMap().value(QStringLiteral("path")).toString());
    QCoreApplication::processEvents();
    for (int step = 0; step < 30; ++step) {
        QKeyEvent press(QEvent::KeyPress, Qt::Key_Down, Qt::NoModifier);
        QCoreApplication::sendEvent(list, &press);
        QCoreApplication::processEvents();
    }
    const int row = browser.rowOf(browser.cursor());
    const double settled = list->property("contentY").toDouble();
    // An animation would still be running: give it the frames and require the window not to move.
    for (int tick = 0; tick < 8; ++tick) {
        QCoreApplication::processEvents();
        QThread::msleep(20);
    }
    const double later = list->property("contentY").toDouble();
    const double viewHeight = list->height();
    if (std::abs(later - settled) > 0.5) {
        std::fprintf(stderr, "dj-ui: the view was still gliding to the row (%.1f -> %.1f)\n",
                     settled, later);
        return 1;
    }
    // The row must sit in the middle of the window, not against its edge: a cursor under the last
    // visible row is exactly what the owner reported as wrong.
    const double windowCentre = settled + viewHeight / 2.0;
    const double rowCentre = row * rowHeight + rowHeight / 2.0;
    if (std::abs(windowCentre - rowCentre) > rowHeight) {
        std::fprintf(stderr,
                     "dj-ui: row %d is not in the middle (row centre %.1f, window centre %.1f)\n",
                     row, rowCentre, windowCentre);
        return 1;
    }
    std::fprintf(stderr, "dj-ui: the view is at row %d at once (contentY %.1f, row height %.1f)\n",
                 row, settled, rowHeight);

    // The wheel moves the cursor: a scroll that only moved the picture slid the selection out of the
    // window, which is what the DJ reported. The wheel event goes through the window, as a real one
    // does, so the handler on the list is the thing under test.
    {
        const int beforeWheel = browser.rowOf(browser.cursor());
        const QPointF local(list->width() / 2.0, list->height() / 2.0);
        const QPointF scene = list->mapToScene(local);
        QWheelEvent wheel(scene, window->mapToGlobal(scene.toPoint()), QPoint(0, 0), QPoint(0, -120),
                          Qt::NoButton, Qt::NoModifier, Qt::NoScrollPhase, false);
        QCoreApplication::sendEvent(window, &wheel);
        for (int tick = 0; tick < 4; ++tick) {
            QCoreApplication::processEvents();
            QThread::msleep(20);
        }
        const int afterWheel = browser.rowOf(browser.cursor());
        if (afterWheel != beforeWheel + 1) {
            std::fprintf(stderr, "dj-ui: the wheel moved the cursor from %d to %d, wanted one row\n",
                         beforeWheel, afterWheel);
            return 1;
        }
        const double wheelCentre = list->property("contentY").toDouble() + list->height() / 2.0;
        const double wheelRowCentre = afterWheel * rowHeight + rowHeight / 2.0;
        if (std::abs(wheelCentre - wheelRowCentre) > rowHeight) {
            std::fprintf(stderr,
                         "dj-ui: after the wheel row %d is not in the middle (row centre %.1f, "
                         "window centre %.1f)\n",
                         afterWheel, wheelRowCentre, wheelCentre);
            return 1;
        }
        std::fprintf(stderr, "dj-ui: the wheel moved the cursor to row %d, still in the middle\n",
                     afterWheel);
    }
    // The DJ's own flow: the cursor is remembered from the last session and the rows arrive after it.
    // A single positioning against a half-built list lands short — the row ends up below the window,
    // which is what he saw when he opened the collection.
    {
        root.mkpath(QStringLiteral("empty"));
        const QString remembered = scroll.filePath(QStringLiteral("Scroll 30.wav"));
        browser.openCollection({root.filePath(QStringLiteral("empty"))});
        QCoreApplication::processEvents();
        browser.setCursor(remembered);
        browser.openCollection({scrollRoot});
        clock.restart();
        while (browser.entries().size() < 40 && clock.elapsed() < 10000) {
            QCoreApplication::processEvents();
            QThread::msleep(20);
        }
        for (int tick = 0; tick < 12; ++tick) {
            QCoreApplication::processEvents();
            QThread::msleep(20);
        }
        // The rows, not only the opening: the index answers a page when it has one, and a page that came
        // back empty has no row to remember — the cursor then reads -1 and a working panel looks broken.
        for (int tick = 0; tick < 250 && browser.entries().isEmpty(); ++tick) {
            QCoreApplication::processEvents();
            QThread::msleep(20);
        }
        const int rememberedRow = browser.rowOf(browser.cursor());
        const double windowCentre = list->property("contentY").toDouble() + list->height() / 2.0;
        const double rowCentre = rememberedRow * rowHeight + rowHeight / 2.0;
        if (rememberedRow < 0 || std::abs(windowCentre - rowCentre) > rowHeight) {
            std::fprintf(stderr,
                         "dj-ui: the remembered row %d is not in the middle (row centre %.1f, "
                         "window centre %.1f)\n",
                         rememberedRow, rowCentre, windowCentre);
            return 1;
        }
        std::fprintf(stderr, "dj-ui: the remembered row %d is in the middle (contentY %.1f)\n",
                     rememberedRow, list->property("contentY").toDouble());
    }

    // Probe: a track out of the collection into a deck — the gesture the DJ says stopped working. The
    // existing check loads from the files section only, which is why this went unnoticed.
    {
        const QPointF point = list->mapToScene(QPointF(20.0, 5 * rowHeight + 9.0));
        for (const QEvent::Type type : {QEvent::MouseButtonPress, QEvent::MouseButtonRelease}) {
            QMouseEvent event(type, point, window->mapToGlobal(point.toPoint()), Qt::LeftButton,
                              Qt::LeftButton, Qt::NoModifier);
            QCoreApplication::sendEvent(window, &event);
        }
        QCoreApplication::processEvents();
        const int clicked = list->property("currentIndex").toInt();
        const QVariant selected = panel->property("selected");
        const QString path = selected.toMap().value(QStringLiteral("path")).toString();
        panel->setProperty("lastAction", QString());
        QVariant returned;
        QMetaObject::invokeMethod(panel, "loadInto", Q_RETURN_ARG(QVariant, returned),
                                  Q_ARG(QVariant, QStringLiteral("A")));
        QElapsedTimer wait;
        wait.start();
        while (wait.elapsed() < 60000
               && !panel->property("lastAction").toString().startsWith(QStringLiteral("loaded "))) {
            QCoreApplication::processEvents();
            QThread::msleep(50);
        }
        const QString action = panel->property("lastAction").toString();
        if (clicked < 0 || path.isEmpty() || !returned.toBool()
            || !action.startsWith(QStringLiteral("loaded "))) {
            std::fprintf(stderr,
                         "dj-ui: a collection row did not load into a deck (clicked %d, selected "
                         "'%s', action '%s')\n",
                         clicked, qPrintable(path), qPrintable(action));
            return 1;
        }
        std::fprintf(stderr, "dj-ui: a collection row loads into a deck: %s\n", qPrintable(action));
    }

    // Every name is its own door. This is the fault the owner met by hand: the columns sit after the row
    // number and the lock, and a mapping that forgets the gutter answers one column to the right — a click
    // on BPM asking the index for Key. The keys the panel asked for say which door the click opened, so the
    // test does not have to guess from the row order.
    for (const char *column : {"artist", "album", "title", "time", "bpm", "key"}) {
        const QString name = QString::fromLatin1(column);
        if (!clickColumn(name, Qt::NoModifier)) {
            std::fprintf(stderr, "dj-ui: the header has no %s column\n", column);
            return 1;
        }
        const QString asked = keys().split(QLatin1Char(',')).value(0);
        if (!asked.startsWith(name)) {
            std::fprintf(stderr, "dj-ui: a click on %s asked for %s\n", column, qPrintable(asked));
            return 1;
        }
    }
    std::fprintf(stderr, "dj-ui: every column's name sorts by itself (%s)\n", qPrintable(keys()));

    // The header's mapping, asked directly, and at two widths: the middle of every column must answer that
    // column. This is the arithmetic a click goes through — the columns start after the row number and the
    // lock, and share what is left — and the fault shows as one column off. A single width can hide it,
    // because the two ways of summing the same shares can agree there; a narrow window makes the gutter a
    // larger part of the header and the difference visible.
    const auto mappingAt = [&](int wide, const char *what) -> QString {
        window->resize(wide, 800);
        for (int tick = 0; tick < 15; ++tick) {
            QCoreApplication::processEvents();
            QThread::msleep(20);
        }
        const double width = header->width();
        for (int index = 0; index < layout.collectionColumns().size(); ++index) {
            QVariant askedLeft;
            QVariant askedSpan;
            QVariant middleAnswer;
            QVariant leftAnswer;
            QMetaObject::invokeMethod(panel, "columnX", Q_RETURN_ARG(QVariant, askedLeft),
                                      Q_ARG(QVariant, index), Q_ARG(QVariant, width));
            QMetaObject::invokeMethod(panel, "columnWidth", Q_RETURN_ARG(QVariant, askedSpan),
                                      Q_ARG(QVariant, index), Q_ARG(QVariant, width));
            const double left = askedLeft.toDouble();
            const double span = askedSpan.toDouble();
            QMetaObject::invokeMethod(panel, "columnIndexAt", Q_RETURN_ARG(QVariant, middleAnswer),
                                      Q_ARG(QVariant, left + span / 2.0), Q_ARG(QVariant, width));
            QMetaObject::invokeMethod(panel, "columnIndexAt", Q_RETURN_ARG(QVariant, leftAnswer),
                                      Q_ARG(QVariant, left + 2.0), Q_ARG(QVariant, width));
            if (middleAnswer.toInt() != index || leftAnswer.toInt() != index) {
                const QString id =
                    layout.collectionColumns().at(index).toMap().value(QStringLiteral("id")).toString();
                return QStringLiteral("at %1 px (header %2) the middle of %3 answers %4, its left edge %5")
                    .arg(wide)
                    .arg(width)
                    .arg(id)
                    .arg(middleAnswer.toInt())
                    .arg(leftAnswer.toInt());
            }
        }
        std::fprintf(stderr, "dj-ui: every column answers itself %s (%u px wide)\n", what,
                     static_cast<unsigned>(width));
        return QString();
    };
    for (const auto &pass : {std::make_pair(1200, "in the wide window"),
                             std::make_pair(720, "in a narrow window")}) {
        if (const QString complaint = mappingAt(pass.first, pass.second); !complaint.isEmpty()) {
            std::fprintf(stderr, "dj-ui: %s\n", qPrintable(complaint));
            return 1;
        }
    }
    window->resize(1200, 800);
    for (int tick = 0; tick < 15; ++tick) {
        QCoreApplication::processEvents();
        QThread::msleep(20);
    }

    std::fprintf(stderr, "dj-ui: sort check done\n");
    return 0;
}

// The collection's Analyze…: one track gets a progress window, a selection gets a question with the
// parallel box and then the same window. Afterwards the numbers are in the list, not only in the files.
/// How the collection lists: the walk, the sidecars, the rows — and the time it takes.
///
/// The section is built by walking every root and reading a `.djmeta` record per file, then sorted and
/// filtered in memory. Whether that holds at a hundred thousand tracks is a question with a number, not
/// an opinion, and this is where the number comes from. Nothing is written.

int runCollectionProbe(FileBrowser &browser, const QString &root)
{
    if (root.isEmpty()) {
        std::fprintf(stderr, "dj-ui: --collection-probe needs --collection-root\n");
        return 1;
    }
    QElapsedTimer clock;
    clock.start();
    browser.openCollection({root});
    const qint64 cold = clock.elapsed();
    const int rows = static_cast<int>(browser.entries().size());
    std::fprintf(stderr, "dj-ui: collection probe: %d rows in %lld ms (cold)\n", rows, cold);
    clock.restart();
    browser.openCollection({root});
    const qint64 warm = clock.elapsed();
    std::fprintf(stderr, "dj-ui: collection probe: %d rows in %lld ms (warm)\n", rows, warm);
    // The sort the DJ asks for by clicking a header, on the rows the walk produced.
    clock.restart();
    browser.sortCollection(QStringLiteral("artist"), false);
    std::fprintf(stderr, "dj-ui: collection probe: sort by artist in %lld ms\n", clock.elapsed());
    // The search field: what the DJ types, and what clearing it costs.
    clock.restart();
    browser.setFilter(QStringLiteral("Track 99999"));
    std::fprintf(stderr, "dj-ui: collection probe: filter -> %d rows in %lld ms\n",
                 static_cast<int>(browser.entries().size()), clock.elapsed());
    clock.restart();
    browser.setFilter(QString());
    std::fprintf(stderr, "dj-ui: collection probe: filter cleared in %lld ms\n", clock.elapsed());
    return 0;
}

/// Does this value, anywhere inside it, name the file?

bool holdsName(const QVariant &value, const QString &name)
{
    if (value.typeId() == QMetaType::QVariantMap) {
        const QVariantMap map = value.toMap();
        for (auto entry = map.cbegin(); entry != map.cend(); ++entry) {
            if (holdsName(entry.value(), name)) {
                return true;
            }
        }
        return false;
    }
    if (value.typeId() == QMetaType::QVariantList) {
        for (const QVariant &item : value.toList()) {
            if (holdsName(item, name)) {
                return true;
            }
        }
        return false;
    }
    return value.canConvert<QString>() && value.toString().contains(name);
}

/// A track out of a real collection into a deck, read only.
///
/// The other checks load from the files section and from a collection this program wrote itself. A
/// collection the DJ made is a different thing: hashed folders, sidecars, names taken from tags. And
/// the panel's own message is not evidence — the deck's track is, so the engine's snapshot is what
/// gets read here.

int runCollectionLoadCheck(QQmlApplicationEngine &engine, FileBrowser &browser,
                                  BridgeClient &bridge, const QString &collectionRoot)
{
    if (collectionRoot.isEmpty()) {
        std::fprintf(stderr, "dj-ui: --collection-load-check needs --collection-root\n");
        return 1;
    }
    auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().value(0));
    if (window == nullptr) {
        std::fprintf(stderr, "dj-ui: no shell window\n");
        return 1;
    }
    auto *panel = findItem(window->contentItem(), QStringLiteral("browserPanel"));
    if (panel == nullptr) {
        std::fprintf(stderr, "dj-ui: the shell has no browser\n");
        return 1;
    }
    browser.openCollection({collectionRoot});
    QElapsedTimer clock;
    clock.start();
    while (browser.entries().size() < 6 && clock.elapsed() < 60000) {
        QCoreApplication::processEvents();
        QThread::msleep(50);
    }
    // The row number: a position in the list, drawn to the left of the columns and reading as the DJ counts.
    if (auto *number = findItem(window->contentItem(), QStringLiteral("collectionCell_number_0"))) {
        const QString shown = number->property("text").toString();
        std::fprintf(stdout, "dj-ui: the first row is numbered '%s'\n", qPrintable(shown));
        if (shown != QStringLiteral("1")) {
            std::fprintf(stderr, "dj-ui: the row number reads '%s', wanted '1'\n", qPrintable(shown));
            return 1;
        }
    } else {
        std::fprintf(stderr, "dj-ui: the collection has no row numbers\n");
        return 1;
    }
    const int rows = static_cast<int>(browser.entries().size());
    if (rows < 6) {
        std::fprintf(stderr, "dj-ui: the collection lists %d rows\n", rows);
        return 1;
    }
    const QVariantMap entry = browser.entries().at(5).toMap();
    const QString path = entry.value(QStringLiteral("path")).toString();
    browser.setCursor(path);
    QCoreApplication::processEvents();
    const QString selectedPath =
        panel->property("selected").toMap().value(QStringLiteral("path")).toString();
    // The button, not the function: the DJ presses a deck button in the header, and a handler that is
    // never reached by a real press is a handler that does not exist.
    auto *button = findItem(window->contentItem(), QStringLiteral("deckButton_A"));
    if (button == nullptr) {
        std::fprintf(stderr, "dj-ui: the header has no deck A button\n");
        return 1;
    }
    panel->setProperty("lastAction", QString());
    const QPointF centre(button->width() / 2.0, button->height() / 2.0);
    const QPointF scene = button->mapToScene(centre);
    for (const QEvent::Type type : {QEvent::MouseButtonPress, QEvent::MouseButtonRelease}) {
        QMouseEvent event(type, scene, window->mapToGlobal(scene.toPoint()), Qt::LeftButton,
                          Qt::LeftButton, Qt::NoModifier);
        QCoreApplication::sendEvent(window, &event);
    }
    QCoreApplication::processEvents();
    const bool accepted = !panel->property("lastAction").toString().isEmpty();
    const QVariant returned = accepted;
    QElapsedTimer settle;
    settle.start();
    while (settle.elapsed() < 90000
           && !panel->property("lastAction").toString().startsWith(QStringLiteral("loaded "))) {
        QCoreApplication::processEvents();
        QThread::msleep(50);
    }
    const QString action = panel->property("lastAction").toString();
    std::fprintf(stderr,
                 "dj-ui: collection load: %d rows, row 5 '%s', selected '%s', loadInto %s, action "
                 "'%s'\n",
                 rows, qPrintable(path), qPrintable(selectedPath),
                 returned.toBool() ? "true" : "false", qPrintable(action));
    if (selectedPath.isEmpty() || !returned.toBool()) {
        return 1;
    }
    const QString name = QFileInfo(path).fileName();
    const auto loaded = [&]() { return holdsName(bridge.snapshot(), name); };
    clock.restart();
    while (!loaded() && clock.elapsed() < 90000) {
        QCoreApplication::processEvents();
        QThread::msleep(50);
    }
    if (!loaded()) {
        std::fprintf(stderr, "dj-ui: deck A does not carry '%s' after the load\n", qPrintable(name));
        return 1;
    }
    std::fprintf(stderr, "dj-ui: the collection track reached deck A: %s\n", qPrintable(name));
    return 0;
}

/// A metronome as a wav file: a click on every beat, so the detector has a beat to find.
///
/// The checks that need several tracks cannot copy one recording: the identity check reads copies as the
/// same track and sets them aside. Three tempos are three tracks, and each one can be measured.

int runCollectionPageCheck(QQmlApplicationEngine &engine, FileBrowser &browser,
                                  BridgeClient &bridge, const QString &fixture,
                                  const QString &collectionRoot)
{
    if (fixture.isEmpty() || !QFile::exists(fixture)) {
        std::fprintf(stderr, "dj-ui: --collection-page-check needs a real track in --import-fixture\n");
        return 1;
    }
    if (collectionRoot.isEmpty()) {
        std::fprintf(stderr, "dj-ui: --collection-page-check needs --collection-root\n");
        return 1;
    }
    QTemporaryDir incoming;
    if (!incoming.isValid()) {
        std::fprintf(stderr, "dj-ui: cannot make a temporary folder\n");
        return 1;
    }
    // The cold case first: no index exists yet, so the engine creates it and runs the schema and its
    // migrations inside the answer. That is what a browser opening the collection on a fresh install
    // does, and it must not hang the engine.
    {
        QVariantMap cold;
        cold.insert(QStringLiteral("id"), 1111);
        cold.insert(QStringLiteral("offset"), 0);
        cold.insert(QStringLiteral("limit"), 100);
        QElapsedTimer coldClock;
        coldClock.start();
        bridge.queryCollection(cold);
        while (coldClock.elapsed() < 15000
               && bridge.collectionPage().value(QStringLiteral("id")).toInt() != 1111) {
            QCoreApplication::processEvents();
            QThread::msleep(10);
        }
        if (bridge.collectionPage().value(QStringLiteral("id")).toInt() != 1111) {
            std::fprintf(stderr, "dj-ui: the engine never answered a query with no index\n");
            return 1;
        }
        std::fprintf(stdout, "dj-ui: a query with no index answered in %lld ms, %d rows\n",
                     static_cast<long long>(coldClock.elapsed()),
                     static_cast<int>(bridge.collectionPage().value(QStringLiteral("rows")).toList().size()));
    }
    const QString source = incoming.path() + QStringLiteral("/A - Page.wav");
    if (!QFile::copy(fixture, source)) {
        std::fprintf(stderr, "dj-ui: cannot copy %s\n", qPrintable(fixture));
        return 1;
    }

    auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
    if (window == nullptr) {
        std::fprintf(stderr, "dj-ui: the root object is not a window\n");
        return 1;
    }
    window->resize(1366, 768);
    window->show();
    browser.open(incoming.path());
    QEventLoop settle;
    QTimer::singleShot(600, &settle, &QEventLoop::quit);
    settle.exec();

    QQuickItem *panel = findItem(window->contentItem(), QStringLiteral("browserPanel"));
    if (panel == nullptr) {
        std::fprintf(stderr, "dj-ui: the shell has no browser\n");
        return 1;
    }
    int row = -1;
    const QVariantList listed = browser.entries();
    for (int index = 0; index < listed.size(); ++index) {
        if (listed.at(index).toMap().value(QStringLiteral("path")).toString() == source) {
            row = index;
        }
    }
    if (row < 0) {
        std::fprintf(stderr, "dj-ui: the browser lists %d rows and not the copy\n",
                     static_cast<int>(listed.size()));
        return 1;
    }

    QVariant returned;
    QMetaObject::invokeMethod(panel, "choose", Q_RETURN_ARG(QVariant, returned),
                              Q_ARG(QVariant, row),
                              Q_ARG(QVariant, static_cast<int>(Qt::NoModifier)));
    QMetaObject::invokeMethod(panel, "askImport", Q_RETURN_ARG(QVariant, returned));
    QCoreApplication::processEvents();
    auto *confirmButton = panel->findChild<QObject *>(QStringLiteral("importConfirm"));
    if (confirmButton == nullptr) {
        std::fprintf(stderr, "dj-ui: the import windows are not in the shell\n");
        return 1;
    }
    QMetaObject::invokeMethod(confirmButton, "clicked");
    const auto state = [&]() { return panel->property("importState").toMap(); };
    QElapsedTimer batch;
    batch.start();
    while (batch.elapsed() < 60000) {
        QCoreApplication::processEvents();
        const QVariantMap now = state();
        if (!now.isEmpty() && now.value(QStringLiteral("total")).toInt() > 0
            && !now.value(QStringLiteral("running")).toBool()) {
            break;
        }
        QThread::msleep(10);
    }
    const QVariantMap finished = state();
    if (finished.value(QStringLiteral("placed")).toInt() < 1) {
        std::fprintf(stderr, "dj-ui: the batch placed %d of %d\n",
                     finished.value(QStringLiteral("placed")).toInt(),
                     finished.value(QStringLiteral("total")).toInt());
        return 1;
    }
    std::fprintf(stdout, "dj-ui: the collection holds the copy after an import (%d placed)\n",
                 finished.value(QStringLiteral("placed")).toInt());

    // The walk: what the list shows today.
    browser.openCollection({collectionRoot});
    QElapsedTimer walkClock;
    walkClock.start();
    while (browser.entries().isEmpty() && walkClock.elapsed() < 5000) {
        QCoreApplication::processEvents();
        QThread::msleep(20);
    }
    const QVariantList walked = browser.entries();

    // The page: what the index says, asked directly — the list is not involved in the answer.
    QVariantMap query;
    query.insert(QStringLiteral("id"), 4242);
    query.insert(QStringLiteral("offset"), 0);
    query.insert(QStringLiteral("limit"), 5000);
    bridge.queryCollection(query);
    QElapsedTimer pageClock;
    pageClock.start();
    while (pageClock.elapsed() < 15000
           && bridge.collectionPage().value(QStringLiteral("id")).toInt() != 4242) {
        QCoreApplication::processEvents();
        QThread::msleep(20);
    }
    const QVariantMap answer = bridge.collectionPage();
    const QVariantList pageRows = answer.value(QStringLiteral("rows")).toList();
    if (answer.value(QStringLiteral("id")).toInt() != 4242) {
        std::fprintf(stderr, "dj-ui: the engine never answered the page question\n");
        return 1;
    }
    std::fprintf(stdout, "dj-ui: the walk lists %d rows, the index page %d\n",
                 static_cast<int>(walked.size()), static_cast<int>(pageRows.size()));

    QHash<QString, QVariantMap> byPath;
    for (const QVariant &entry : pageRows) {
        const QVariantMap made = entry.toMap();
        byPath.insert(made.value(QStringLiteral("path")).toString(), made);
    }
    int told = 0;
    int compared = 0;
    int wrong = 0;
    for (const QVariant &entry : walked) {
        const QVariantMap row_ = entry.toMap();
        const QString path = row_.value(QStringLiteral("path")).toString();
        if (!byPath.contains(path)) {
            if (told++ < 5) {
                std::fprintf(stderr, "dj-ui: the index has no row for %s\n", qPrintable(path));
            }
            ++wrong;
            continue;
        }
        const QVariantMap pageRow = byPath.take(path);
        ++compared;
        const auto complain = [&](const char *field, const QString &walkedValue,
                                  const QString &pageValue) {
            if (told++ < 5) {
                std::fprintf(stderr, "dj-ui: %s differs on %s: walk '%s', index '%s'\n", field,
                             qPrintable(path), qPrintable(walkedValue), qPrintable(pageValue));
            }
            ++wrong;
        };
        if (row_.value(QStringLiteral("artist")).toString()
            != pageRow.value(QStringLiteral("artist")).toString()) {
            complain("artist", row_.value(QStringLiteral("artist")).toString(),
                     pageRow.value(QStringLiteral("artist")).toString());
        }
        if (row_.value(QStringLiteral("album")).toString()
            != pageRow.value(QStringLiteral("album")).toString()) {
            complain("album", row_.value(QStringLiteral("album")).toString(),
                     pageRow.value(QStringLiteral("album")).toString());
        }
        if (row_.value(QStringLiteral("title")).toString()
            != pageRow.value(QStringLiteral("title")).toString()) {
            complain("title", row_.value(QStringLiteral("title")).toString(),
                     pageRow.value(QStringLiteral("title")).toString());
        }
        // The walk carries milliseconds, the index frames and their rate: one number, two spellings.
        const double frames = pageRow.value(QStringLiteral("duration_frames")).toDouble();
        const double rate = pageRow.value(QStringLiteral("sample_rate")).toDouble();
        const qint64 pageMs = rate > 0.0 && frames > 0.0
            ? static_cast<qint64>(frames * 1000.0 / rate + 0.5)
            : 0;
        const qint64 walkedMs = row_.value(QStringLiteral("duration_ms")).toLongLong();
        if (std::abs(pageMs - walkedMs) > 1) {
            complain("duration", QString::number(walkedMs), QString::number(pageMs));
        }
        // Genres: the walk keeps them apart, the index keeps one line per name.
        QStringList pageGenres;
        const QString names = pageRow.value(QStringLiteral("genre")).toString();
        for (const QString &genre : names.split(QStringLiteral(", "), Qt::SkipEmptyParts)) {
            pageGenres.append(genre.trimmed());
        }
        const QStringList walkedGenres = row_.value(QStringLiteral("genres")).toStringList();
        if (pageGenres != walkedGenres) {
            complain("genres", walkedGenres.join(QStringLiteral("|")),
                     pageGenres.join(QStringLiteral("|")));
        }
    }
    if (!byPath.isEmpty()) {
        const QString extra = byPath.keys().value(0);
        if (told < 5) {
            std::fprintf(stderr, "dj-ui: the index has a row the walk does not: %s\n",
                         qPrintable(extra));
        }
        ++wrong;
    }
    std::fprintf(stdout, "dj-ui: %d rows compared field by field, %d differ\n", compared, wrong);
    if (wrong != 0) {
        return 1;
    }
    std::fprintf(stdout, "dj-ui: the index and the walk agree on every row\n");

    // The genre boxes: the engine counts them in its own table and publishes them apart from the page. The
    // walk knows the same genres from the same files, so the two counts must agree — that is what makes
    // "the boxes come from the database" a measurement instead of a claim.
    {
        QMap<QString, int> walkedGenres;
        for (const QVariant &entry : walked) {
            for (const QString &genre : entry.toMap().value(QStringLiteral("genres")).toStringList()) {
                walkedGenres[genre.toLower()] += 1;
            }
        }
        QMap<QString, int> counted;
        QElapsedTimer waitForGenres;
        waitForGenres.start();
        while (waitForGenres.elapsed() < 5000) {
            QCoreApplication::processEvents();
            const QVariantList said = bridge.snapshot().value(QStringLiteral("genres")).toList();
            if (!said.isEmpty()) {
                counted.clear();
                for (const QVariant &entry : said) {
                    const QVariantMap box = entry.toMap();
                    counted.insert(box.value(QStringLiteral("name")).toString().toLower(),
                                   box.value(QStringLiteral("count")).toInt());
                }
                break;
            }
            QThread::msleep(20);
        }
        if (counted.isEmpty()) {
            std::fprintf(stderr, "dj-ui: the index said nothing about genres\n");
            return 1;
        }
        if (counted != walkedGenres) {
            std::fprintf(stderr, "dj-ui: the genre table disagrees with the walk\n");
            for (auto it = counted.cbegin(); it != counted.cend(); ++it) {
                std::fprintf(stderr, "dj-ui:   table %s %d, walk %d\n", qPrintable(it.key()),
                             it.value(), walkedGenres.value(it.key(), -1));
            }
            for (auto it = walkedGenres.cbegin(); it != walkedGenres.cend(); ++it) {
                if (!counted.contains(it.key())) {
                    std::fprintf(stderr, "dj-ui:   only in the walk: %s %d\n", qPrintable(it.key()),
                                 it.value());
                }
            }
            return 1;
        }
        std::fprintf(stdout, "dj-ui: the genre boxes come from the table: %d genres, the walk agrees\n",
                     static_cast<int>(counted.size()));
    }

    // A genre filter must narrow the list, not empty it: the answer is already narrowed by the index and
    // must be shown as it came. This is the bug the DJ reported — a saved genre left an empty collection.
    if (!pageRows.isEmpty()) {
        const QString names = pageRows.first().toMap().value(QStringLiteral("genre")).toString();
        const QString wanted = names.split(QStringLiteral(", ")).value(0).trimmed();
        if (!wanted.isEmpty()) {
            browser.setGenreFilter({wanted});
            QElapsedTimer filtered;
            filtered.start();
            while (browser.entries().isEmpty() && filtered.elapsed() < 5000) {
                QCoreApplication::processEvents();
                QThread::msleep(20);
            }
            std::fprintf(stdout, "dj-ui: the genre filter '%s' leaves %d rows\n", qPrintable(wanted),
                         static_cast<int>(browser.entries().size()));
            if (browser.entries().isEmpty()) {
                std::fprintf(stderr, "dj-ui: a genre the index knows left an empty list\n");
                return 1;
            }
            browser.setGenreFilter({});
        }
    }

    // Can the engine still take a load after answering a collection query? That is what broke when the
    // page was wired into the list: the query opened the index — schema and migrations, a write — on the
    // same loop that acks deck commands. The number below is the answer, not an opinion.
    if (!walked.isEmpty()) {
        browser.setCursor(walked.first().toMap().value(QStringLiteral("path")).toString());
        QCoreApplication::processEvents();
        panel->setProperty("lastAction", QString());
        QVariant returnedLoad;
        QElapsedTimer loadClock;
        loadClock.start();
        QMetaObject::invokeMethod(panel, "loadInto", Q_RETURN_ARG(QVariant, returnedLoad),
                                  Q_ARG(QVariant, QStringLiteral("A")));
        while (loadClock.elapsed() < 20000
               && !panel->property("lastAction").toString().startsWith(QStringLiteral("loaded "))) {
            QCoreApplication::processEvents();
            QThread::msleep(20);
        }
        const QString action = panel->property("lastAction").toString();
        std::fprintf(stdout, "dj-ui: a load after the query -> '%s' in %lld ms\n", qPrintable(action),
                     static_cast<long long>(loadClock.elapsed()));
        if (!action.startsWith(QStringLiteral("loaded "))) {
            std::fprintf(stderr, "dj-ui: the engine did not take a load after the query\n");
            return 1;
        }
    }

    // The header the DJ reads the collection by: the count beside the title, the browser's own controls
    // at the far right. Measured by the same code the import check runs.
    if (const int verdict = measureCollectionHeader(panel); verdict != 0) {
        return verdict;
    }
    return 0;
}

} // namespace checks
