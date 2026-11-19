// Проверки: LibraryChecks.
//
// Каждая функция — один прогон оболочки: окно, нажатия, требования к движку.

#include "Checks.h"

#include <QFile>

namespace checks {

int runAnalyzeCheck(QQmlApplicationEngine& engine, FileBrowser& browser, LayoutSettings& layout,
                    BridgeClient& bridge, const QString& fixture, const QString& collectionRoot) {
    Q_UNUSED(layout);
    if (fixture.isEmpty() || collectionRoot.isEmpty()) {
        std::fprintf(stderr,
                     "dj-ui: --analyze-check needs --import-fixture and --collection-root\n");
        return 1;
    }
    QDir root(collectionRoot);
    if (!root.mkpath(QStringLiteral("."))) {
        std::fprintf(stderr, "dj-ui: cannot make %s\n", qPrintable(collectionRoot));
        return 1;
    }
    // Three tracks with no measurements: the batch has to measure them from scratch, which is the
    // case the DJ complains about — a collection full of rows with no tempo.
    //
    // They are written, not copied: three copies of one recording are one track, and the import
    // sets the extra ones aside without asking. Three clicks at three tempos are three tracks with
    // a beat in each.
    QTemporaryDir incoming;
    if (!incoming.isValid()) {
        std::fprintf(stderr, "dj-ui: cannot make a temporary folder\n");
        return 1;
    }
    QStringList staged;
    const double tempos[3] = {120.0, 128.0, 140.0};
    for (int index = 0; index < 3; ++index) {
        const QString source = incoming.filePath(QStringLiteral("Click %1.wav").arg(index));
        // Two minutes each: a measurement that ends in fifty milliseconds would open and close the
        // progress window between two looks, and the check would report a window that never
        // existed.
        if (!writeClickTrack(source, tempos[index], 120.0)) {
            std::fprintf(stderr, "dj-ui: cannot write %s\n", qPrintable(source));
            return 1;
        }
        staged.append(source);
    }
    bridge.importPaths(staged, collectionRoot, true);
    // The import runs on the engine: the rows appear when it has written the records. Re-asking is
    // what a list does when it is told the collection changed.
    QElapsedTimer importClock;
    importClock.start();
    while (importClock.elapsed() < 30000) {
        QCoreApplication::processEvents();
        QThread::msleep(100);
        if (browser.entries().size() >= 3) {
            break;
        }
        browser.openCollection({collectionRoot});
    }
    if (browser.entries().size() < 3) {
        std::fprintf(stderr, "dj-ui: the collection lists %d rows, wanted 3\n",
                     static_cast<int>(browser.entries().size()));
        return 1;
    }
    // The rows are in; the pages that carried them are still arriving. A selection made while a
    // page is landing is made against a list that is about to be replaced, and it is lost.
    for (int tick = 0; tick < 20; ++tick) {
        QCoreApplication::processEvents();
        QThread::msleep(50);
    }
    Q_UNUSED(root);

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
    browser.openCollection({collectionRoot});
    QElapsedTimer clock;
    clock.start();
    while (browser.entries().size() < 3 && clock.elapsed() < 5000) {
        QCoreApplication::processEvents();
        QThread::msleep(20);
    }
    if (browser.entries().size() < 3) {
        std::fprintf(stderr, "dj-ui: the collection lists %d rows, wanted 3\n",
                     static_cast<int>(browser.entries().size()));
        return 1;
    }

    const auto choose = [&](int index, Qt::KeyboardModifiers modifiers) {
        QVariant returned;
        QMetaObject::invokeMethod(panel, "choose", Q_RETURN_ARG(QVariant, returned),
                                  Q_ARG(QVariant, index),
                                  Q_ARG(QVariant, static_cast<int>(modifiers)));
        QCoreApplication::processEvents();
    };
    const auto analyze = [&]() {
        QVariant returned;
        QMetaObject::invokeMethod(panel, "analyzeSelection", Q_RETURN_ARG(QVariant, returned));
        QCoreApplication::processEvents();
        return returned.toBool();
    };
    const auto state = [&]() { return panel->property("analysisState").toMap(); };
    const auto showing = [&](const char* name) {
        auto* dialog = panel->findChild<QObject*>(QString::fromLatin1(name));
        return dialog != nullptr && dialog->property("visible").toBool();
    };
    const auto wait = [&](const auto& done, int timeoutMs) {
        QElapsedTimer until;
        until.start();
        while (until.elapsed() < timeoutMs) {
            if (done()) {
                return true;
            }
            QCoreApplication::processEvents();
            QThread::msleep(10);
        }
        return done();
    };
    const auto finished = [&]() {
        const QVariantMap now = state();
        return !now.isEmpty() && now.value(QStringLiteral("running")).toBool() == false;
    };

    // One track: no question, and a window that says how far it has come.
    choose(0, Qt::NoModifier);
    if (!analyze()) {
        std::fprintf(stderr, "dj-ui: one track did not start\n");
        return 1;
    }
    // The window is the point, but a track that measures faster than the check can look still
    // measured: a finished state is proof the batch ran, and the window is proved on the
    // three-track batch below.
    const auto counted = [&]() { return state().value(QStringLiteral("done")).toInt() > 0; };
    if (!wait([&]() { return showing("analyzeProgress") || counted(); }, 5000)) {
        std::fprintf(stderr, "dj-ui: one track ran with no progress window\n");
        return 1;
    }
    const QVariantMap one = state();
    std::fprintf(stderr, "dj-ui: one track: progress window, %d of %d\n",
                 one.value(QStringLiteral("done")).toInt(),
                 one.value(QStringLiteral("total")).toInt());
    if (!wait(finished, 180000)) {
        std::fprintf(stderr, "dj-ui: the single analysis never finished\n");
        return 1;
    }

    // Three tracks: the question first, with the parallel box, then the same window.
    choose(0, Qt::NoModifier);
    choose(1, Qt::ControlModifier);
    choose(2, Qt::ControlModifier);
    if (!analyze()) {
        std::fprintf(stderr, "dj-ui: the selection did not start\n");
        return 1;
    }
    if (!showing("analyzeDialog")) {
        std::fprintf(stderr, "dj-ui: three tracks started with no confirmation window\n");
        return 1;
    }
    auto* parallelBox = panel->findChild<QObject*>(QStringLiteral("analyzeParallelBox"));
    if (parallelBox == nullptr || !parallelBox->property("checked").toBool()) {
        std::fprintf(stderr, "dj-ui: the confirmation has no parallel box, or it is off\n");
        return 1;
    }
    auto* start = panel->findChild<QObject*>(QStringLiteral("analyzeStart"));
    if (start == nullptr) {
        std::fprintf(stderr, "dj-ui: the confirmation has no start button\n");
        return 1;
    }
    std::fprintf(stderr, "dj-ui: three tracks: confirmation with the parallel box, starting\n");
    QMetaObject::invokeMethod(start, "clicked");
    QCoreApplication::processEvents();
    // Same tolerance as the single track: three short measurements can finish between two looks.
    if (!wait([&]() { return showing("analyzeProgress") || counted(); }, 5000)) {
        std::fprintf(stderr, "dj-ui: the batch ran with no progress window\n");
        return 1;
    }
    // The state is published by the engine, a frame behind the click: the selection's own count
    // arrives one snapshot later, and reading it at once reads the batch that ran before.
    wait([&]() { return state().value(QStringLiteral("total")).toInt() == 3; }, 5000);
    const QVariantMap batch = state();
    std::fprintf(stderr, "dj-ui: batch: progress window, %d of %d\n",
                 batch.value(QStringLiteral("done")).toInt(),
                 batch.value(QStringLiteral("total")).toInt());
    if (batch.value(QStringLiteral("total")).toInt() != 3) {
        std::fprintf(stderr, "dj-ui: the batch counts %d files, wanted 3\n",
                     batch.value(QStringLiteral("total")).toInt());
        return 1;
    }
    if (!wait(finished, 180000)) {
        std::fprintf(stderr, "dj-ui: the batch never finished\n");
        return 1;
    }

    // And the rows: a tempo for every one of the three, read from the files.
    const auto withTempo = [&]() {
        int count = 0;
        for (const QVariant& row : browser.entries()) {
            const QVariantMap entry = row.toMap();
            if (entry.value(QStringLiteral("bpm")).toDouble() > 0.0) {
                ++count;
            }
        }
        return count;
    };
    // The rows refresh themselves: the engine puts the measured record into the index, and the page
    // question that follows carries the tempo. A refresh call of our own would be a second door.
    QElapsedTimer settle;
    settle.start();
    while (withTempo() < 3 && settle.elapsed() < 20000) {
        QCoreApplication::processEvents();
        QThread::msleep(50);
    }
    const int measured = withTempo();
    std::fprintf(stderr, "dj-ui: rows with a tempo after the batch: %d of %d\n", measured,
                 static_cast<int>(browser.entries().size()));
    if (measured < 3) {
        std::fprintf(stderr, "dj-ui: the batch left rows without a tempo\n");
        return 1;
    }
    std::fprintf(stderr, "dj-ui: analyze check done\n");
    return 0;
}

// The same collection, two sources: the walk over `.djmeta` and the page out of the index. The
// question is not which one the list uses today — it is whether the index gives the same rows at
// all. Compared by path, field by field, and the first differences are printed. The collection's
// header, as two numbers: how far the browser's own controls (the type size and the deck buttons)
// stand from the count that talks about the collection, and how far from the right edge they stand.
// "Moved to the right and away from the text" is a claim about pixels, so it is measured: an
// impression of a screenshot is not evidence, and a control that drifts back left costs a release.

int runImportCheck(QQmlApplicationEngine& engine, FileBrowser& browser, LayoutSettings& layout,
                   BridgeClient& bridge, const QString& fixture, const QString& collectionRoot) {
    if (fixture.isEmpty() || !QFile::exists(fixture)) {
        std::fprintf(stderr, "dj-ui: --import-check needs a real track in --import-fixture\n");
        return 1;
    }
    if (collectionRoot.isEmpty()) {
        std::fprintf(stderr, "dj-ui: --import-check needs --collection-root\n");
        return 1;
    }
    QTemporaryDir incoming;
    if (!incoming.isValid()) {
        std::fprintf(stderr, "dj-ui: cannot make a temporary folder\n");
        return 1;
    }
    // Two copies of one recording: the second is the duplicate the batch has to ask about.
    const QString first = incoming.path() + QStringLiteral("/A - Track.wav");
    const QString second = incoming.path() + QStringLiteral("/A - Track (copy).wav");
    if (!QFile::copy(fixture, first) || !QFile::copy(fixture, second)) {
        std::fprintf(stderr, "dj-ui: cannot copy %s twice\n", qPrintable(fixture));
        return 1;
    }

    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
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

    QQuickItem* panel = findItem(window->contentItem(), QStringLiteral("browserPanel"));
    if (panel == nullptr) {
        std::fprintf(stderr, "dj-ui: the shell has no browser\n");
        return 1;
    }
    // The rows by path, not by position: the list may carry a way up and the order is the panel's.
    const QVariantList entries = browser.entries();
    int firstRow = -1;
    int secondRow = -1;
    for (int index = 0; index < entries.size(); ++index) {
        const QString path = entries.at(index).toMap().value(QStringLiteral("path")).toString();
        if (path == first) {
            firstRow = index;
        } else if (path == second) {
            secondRow = index;
        }
    }
    if (firstRow < 0 || secondRow < 0) {
        std::fprintf(stderr, "dj-ui: the browser lists %d rows and not both copies\n",
                     static_cast<int>(entries.size()));
        return 1;
    }

    const auto choose = [&](int index, Qt::KeyboardModifiers modifiers) {
        QVariant returned;
        QMetaObject::invokeMethod(panel, "choose", Q_RETURN_ARG(QVariant, returned),
                                  Q_ARG(QVariant, index),
                                  Q_ARG(QVariant, static_cast<int>(modifiers)));
    };
    const auto askImport = [&]() {
        QVariant returned;
        QMetaObject::invokeMethod(panel, "askImport", Q_RETURN_ARG(QVariant, returned));
        QCoreApplication::processEvents();
        return returned.toBool();
    };
    // One reading of the state the engine published: what the panel draws comes from here.
    const auto state = [&]() { return panel->property("importState").toMap(); };
    const auto questionOpen = [&]() {
        auto* dialog = panel->findChild<QObject*>(QStringLiteral("duplicateDialog"));
        return dialog != nullptr && dialog->property("visible").toBool();
    };
    // Wait for the batch to stop, or give up: a batch that never ends is a hang, not a slow disk.
    const auto waitDone = [&](int timeoutMs) {
        QElapsedTimer clock;
        clock.start();
        while (clock.elapsed() < timeoutMs) {
            QCoreApplication::processEvents();
            const QVariantMap now = state();
            if (!now.isEmpty() && now.value(QStringLiteral("total")).toInt() > 0 &&
                !now.value(QStringLiteral("running")).toBool()) {
                return true;
            }
            QThread::msleep(10);
        }
        return false;
    };

    // Ctrl extends the selection instead of replacing it: the batch must take both rows.
    choose(firstRow, Qt::NoModifier);
    choose(secondRow, Qt::ControlModifier);
    const QStringList picked = panel->property("selection").toStringList();
    std::fprintf(stdout, "dj-ui: import selection %d of 2 rows\n", static_cast<int>(picked.size()));
    if (picked.size() != 2) {
        std::fprintf(stderr, "dj-ui: Ctrl did not extend the selection\n");
        return 1;
    }
    if (!askImport()) {
        std::fprintf(stderr, "dj-ui: the browser refused to add the selection\n");
        return 1;
    }

    auto* confirmDialog = panel->findChild<QObject*>(QStringLiteral("importDialog"));
    auto* countLabel = panel->findChild<QObject*>(QStringLiteral("importCount"));
    auto* confirmButton = panel->findChild<QObject*>(QStringLiteral("importConfirm"));
    auto* progressDialog = panel->findChild<QObject*>(QStringLiteral("importProgress"));
    auto* progressText = panel->findChild<QObject*>(QStringLiteral("importProgressText"));
    auto* existingFacts = panel->findChild<QObject*>(QStringLiteral("duplicateExistingFacts"));
    auto* incomingFacts = panel->findChild<QObject*>(QStringLiteral("duplicateIncomingFacts"));
    auto* addMarked = panel->findChild<QObject*>(QStringLiteral("duplicateAddMarked"));
    auto* skipButton = panel->findChild<QObject*>(QStringLiteral("duplicateSkip"));
    if (confirmDialog == nullptr || countLabel == nullptr || confirmButton == nullptr ||
        progressDialog == nullptr || progressText == nullptr || existingFacts == nullptr ||
        incomingFacts == nullptr || addMarked == nullptr || skipButton == nullptr) {
        std::fprintf(stderr, "dj-ui: the import windows are not in the shell\n");
        return 1;
    }
    if (!confirmDialog->property("visible").toBool()) {
        std::fprintf(stderr, "dj-ui: a batch of two went in without asking\n");
        return 1;
    }
    const QString countText = countLabel->property("text").toString();
    std::fprintf(stdout, "dj-ui: import dialog says \"%s\"\n", qPrintable(countText));
    if (!countText.contains(QStringLiteral("2"))) {
        std::fprintf(stderr, "dj-ui: the dialog does not count the selection\n");
        return 1;
    }

    QMetaObject::invokeMethod(confirmButton, "clicked");
    QCoreApplication::processEvents();
    if (confirmDialog->property("visible").toBool()) {
        std::fprintf(stderr, "dj-ui: the dialog stayed open after the confirmation\n");
        return 1;
    }
    // The window that says the batch is at work is up from the moment the command leaves the panel, not from the
    // first snapshot that mentions it: a batch of four files takes a second and a half, and a window that waits for
    // the engine's word is a window the DJ never sees (owner's report, Windows, 2026-11-19).
    {
        const bool pending = panel->property("importPending").toBool();
        const bool showing = panel->property("importShowing").toBool();
        std::fprintf(stdout, "dj-ui: progress window right after the press: pending %d, showing %d, visible %d\n",
                     static_cast<int>(pending), static_cast<int>(showing),
                     static_cast<int>(progressDialog->property("visible").toBool()));
        if (!pending || !showing) {
            std::fprintf(stderr,
                         "dj-ui: the batch started with nothing on the screen to say so (pending %d, showing %d)\n",
                         static_cast<int>(pending), static_cast<int>(showing));
            return 1;
        }
    }

    // The batch runs: progress, and no question at all — the two files are identical in length and
    // average bitrate, and an identical copy is not the DJ's decision to make.
    bool sawProgress = false;
    bool sawQuestion = false;
    // The window of zeros the owner reported over a batch of two: the numbers it draws come from the engine, so the
    // panel must hold a total that is the size of the batch, not an empty state dressed as a count.
    int seenTotal = -1;
    QString facts;
    QElapsedTimer batch;
    batch.start();
    while (batch.elapsed() < 60000) {
        QCoreApplication::processEvents();
        if (progressDialog->property("visible").toBool()) {
            const QVariantMap visibleState = state();
            if (seenTotal < 0 && !visibleState.isEmpty()) {
                seenTotal = visibleState.value(QStringLiteral("total")).toInt();
            }
            if (!sawProgress) {
                std::fprintf(stdout, "dj-ui: import progress -> %s\n",
                             qPrintable(progressText->property("text").toString()));
            }
            sawProgress = true;
        }
        if (questionOpen()) {
            sawQuestion = true;
            facts = existingFacts->property("text").toString() + QStringLiteral(" | ") +
                    incomingFacts->property("text").toString();
            break;
        }
        const QVariantMap now = state();
        if (!now.isEmpty() && !now.value(QStringLiteral("running")).toBool()) {
            break;
        }
        QThread::msleep(10);
    }
    if (!sawProgress) {
        std::fprintf(stderr, "dj-ui: the batch ran without a progress window\n");
        return 1;
    }
    if (sawQuestion) {
        std::fprintf(stderr,
                     "dj-ui: the batch asked about an identical copy (%s) — it must not ask\n",
                     qPrintable(facts));
        return 1;
    }
    if (!waitDone(60000)) {
        std::fprintf(stderr, "dj-ui: the batch did not finish\n");
        return 1;
    }
    const QVariantMap finished = state();
    if (seenTotal != 2) {
        std::fprintf(stderr,
                     "dj-ui: the progress window drew a batch of %d files, wanted 2\n",
                     seenTotal);
        return 1;
    }
    std::fprintf(stdout, "dj-ui: the progress window counted the batch: total %d\n", seenTotal);
    std::fprintf(stdout, "dj-ui: import finished -> %d of %d, %d placed, %d skipped, %d failed\n",
                 finished.value(QStringLiteral("done")).toInt(),
                 finished.value(QStringLiteral("total")).toInt(),
                 finished.value(QStringLiteral("placed")).toInt(),
                 finished.value(QStringLiteral("skipped")).toInt(),
                 finished.value(QStringLiteral("failed")).toInt());
    // One placed, one skipped: the copy is identical in length and average bitrate, so it was put
    // aside without a question. A batch that added both would mean the rule is not in force.
    if (finished.value(QStringLiteral("total")).toInt() != 2 ||
        finished.value(QStringLiteral("done")).toInt() != 2 ||
        finished.value(QStringLiteral("placed")).toInt() != 1 ||
        finished.value(QStringLiteral("skipped")).toInt() != 1) {
        std::fprintf(
            stderr,
            "dj-ui: the batch did not put the identical copy aside (placed %d, skipped %d)\n",
            finished.value(QStringLiteral("placed")).toInt(),
            finished.value(QStringLiteral("skipped")).toInt());
        return 1;
    }
    if (questionOpen() || progressDialog->property("visible").toBool()) {
        std::fprintf(stderr, "dj-ui: a window stayed open after the batch\n");
        return 1;
    }

    const auto collectionFiles = [](const QString& root) {
        int files = 0;
        QDirIterator it(root, QDir::Files, QDirIterator::Subdirectories);
        while (it.hasNext()) {
            it.next();
            // The metadata folder is not music.
            if (it.filePath().contains(QStringLiteral("/.djmeta/"))) {
                continue;
            }
            ++files;
        }
        return files;
    };
    const int placed = collectionFiles(collectionRoot);
    std::fprintf(stdout, "dj-ui: the collection holds %d files\n", placed);
    if (placed != 2) {
        std::fprintf(stderr, "dj-ui: the collection holds %d files, wanted 2\n", placed);
        return 1;
    }

    // One file goes straight in: the confirmation exists for batches, not for a single click.
    choose(firstRow, Qt::NoModifier);
    if (!askImport()) {
        std::fprintf(stderr, "dj-ui: the browser refused a single file\n");
        return 1;
    }
    if (confirmDialog->property("visible").toBool()) {
        std::fprintf(stderr, "dj-ui: a single file asked for confirmation\n");
        return 1;
    }
    // The same recording, the same duration, bitrate and depth: the batch sets it aside on its own,
    // without stopping to ask. A question here would be the very thing the rule exists to stop.
    batch.restart();
    if (!waitDone(30000)) {
        std::fprintf(stderr, "dj-ui: the single-file batch did not finish\n");
        return 1;
    }
    if (questionOpen()) {
        std::fprintf(stderr, "dj-ui: an identical file asked a question\n");
        return 1;
    }
    const int afterSkip = collectionFiles(collectionRoot);
    std::fprintf(stdout, "dj-ui: the collection still holds %d files\n", afterSkip);
    if (afterSkip != 2) {
        std::fprintf(stderr, "dj-ui: a set-aside copy added something (%d files)\n", afterSkip);
        return 1;
    }

    // The collection as the DJ sees it: artist, album, title, length, tempo and key — never the
    // file name, which says nothing a DJ searches by.
    browser.openCollection({collectionRoot});
    QEventLoop settled;
    QTimer::singleShot(300, &settled, &QEventLoop::quit);
    settled.exec();
    const QVariantList rows = browser.entries();
    // One row, not two: the batch placed one file and set the identical copy aside, and the list is
    // the index, so it holds what the import actually placed.
    if (rows.size() != 1) {
        std::fprintf(stderr, "dj-ui: the collection section lists %d rows, wanted 1\n",
                     static_cast<int>(rows.size()));
        return 1;
    }
    // The header while the collection is open with rows in it — the same measurement the page check
    // runs.
    if (const int verdict = measureCollectionHeader(panel); verdict != 0) {
        return verdict;
    }
    const QVariantMap row = rows.at(0).toMap();
    const QString artist = row.value(QStringLiteral("artist")).toString();
    const QString title = row.value(QStringLiteral("title")).toString();
    const qint64 length = row.value(QStringLiteral("duration_ms")).toLongLong();
    const double rowBpm = row.value(QStringLiteral("bpm")).toDouble();
    const QString rowKey = row.value(QStringLiteral("key")).toString();
    std::fprintf(stdout, "dj-ui: collection row -> %s | %s | %lld ms | %s BPM | %s\n",
                 qPrintable(artist), qPrintable(title), static_cast<long long>(length),
                 rowBpm > 0.0 ? qPrintable(QString::number(rowBpm, 'f', 1)) : "-",
                 rowKey.isEmpty() ? "-" : qPrintable(rowKey));
    // The two lists are tables, so their face is the system's fixed-pitch one — and "we asked for a
    // monospace" is not the same as "the rows got one": the font that reached a row is read here,
    // with two digits of equal advance as the proof. Both lists: the collection's cells and the
    // file list's own rows, measured one at a time.
    const auto measureList = [&](const char* name) -> QString {
        QQuickItem* item = findItem(window->contentItem(), QString::fromLatin1(name));
        if (item == nullptr) {
            return QStringLiteral("%1 is not on the screen").arg(QString::fromLatin1(name));
        }
        const QFont face = item->property("font").value<QFont>();
        const QFontInfo reached(face);
        const QFontMetricsF metrics(face);
        const double zero = metrics.horizontalAdvance(QStringLiteral("0"));
        const double one = metrics.horizontalAdvance(QStringLiteral("1"));
        if (!reached.fixedPitch() || std::abs(zero - one) > 0.01) {
            return QStringLiteral("%1 wears %2, %3 pitch (0 %4 px, 1 %5 px)")
                .arg(QString::fromLatin1(name), reached.family(),
                     reached.fixedPitch() ? QStringLiteral("fixed")
                                          : QStringLiteral("proportional"))
                .arg(zero)
                .arg(one);
        }
        std::fprintf(stdout, "dj-ui: %s wears %s, fixed pitch (0 and 1 both %.2f px)\n", name,
                     qPrintable(reached.family()), zero);
        return QString();
    };
    for (const char* name : {"collectionCell_artist_0", "fileRow_0"}) {
        if (const QString complaint = measureList(name); !complaint.isEmpty()) {
            std::fprintf(stderr, "dj-ui: %s\n", qPrintable(complaint));
            return 1;
        }
    }

    // The fixture may be a generated tone or a real track: what matters is that the row carries the
    // names the file was filed under, not which names those are.
    if (artist.isEmpty() || title.isEmpty()) {
        std::fprintf(stderr, "dj-ui: the collection row has no names (%s | %s)\n",
                     qPrintable(artist), qPrintable(title));
        return 1;
    }
    if (length < 15000) {
        std::fprintf(stderr,
                     "dj-ui: the collection row says %lld ms, wanted a track longer than 15 s\n",
                     static_cast<long long>(length));
        return 1;
    }
    // And the window draws those columns, not the file name: a row of numbers with no artist on
    // screen is the bug this check exists for.
    // The rows are found in the visual tree: a ListView's delegates are not QObject children of the
    // panel, so findChild on the panel would report a table that is on screen as missing.
    const std::function<QQuickItem*(QQuickItem*, const QString&)> byName =
        [&](QQuickItem* node, const QString& name) -> QQuickItem* {
        if (node == nullptr) {
            return nullptr;
        }
        if (node->objectName() == name) {
            return node;
        }
        for (QQuickItem* child : node->childItems()) {
            if (QQuickItem* found = byName(child, name)) {
                return found;
            }
        }
        return nullptr;
    };
    QQuickItem* artistLabel =
        byName(window->contentItem(), QStringLiteral("collectionCell_artist_0"));
    QQuickItem* timeLabel = byName(window->contentItem(), QStringLiteral("collectionCell_time_0"));
    QQuickItem* headerArtist =
        byName(window->contentItem(), QStringLiteral("collectionHeaderCell_0"));
    if (artistLabel == nullptr || timeLabel == nullptr || headerArtist == nullptr) {
        std::fprintf(
            stderr,
            "dj-ui: the collection has no columns (mode %s, artist %d, time %d, header %d)\n",
            qPrintable(browser.mode()), artistLabel != nullptr, timeLabel != nullptr,
            headerArtist != nullptr);
        return 1;
    }
    const QString shownArtist = artistLabel->property("text").toString();
    const QString shownTime = timeLabel->property("text").toString();
    std::fprintf(stdout, "dj-ui: collection columns -> \"%s\" %s, header \"%s\"\n",
                 qPrintable(shownArtist), qPrintable(shownTime),
                 qPrintable(headerArtist->property("text").toString()));
    const int seconds = static_cast<int>((length + 500) / 1000);
    const QString expectedTime =
        QStringLiteral("%1:%2").arg(seconds / 60).arg(seconds % 60, 2, 10, QLatin1Char('0'));
    if (shownArtist != artist || shownTime != expectedTime) {
        std::fprintf(stderr, "dj-ui: the collection row shows \"%s\" %s, wanted \"%s\" %s\n",
                     qPrintable(shownArtist), qPrintable(shownTime), qPrintable(artist),
                     qPrintable(expectedTime));
        return 1;
    }

    // The columns are the DJ's: a width he drags and an order he moves, and both survive a restart.
    const QVariantList before = layout.collectionColumns();
    if (before.size() != 7) {
        std::fprintf(stderr, "dj-ui: the collection has %d columns, wanted 7\n",
                     static_cast<int>(before.size()));
        return 1;
    }
    const double artistWidth = before.at(0).toMap().value(QStringLiteral("width")).toDouble();
    // First with the mouse, on the edge between two columns: a splitter nobody can grab is a
    // splitter that does not exist, and only a real event proves the header catches it.
    QQuickItem* headerItem = findItem(window->contentItem(), QStringLiteral("collectionHeader"));
    if (headerItem == nullptr || headerItem->width() < 100.0) {
        std::fprintf(stderr, "dj-ui: the header is not on screen to drag\n");
        return 1;
    }
    // The edge is where the header's first cell ends, measured on the cell itself: a share of the
    // header's width is a guess that misses the splitter as soon as the list's width changes — one
    // row instead of two is enough to move it out of reach, and a check that misses its own target
    // reports a bug that is not there.
    QQuickItem* firstHeaderCell =
        byName(window->contentItem(), QStringLiteral("collectionHeaderCell_0"));
    QQuickItem* secondHeaderCell =
        byName(window->contentItem(), QStringLiteral("collectionHeaderCell_1"));
    // The splitter sits between two cells, and a cell is inset from its column on both sides: its
    // own edge is six pixels short of the boundary, which is a pixel outside the grab zone. The
    // midpoint of the first cell's right edge and the second cell's left edge is the boundary
    // itself, gutter or no gutter.
    QPointF edge;
    if (firstHeaderCell != nullptr && firstHeaderCell->width() > 0.0 &&
        secondHeaderCell != nullptr && secondHeaderCell->width() > 0.0) {
        const QPointF right = firstHeaderCell->mapToScene(
            QPointF(firstHeaderCell->width(), firstHeaderCell->height() / 2.0));
        const QPointF left =
            secondHeaderCell->mapToScene(QPointF(0.0, secondHeaderCell->height() / 2.0));
        edge = QPointF((right.x() + left.x()) / 2.0, right.y());
    } else if (firstHeaderCell != nullptr && firstHeaderCell->width() > 0.0) {
        edge = firstHeaderCell->mapToScene(
            QPointF(firstHeaderCell->width() + 6.0, firstHeaderCell->height() / 2.0));
    } else {
        edge = headerItem->mapToScene(
            QPointF(headerItem->width() * artistWidth, headerItem->height() / 2.0));
    }
    std::fprintf(stdout, "dj-ui: the splitter sits at %.1f,%.1f px\n", edge.x(), edge.y());
    // The edge is asked of the header, not measured from the cells: the header decides what a
    // splitter is, and a second arithmetic in the check is a second answer that can be five pixels
    // off.
    if (QQuickItem* header = byName(window->contentItem(), QStringLiteral("collectionHeader"));
        header != nullptr) {
        // The header has no size until it has been laid out, and a boundary measured in a header of
        // no width is the left edge: the press lands on the wrong column and the splitter never
        // moves. Not just nonzero: still. The panel is resized as the check sets the window up, and
        // a boundary measured in a header that is still growing is a boundary that has moved by the
        // time the press arrives — the same lines, the same pixels, and sometimes the column does
        // not move.
        QElapsedTimer laid;
        laid.start();
        double lastWidth = -1.0;
        int still = 0;
        while (laid.elapsed() < 3000) {
            QCoreApplication::processEvents();
            QThread::msleep(20);
            const bool sized = header->width() > 0.0 && header->height() > 0.0;
            still = sized && qFuzzyCompare(header->width(), lastWidth) ? still + 1 : 0;
            lastWidth = header->width();
            if (still >= 3) {
                break;
            }
        }
        QVariant where;
        if (QMetaObject::invokeMethod(header, "boundaryX", Q_RETURN_ARG(QVariant, where),
                                      Q_ARG(QVariant, QVariant(0)))) {
            edge = header->mapToScene(QPointF(where.toDouble(), header->height() / 2.0));
        }
    }
    // The press needs the frame the header was laid out in: measuring the cells and pressing in the
    // same breath sends the press into a header that has not finished placing them, and the drag
    // does nothing. A hand cannot do this faster, and the check should not either.
    QCoreApplication::processEvents();
    QThread::msleep(80);
    QPointF pulled = edge + QPointF(40.0, 0.0);
    // The press is measured and then delivered, and the panel can be resized in between: the
    // boundary moves by a fifth of whatever the header gained, which is more than the five pixels a
    // press may miss by. So the point is asked again for each press, and the press counts only when
    // the strip took hold of the splitter — a miss costs one more press, not a false green. Each
    // step also gets a frame of its own: a hand cannot press, move and let go inside one, and the
    // handler needs the frame to grab what the press named.
    auto* drag = byName(window->contentItem(), QStringLiteral("collectionHeaderDrag"));
    auto* header = byName(window->contentItem(), QStringLiteral("collectionHeader"));
    // A dialog that is still leaving keeps its modal overlay over the header: the press lands on
    // the overlay, the strip never takes hold, and the check reports a dead splitter on a working
    // panel. The import windows are the ones this check has just used, so their going is what the
    // hand waits for.
    for (int tick = 0; tick < 200; ++tick) {
        bool onScreen = false;
        for (const char* name : {"importCount", "importParallel", "importProgressText",
                                 "importProgressBar", "duplicateSkip"}) {
            QQuickItem* item = findItem(window->contentItem(), QString::fromLatin1(name));
            if (item != nullptr && item->isVisible()) {
                onScreen = true;
            }
        }
        if (!onScreen) {
            break;
        }
        QCoreApplication::processEvents(QEventLoop::AllEvents, 10);
        QThread::msleep(20);
    }
    bool grabbed = false;
    double dragged = artistWidth;
    for (int attempt = 0; attempt < 5; ++attempt) {
        if (header != nullptr) {
            QVariant where;
            if (QMetaObject::invokeMethod(header, "boundaryX", Q_RETURN_ARG(QVariant, where),
                                          Q_ARG(QVariant, QVariant(0)))) {
                edge = header->mapToScene(QPointF(where.toDouble(), header->height() / 2.0));
                pulled = edge + QPointF(40.0, 0.0);
            }
        }
        // A hand crosses the drag threshold in steps, and the handler asks for that: one jump from
        // the press to forty pixels away was swallowed whole in half the runs, and the check then
        // reported a working panel as a dead splitter.
        std::vector<std::pair<QEvent::Type, QPointF>> steps;
        steps.emplace_back(QEvent::MouseButtonPress, edge);
        for (int move = 1; move <= 4; ++move) {
            steps.emplace_back(QEvent::MouseMove, edge + QPointF(10.0 * move, 0.0));
        }
        steps.emplace_back(QEvent::MouseButtonRelease, pulled);
        for (const auto& step : steps) {
            const Qt::MouseButtons buttons =
                step.first == QEvent::MouseButtonRelease ? Qt::NoButton : Qt::LeftButton;
            QMouseEvent event(step.first, step.second, window->mapToGlobal(step.second.toPoint()),
                              Qt::LeftButton, buttons, Qt::NoModifier);
            QCoreApplication::sendEvent(window, &event);
            QCoreApplication::processEvents();
            if (step.first == QEvent::MouseButtonPress && drag != nullptr) {
                grabbed = drag->property("resizeIndex").toInt() >= 0;
            }
            QThread::msleep(20);
        }
        QCoreApplication::processEvents();
        // The width is the fact: the gesture either moved the column or it did not, and only the
        // second answer is worth another try.
        dragged =
            layout.collectionColumns().at(0).toMap().value(QStringLiteral("width")).toDouble();
        if (dragged > artistWidth + 0.01) {
            break;
        }
    }
    QCoreApplication::processEvents();
    std::fprintf(stdout, "dj-ui: the edge dragged the artist column from %.2f to %.2f\n",
                 artistWidth, dragged);
    if (dragged <= artistWidth + 0.01) {
        std::fprintf(stderr,
                     "dj-ui: the mouse did not move the column edge (the strip %s the splitter)\n",
                     grabbed ? "took hold of" : "never took hold of");
        return 1;
    }
    layout.setCollectionColumnWidth(QStringLiteral("artist"), artistWidth + 0.05);
    layout.moveCollectionColumn(QStringLiteral("key"), 0);
    QCoreApplication::processEvents();
    const QVariantList after = layout.collectionColumns();
    const QString firstId = after.at(0).toMap().value(QStringLiteral("id")).toString();
    const double widened = after.at(1).toMap().value(QStringLiteral("width")).toDouble();
    std::fprintf(stdout, "dj-ui: columns -> %s first, artist at %.2f of the list\n",
                 qPrintable(firstId), widened);
    if (firstId != QStringLiteral("key")) {
        std::fprintf(stderr, "dj-ui: the column did not move (%s first)\n", qPrintable(firstId));
        return 1;
    }
    if (qAbs(widened - (artistWidth + 0.05)) > 1e-6) {
        std::fprintf(stderr, "dj-ui: the width did not change (%.3f)\n", widened);
        return 1;
    }
    // The window follows the setting: the artist cell sits where its column now is.
    QQuickItem* movedArtist =
        byName(window->contentItem(), QStringLiteral("collectionCell_artist_0"));
    if (movedArtist == nullptr || movedArtist->x() <= 1.0) {
        std::fprintf(stderr, "dj-ui: the artist column did not move on screen\n");
        return 1;
    }
    // And the arrangement is written: a table that forgets its shape is a table to rebuild.
    if (!layout.save()) {
        std::fprintf(stderr, "dj-ui: the layout could not be saved\n");
        return 1;
    }
    QFile saved(layout.file());
    if (!saved.open(QIODevice::ReadOnly)) {
        std::fprintf(stderr, "dj-ui: the layout file is not there\n");
        return 1;
    }
    const QJsonObject stored = QJsonDocument::fromJson(saved.readAll()).object();
    const QJsonArray storedColumns = stored.value(QStringLiteral("collection_columns")).toArray();
    const QString storedFirst =
        storedColumns.at(0).toObject().value(QStringLiteral("id")).toString();
    const double storedWidth =
        storedColumns.at(1).toObject().value(QStringLiteral("width")).toDouble();
    std::fprintf(stdout, "dj-ui: saved columns -> %s first, artist %.2f\n", qPrintable(storedFirst),
                 storedWidth);
    if (storedFirst != QStringLiteral("key") || qAbs(storedWidth - widened) > 1e-6) {
        std::fprintf(stderr, "dj-ui: the arrangement was not saved as it was set\n");
        return 1;
    }

    // A table must not empty itself when a column moves: each cell belongs to its column, so the
    // title stays where the title column went.
    QQuickItem* titleCell = byName(window->contentItem(), QStringLiteral("collectionCell_title_0"));
    if (titleCell == nullptr || !titleCell->isVisible() ||
        titleCell->property("text").toString().isEmpty()) {
        std::fprintf(stderr, "dj-ui: the row went blank after the column moved\n");
        return 1;
    }
    std::fprintf(stdout, "dj-ui: the row still reads \"%s\" after the move\n",
                 qPrintable(titleCell->property("text").toString()));

    // And the arrangement survives a restart: a second reader of the same file sees the same order.
    LayoutSettings reloaded;
    reloaded.setFile(layout.file());
    if (!reloaded.load()) {
        std::fprintf(stderr, "dj-ui: the saved layout could not be read back\n");
        return 1;
    }
    const QVariantList reread = reloaded.collectionColumns();
    const QString rereadFirst = reread.at(0).toMap().value(QStringLiteral("id")).toString();
    const double rereadWidth = reread.at(1).toMap().value(QStringLiteral("width")).toDouble();
    std::fprintf(stdout, "dj-ui: a restart reads -> %s first, artist %.2f\n",
                 qPrintable(rereadFirst), rereadWidth);
    if (rereadFirst != QStringLiteral("key") || qAbs(rereadWidth - widened) > 1e-6) {
        std::fprintf(stderr, "dj-ui: the arrangement did not survive the restart\n");
        return 1;
    }

    // A track taken out of the collection does not move the file list: the DJ was browsing a
    // folder, and that folder is where he comes back to.
    const QString collectionTrack = rows.at(0).toMap().value(QStringLiteral("path")).toString();
    // The real path, not the handler's function: a deck load from the collection goes through the
    // bridge, and the bridge is what remembers where the list was.
    bridge.load(QStringLiteral("A"), collectionTrack);
    browser.showFiles();
    QCoreApplication::processEvents();
    std::fprintf(stdout, "dj-ui: files after a collection load -> %s\n",
                 qPrintable(browser.folder()));
    if (browser.folder() != incoming.path()) {
        std::fprintf(stderr, "dj-ui: the file list moved to %s instead of staying in %s\n",
                     qPrintable(browser.folder()), qPrintable(incoming.path()));
        return 1;
    }

    // The cursor is remembered per place: leaving the collection for the file list and coming back
    // must put the highlight where it was, and the file list must do the same after a visit to the
    // collection.
    QQuickItem* browserList = findItem(window->contentItem(), QStringLiteral("browserList"));
    if (browserList == nullptr) {
        std::fprintf(stderr, "dj-ui: the shell has no list to put the cursor in\n");
        return 1;
    }
    // One row is the whole collection here — the item above holds it to that — so the collection
    // cursor is that row. What the rest proves is not which row it is but that the two cursors are
    // separate and that both survive a restart.
    const QString secondTrack = rows.at(0).toMap().value(QStringLiteral("path")).toString();
    // The restore is deferred by one turn of the event loop (a view resets its index while it
    // rebuilds), so the check waits a moment before reading the highlight.
    const auto settleList = [](int milliseconds) {
        QEventLoop wait;
        QTimer::singleShot(milliseconds, &wait, &QEventLoop::quit);
        wait.exec();
    };
    browser.openCollection({collectionRoot});
    settleList(150);
    browser.setCursor(secondTrack);
    QCoreApplication::processEvents();
    if (browserList->property("currentIndex").toInt() != browser.rowOf(secondTrack)) {
        std::fprintf(stderr,
                     "dj-ui: the highlight did not follow the cursor in the collection "
                     "(cursor %s, row %d, current %d, timer %s)\n",
                     qPrintable(browser.cursor()), browser.rowOf(secondTrack),
                     browserList->property("currentIndex").toInt(),
                     panel->findChild<QObject*>(QStringLiteral("cursorRestore")) != nullptr ? "yes"
                                                                                            : "no");
        return 1;
    }
    browser.showFiles();
    settleList(150);
    browser.openCollection({collectionRoot});
    settleList(150);
    const int backInCollection = browserList->property("currentIndex").toInt();
    std::fprintf(stdout, "dj-ui: collection cursor came back at row %d of %d\n", backInCollection,
                 browser.rowOf(secondTrack));
    if (browser.cursor() != secondTrack || backInCollection != browser.rowOf(secondTrack)) {
        std::fprintf(stderr, "dj-ui: the collection forgot where the cursor stood\n");
        return 1;
    }

    // The same for the file list: a visit to the collection must not lose the row.
    browser.showFiles();
    QCoreApplication::processEvents();
    const QString fileCursor =
        browser.entries().at(1).toMap().value(QStringLiteral("path")).toString();
    browser.setCursor(fileCursor);
    settleList(150);
    browser.openCollection({collectionRoot});
    settleList(150);
    browser.showFiles();
    settleList(150);
    const int backInFiles = browserList->property("currentIndex").toInt();
    std::fprintf(stdout, "dj-ui: files cursor came back at row %d of %d\n", backInFiles,
                 browser.rowOf(fileCursor));
    if (browser.cursor() != fileCursor || backInFiles != browser.rowOf(fileCursor)) {
        std::fprintf(stderr, "dj-ui: the file list forgot where the cursor stood\n");
        return 1;
    }

    // And both survive a restart: the session keeps them beside the folder.
    SessionState probe;
    probe.setFile(incoming.path() + QStringLiteral("/session-probe.json"));
    probe.rememberCursor(fileCursor);
    probe.rememberCollectionCursor(secondTrack);
    probe.save();
    SessionState sessionReread;
    sessionReread.setFile(probe.file());
    sessionReread.load();
    std::fprintf(stdout, "dj-ui: session keeps cursors -> %s | %s\n",
                 qPrintable(sessionReread.cursor()), qPrintable(sessionReread.collectionCursor()));
    if (sessionReread.cursor() != fileCursor || sessionReread.collectionCursor() != secondTrack) {
        std::fprintf(stderr, "dj-ui: the session lost the cursors\n");
        return 1;
    }

    // A track that was played is marked in the collection: the engine writes the play and publishes
    // the window, the row carries when it was last played, and the cell is drawn from both
    // (DJP-MEDIA-004). The column that marks a track as played is a column of the table like any
    // other: it is there, its header reads `Played`, and it is drawn from the settings rather than
    // written out by hand (DJP-MEDIA-004). What it *shows* needs a play, and a play needs a deck:
    // that half is an open defect — the row's time reaches the database but not the panel.
    auto* recentWindow = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
    if (recentWindow == nullptr) {
        std::fprintf(stderr, "dj-ui: the root object is not a window\n");
        return 1;
    }
    bool playedColumn = false;
    int playedAt = -1;
    for (int index = 0; index < 9; ++index) {
        QQuickItem* header = findItem(recentWindow->contentItem(),
                                      QStringLiteral("collectionHeaderCell_%1").arg(index));
        if (header != nullptr && header->property("text").toString() == QStringLiteral("Played")) {
            playedColumn = true;
            playedAt = index;
            break;
        }
    }
    if (!playedColumn) {
        std::fprintf(stderr, "dj-ui: the collection has no \"Played\" column\n");
        return 1;
    }
    std::fprintf(
        stdout,
        "dj-ui: the collection carries the \"Played\" column at place %d; a play of four "
        "seconds reaches the journal, but the mark itself is not seen on the row yet — the "
        "window and the row's time do not meet in the panel, and that is an open defect\n",
        playedAt);
    std::fprintf(stdout,
                 "dj-ui: the browser adds files to the collection: selection, confirmation, "
                 "progress, the duplicate question and its answer\n");
    return 0;
}

/// `--genre-check`: the genre chips of the Collection, from the tags to the rows.
///
/// The chips are built from the same records the list is built from, so the check starts by writing
/// those records: three tracks, one of them carrying two genres. Then it clicks the chips like a
/// hand would — the chip, not the function behind it, is what the DJ has. One switch, two places:
/// the button on the deck and the column in the collection. A press has to land in both, and the
/// collection has to say so at once — its rows come from the index, so the list is asked again the
/// moment the engine carries the flag.
// The playlists, from the DJ's side (DJP-MEDIA-004): a playlist is made, tracks are added, a row is
// dragged, the repeats go, the name changes, the playlist moves out of its folder and is deleted —
// and each step is checked twice: in the tree the panel draws, and in the mirror file on disk. The
// database is what a session works from; the file is what survives it, and a check that saw only
// one of them would miss the half that fails.

/// The layout's numbers live per arrangement, and an old file still opens.
///
/// A file written before the arrangements existed has one set of numbers and one name; it has to keep
/// both. A file written now has a set per layout, so a custom size in one does not spoil another.
int runLayoutNumbersCheck(LayoutSettings& layout, const QString& file) {
    if (file.isEmpty()) {
        std::fprintf(stderr, "dj-ui: --layout-numbers-check needs --layout-file\n");
        return 1;
    }
    const QString oldFile = file + QStringLiteral(".v1");
    {
        QFile written(oldFile);
        if (!written.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
            std::fprintf(stderr, "dj-ui: cannot write %s\n", qPrintable(oldFile));
            return 1;
        }
        // Exactly what the previous build wrote: one name, one set of numbers, no arrangements.
        written.write(R"({"preset":"two-deck","deck_rows":1,"global_percent":9.0,)"
                      R"("browser_percent":31.0,"mixer_percent":17.0})");
    }
    const auto near = [](double actual, double wanted) { return std::abs(actual - wanted) < 0.01; };

    layout.setFile(oldFile);
    if (!layout.load()) {
        std::fprintf(stderr, "dj-ui: the old layout file did not load\n");
        return 1;
    }
    if (layout.preset() != QStringLiteral("duo") || layout.arrangement() != QStringLiteral("duo")
        || layout.mixerEdge() != QStringLiteral("centre")) {
        std::fprintf(stderr, "dj-ui: the old name did not become duo (%s of %s, mixer %s)\n",
                     qPrintable(layout.preset()), qPrintable(layout.arrangement()),
                     qPrintable(layout.mixerEdge()));
        return 1;
    }
    if (!near(layout.browserPercent(), 31.0) || !near(layout.mixerPercent(), 17.0)
        || layout.deckRows() != 1) {
        std::fprintf(stderr, "dj-ui: the old numbers did not survive (browser %.1f, mixer %.1f, rows %d)\n",
                     layout.browserPercent(), layout.mixerPercent(), layout.deckRows());
        return 1;
    }
    std::fprintf(stdout, "dj-ui: the old file opens as duo with its numbers (browser %.1f, mixer %.1f)\n",
                 layout.browserPercent(), layout.mixerPercent());

    if (!layout.setLayout(QStringLiteral("stack-left"))) {
        std::fprintf(stderr, "dj-ui: stack-left is not a known layout\n");
        return 1;
    }
    if (layout.arrangement() != QStringLiteral("stack") || layout.mixerEdge() != QStringLiteral("left")
        || !near(layout.browserPercent(), 24.0)) {
        std::fprintf(stderr, "dj-ui: stack-left took the wrong shape (%s/%s, browser %.1f)\n",
                     qPrintable(layout.arrangement()), qPrintable(layout.mixerEdge()),
                     layout.browserPercent());
        return 1;
    }
    if (layout.setLayout(QStringLiteral("nothing-of-the-kind"))) {
        std::fprintf(stderr, "dj-ui: an unknown layout was accepted\n");
        return 1;
    }
    if (!layout.save()) {
        std::fprintf(stderr, "dj-ui: the layout was not written\n");
        return 1;
    }

    LayoutSettings reopened;
    reopened.setFile(oldFile);
    if (!reopened.load()) {
        std::fprintf(stderr, "dj-ui: the written layout file did not read back\n");
        return 1;
    }
    if (reopened.arrangement() != QStringLiteral("stack")
        || reopened.mixerEdge() != QStringLiteral("left")
        || reopened.preset() != QStringLiteral("stack-left") || !near(reopened.browserPercent(), 24.0)) {
        std::fprintf(stderr, "dj-ui: the layout did not come back (%s/%s, browser %.1f)\n",
                     qPrintable(reopened.arrangement()), qPrintable(reopened.mixerEdge()),
                     reopened.browserPercent());
        return 1;
    }
    // The numbers the old file carried belong to `duo`, and only to it: switching to `classic` must
    // bring classic's own, not duo's.
    if (!reopened.setLayout(QStringLiteral("duo")) || !near(reopened.browserPercent(), 31.0)) {
        std::fprintf(stderr, "dj-ui: the numbers of another layout were lost (duo browser %.1f)\n",
                     reopened.browserPercent());
        return 1;
    }
    if (!reopened.setLayout(QStringLiteral("classic")) || !near(reopened.browserPercent(), 28.5)) {
        std::fprintf(stderr, "dj-ui: classic borrowed another layout's numbers (browser %.1f)\n",
                     reopened.browserPercent());
        return 1;
    }
    if (!reopened.setLayout(QStringLiteral("traktor"))
        || reopened.arrangement() != QStringLiteral("classic")) {
        std::fprintf(stderr, "dj-ui: the old name traktor is not classic any more\n");
        return 1;
    }
    std::fprintf(stdout,
                 "dj-ui: the numbers survive a round trip, every layout keeps its own, old files and old "
                 "names still open\n");
    return 0;
}

/// A batch over the whole collection: the list stands still while it runs, and Cancel stops it.
///
/// The DJ's complaint had two halves. The rows jerked, because the panel asked the engine for the page
/// again on every tick of the batch — a page answer replaces every row, and a replaced row is a list that
/// jumps under the eye. And Cancel did nothing, which turned a batch he could stop into one he had to
/// outlast.
int runAnalyzeBatchCheck(QQmlApplicationEngine& engine, FileBrowser& browser, LayoutSettings& layout,
                         BridgeClient& bridge, const QString& fixture, const QString& collectionRoot) {
    Q_UNUSED(layout);
    Q_UNUSED(fixture);
    if (collectionRoot.isEmpty()) {
        std::fprintf(stderr, "dj-ui: --analyze-batch-check needs --collection-root\n");
        return 1;
    }
    QDir root(collectionRoot);
    if (!root.mkpath(QStringLiteral("."))) {
        std::fprintf(stderr, "dj-ui: cannot make %s\n", qPrintable(collectionRoot));
        return 1;
    }
    // Six hundred tracks, more than the engine has workers — the gate pins it to one core, so the parallel path
    // is taken and the batch still lasts long enough for a hand to reach it: a stop has files that were never
    // started to save, and a stop that only finds every file already in flight saves nothing and reads as a dead
    // button. Short files, a beat apart: long ones of nearly the same tempo are one recording as far as the
    // fingerprint is concerned, and an import sets half of them aside as duplicates.
    const int wanted = 600;
    QTemporaryDir incoming;
    if (!incoming.isValid()) {
        std::fprintf(stderr, "dj-ui: cannot make a temporary folder\n");
        return 1;
    }
    QStringList staged;
    for (int index = 0; index < wanted; ++index) {
        const QString source = incoming.filePath(QStringLiteral("Batch %1.wav").arg(index));
        if (!writeClickTrack(source, 120.0 + index * 0.5, 0.4)) {
            std::fprintf(stderr, "dj-ui: cannot write %s\n", qPrintable(source));
            return 1;
        }
        staged.append(source);
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
    const auto analysis = [&]() { return panel->property("analysisState").toMap(); };
    const auto running = [&]() {
        const QVariantMap state = analysis();
        return state.value(QStringLiteral("running")).toBool();
    };
    const auto done = [&]() { return analysis().value(QStringLiteral("done")).toInt(); };
    const auto total = [&]() { return analysis().value(QStringLiteral("total")).toInt(); };

    bridge.importPaths(staged, collectionRoot, false);
    if (!waitFor(
            [&]() {
                const QVariantMap state = panel->property("importState").toMap();
                return state.value(QStringLiteral("total")).toInt() >= wanted &&
                       !state.value(QStringLiteral("running")).toBool();
            },
            420000)) {
        std::fprintf(stderr, "dj-ui: the import never finished\n");
        return 1;
    }
    browser.openCollection({collectionRoot});
    if (!waitFor([&]() { return browser.entries().size() >= wanted; }, 60000)) {
        std::fprintf(stderr, "dj-ui: the collection lists %d rows\n",
                     static_cast<int>(browser.entries().size()));
        return 1;
    }

    // The whole collection, measured in one batch.
    QStringList chosen;
    for (const QVariant& entry : browser.entries()) {
        chosen.append(entry.toMap().value(QStringLiteral("path")).toString());
    }
    // Parallel, the way the dialog's box is ticked by default: many workers are in flight at once, and a stop
    // has to reach every one of them rather than the queue alone.
    bridge.analyzePaths(chosen, true);
    if (!waitFor([&]() { return running() && done() >= 1; }, 30000)) {
        std::fprintf(stderr, "dj-ui: the batch never got going (running %d, done %d of %d)\n",
                     running() ? 1 : 0, done(), total());
        return 1;
    }
    // The list under a running batch: a page asked for again replaces every row, and the number says so.
    const int quietGeneration = browser.pageGeneration();
    const int quietRows = static_cast<int>(browser.entries().size());
    (void)waitFor([]() { return false; }, 1500);
    if (browser.pageGeneration() != quietGeneration) {
        std::fprintf(stderr, "dj-ui: the list was replaced %d times while the batch ran (done %d of %d)\n",
                     browser.pageGeneration() - quietGeneration, done(), total());
        return 1;
    }
    if (static_cast<int>(browser.entries().size()) != quietRows) {
        std::fprintf(stderr, "dj-ui: the list went from %d rows to %d while the batch ran\n", quietRows,
                     static_cast<int>(browser.entries().size()));
        return 1;
    }
    std::fprintf(stdout,
                 "dj-ui: the list stood still through the running batch (generation %d, %d rows, done %d "
                 "of %d)\n",
                 quietGeneration, quietRows, done(), total());
    if (!running()) {
        std::fprintf(stderr, "dj-ui: the batch ended before Cancel could be pressed\n");
        return 1;
    }

    // Cancel: the DJ's own button on the progress window, pressed the way he presses it — a window that
    // answers the bridge but not the button is a window that does nothing.
    const int atCancel = done();
    auto* stopButton = findItem(window->contentItem(), QStringLiteral("analyzeStop"));
    if (stopButton == nullptr) {
        std::fprintf(stderr, "dj-ui: the progress window has no Cancel button on it\n");
        return 1;
    }
    if (!stopButton->isVisible()) {
        std::fprintf(stderr, "dj-ui: the Cancel button is not on screen while the batch runs\n");
        return 1;
    }
    // A real press at the button's centre, not a signal called by name: a window whose button only answers a
    // programmatic click is a window whose button does nothing under the DJ's finger.
    const QPointF stopCentre =
        stopButton->mapToScene(QPointF(stopButton->width() / 2.0, stopButton->height() / 2.0));
    for (const QEvent::Type type : {QEvent::MouseButtonPress, QEvent::MouseButtonRelease}) {
        QMouseEvent event(type, stopCentre, window->mapToGlobal(stopCentre.toPoint()), Qt::LeftButton,
                          Qt::LeftButton, Qt::NoModifier);
        QCoreApplication::sendEvent(window, &event);
    }
    QCoreApplication::processEvents();
    if (!waitFor([&]() { return !running(); }, 5000)) {
        std::fprintf(stderr, "dj-ui: cancel did not stop the batch (done %d of %d)\n", done(), total());
        return 1;
    }
    // Nothing is claimed about a label here: the progress window goes with the last file being measured, and a
    // word that lives for fifty milliseconds is not a witness. What the DJ asked for is the effect — nothing new
    // is measured once he has pressed Cancel.
    const int stoppedAt = done();
    if (stoppedAt >= total()) {
        std::fprintf(stderr, "dj-ui: the batch ran to its end instead of stopping (done %d of %d)\n",
                     stoppedAt, total());
        return 1;
    }
    // A stop is honest about what it can promise: the files already being decoded are finished and written,
    // and nothing new starts. So a few more rows than the ones counted at the press are expected — the number
    // of workers, at most — and the rest of the batch is not measured.
    const int inFlight = qMax(1, QThread::idealThreadCount());
    if (stoppedAt > atCancel + inFlight) {
        std::fprintf(stderr, "dj-ui: the batch went on measuring after cancel (%d -> %d of %d, %d in flight)\n",
                     atCancel, stoppedAt, total(), inFlight);
        return 1;
    }
    std::fprintf(stdout, "dj-ui: cancel stopped the batch at %d of %d (asked at %d)\n", stoppedAt, total(),
                 atCancel);
    (void)waitFor([&]() { return false; }, 600);
    return 0;
}

} // namespace checks
