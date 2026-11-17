// Общие помощники проверок: то, чем проверка смотрит на окно и на снапшот.

#include "Checks.h"

namespace checks {

// QML items created by a Repeater are not always in the window's QObject tree, so a search by name
// has to walk the visual tree — the one the user actually sees.
QQuickItem *findItem(QQuickItem *root, const QString &name)
{
    if (root == nullptr) {
        return nullptr;
    }
    if (root->objectName() == name) {
        return root;
    }
    const QList<QQuickItem *> children = root->childItems();
    for (QQuickItem *child : children) {
        if (QQuickItem *found = findItem(child, name)) {
            return found;
        }
    }
    return nullptr;
}

int measureCollectionHeader(QQuickItem *panel)
{
    QQuickItem *headerRow = findItem(panel, QStringLiteral("browserHeaderRow"));
    QQuickItem *countLabel = findItem(panel, QStringLiteral("collectionCount"));
    QQuickItem *minusButton = findItem(panel, QStringLiteral("fontMinus"));
    QQuickItem *lastDeckButton = findItem(panel, QStringLiteral("deckButton_D"));
    if (headerRow == nullptr || countLabel == nullptr || minusButton == nullptr
        || lastDeckButton == nullptr) {
        std::fprintf(stderr, "dj-ui: the collection header has no row, count or buttons to measure\n");
        return 1;
    }
    QCoreApplication::processEvents();
    const auto sceneRect = [](QQuickItem *item) {
        return item->mapRectToScene(QRectF(0, 0, item->width(), item->height()));
    };
    const QRectF rowRect = sceneRect(headerRow);
    const QRectF countRect = sceneRect(countLabel);
    const QRectF minusRect = sceneRect(minusButton);
    const QRectF deckRect = sceneRect(lastDeckButton);
    const double gap = minusRect.left() - countRect.right();
    const double slack = rowRect.right() - deckRect.right();
    std::fprintf(stdout,
                 "dj-ui: the collection header: controls %.0f px right of the count, %.1f px from the "
                 "right edge\n",
                 gap, slack);
    if (gap < 8.0) {
        std::fprintf(stderr, "dj-ui: the type and deck buttons crowd the collection count (%.0f px)\n",
                     gap);
        return 1;
    }
    if (slack < -0.5 || slack > 2.0) {
        std::fprintf(stderr, "dj-ui: the browser's controls are not at the right edge (%.1f px)\n",
                     slack);
        return 1;
    }
    return 0;
}

int measureDeckRows(QQuickWindow *window, const QString &deck)
{
    // A roomy window, always: at a small one the wave is already at its floor, a folded row changes
    // nothing, and the measurement would be about the floor rather than about the arrangement.
    window->resize(1600, 1000);
    window->show();
    QElapsedTimer settling;
    settling.start();
    while (settling.elapsed() < 800) {
        QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
        QThread::msleep(10);
    }
    const auto rectOf = [&](const QString &name) {
        QQuickItem *item = findItem(window->contentItem(), name);
        if (item == nullptr || !item->isVisible()) {
            return QRectF();
        }
        return item->mapRectToScene(QRectF(0, 0, item->width(), item->height()));
    };
    // The layout child, not the strip drawn inside it: what has to give the height back is the item the
    // column lays out (`deckDetail_`), and an inner canvas can have a height of its own.
    const auto waveHeight = [&]() { return rectOf(QStringLiteral("deckDetail_") + deck).height(); };
    const QRectF panel = rectOf(QStringLiteral("deckPanel_") + deck);
    const QRectF wave = rectOf(QStringLiteral("deckDetail_") + deck);
    const QRectF transport = rectOf(QStringLiteral("transport_") + deck);
    if (panel.isEmpty() || wave.isEmpty() || transport.isEmpty()) {
        std::fprintf(stderr, "dj-ui: the deck %s has no panel, no wave or no transport row\n",
                     qPrintable(deck));
        return 1;
    }
    // The strip folds, and what it costs is the wave's room: the measurement opens it, folds it and
    // requires the wave to take the row's height back. Whatever state it is found in, it ends the same
    // way it was found, so the check does not decide for the next one.
    const bool startedOpen = !rectOf(QStringLiteral("jumpGroup_") + deck).isEmpty();
    auto *toggle = findItem(window->contentItem(), QStringLiteral("loopJumpToggle_") + deck);
    if (toggle == nullptr) {
        std::fprintf(stderr, "dj-ui: the deck %s has no button for the folding strip\n",
                     qPrintable(deck));
        return 1;
    }
    const auto pressToggle = [&]() {
        QMetaObject::invokeMethod(toggle, "clicked");
        // The layout re-polishes on the next frame: a row that has just been shown still carries the
        // position it had while it was hidden, and reading the geometry before that pass measures nothing.
        QElapsedTimer settling;
        settling.start();
        while (settling.elapsed() < 700) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(10);
        }
    };
    if (!startedOpen) {
        pressToggle();
    }
    const QRectF stripRow = rectOf(QStringLiteral("loopsize_") + deck);
    const QRectF stripLabel = rectOf(QStringLiteral("stripLabel_") + deck);
    const QRectF jumps = rectOf(QStringLiteral("jumpGroup_") + deck);
    if (stripRow.isEmpty() || stripLabel.isEmpty() || jumps.isEmpty()) {
        std::fprintf(stderr, "dj-ui: the strip of the deck %s did not open\n", qPrintable(deck));
        return 1;
    }
    const double openWave = waveHeight();
    std::fprintf(stdout,
                 "dj-ui: the strip open: a row of %.0f px, a wave of %.0f px, the loop sizes start %.0f px "
                 "from the row's left edge and the beatjumps end %.0f px from its right one\n",
                 stripRow.height(), openWave, stripLabel.left() - stripRow.left(),
                 stripRow.right() - jumps.right());
    if (stripLabel.left() - stripRow.left() > 8.0) {
        std::fprintf(stderr, "dj-ui: the loop sizes start %.0f px from the row's left edge\n",
                     stripLabel.left() - stripRow.left());
        return 1;
    }
    if (stripRow.right() - jumps.right() > 8.0) {
        std::fprintf(stderr, "dj-ui: the beatjumps end %.0f px from the row's right edge\n",
                     stripRow.right() - jumps.right());
        return 1;
    }
    pressToggle();
    if (!rectOf(QStringLiteral("jumpGroup_") + deck).isEmpty()) {
        std::fprintf(stderr, "dj-ui: the strip did not fold\n");
        return 1;
    }
    const double shutWave = waveHeight();
    const double given = shutWave - openWave;
    std::fprintf(stdout,
                 "dj-ui: the strip folded: the wave went from %.0f to %.0f px, %.0f px for a row of "
                 "%.0f px\n",
                 openWave, shutWave, given, stripRow.height());
    if (given < stripRow.height() - 2.0 || given > stripRow.height() + 12.0) {
        std::fprintf(stderr, "dj-ui: the wave gave %.0f px for a row of %.0f px\n", given,
                     stripRow.height());
        return 1;
    }
    const auto where = [&](const QString &name) {
        const QRectF r = rectOf(name);
        return r.isEmpty() ? QStringLiteral("no")
                           : QStringLiteral("%1..%2")
                                 .arg(r.top() - panel.top(), 0, 'f', 0)
                                 .arg(r.bottom() - panel.top(), 0, 'f', 0);
    };
    std::fprintf(stdout,
                 "dj-ui: in panel coordinates (%.0f px tall): wave %s, transport %s, grid %s, strip %s\n",
                 panel.height(), qPrintable(where(QStringLiteral("deckDetail_") + deck)),
                 qPrintable(where(QStringLiteral("transport_") + deck)),
                 qPrintable(where(QStringLiteral("gridedit_") + deck)),
                 qPrintable(where(QStringLiteral("loopsize_") + deck)));
    // Back to the state it was found in: the strip was open when the check reached this point, it is
    // open again, and the next check sees what this one saw.
    if (startedOpen) {
        pressToggle();
    }
    std::fprintf(stdout, "dj-ui: the strip is %s again, as the check found it, the wave has %.0f px\n",
                 startedOpen ? "open" : "folded", waveHeight());
    // The grid tools are centred on the deck: a row that is full width and a block of buttons pushed to
    // the left edge leaves the middle of the panel empty (owner's decision).
    // Play and Cue are one pair: one width for the two, taken from what the pair would take on its own,
    // so the row keeps its room and neither button reads as a bigger control than the other.
    auto *playButton = findItem(window->contentItem(), QStringLiteral("play_") + deck);
    auto *cueButton = findItem(window->contentItem(), QStringLiteral("cue_") + deck);
    if (playButton == nullptr || cueButton == nullptr) {
        std::fprintf(stderr, "dj-ui: the deck %s has no Play or no Cue button\n", qPrintable(deck));
        return 1;
    }
    std::fprintf(stdout,
                 "dj-ui: Play is %.0f px wide and Cue is %.0f px wide, %.0f px between them\n",
                 playButton->width(), cueButton->width(), std::abs(playButton->width() - cueButton->width()));
    if (std::abs(playButton->width() - cueButton->width()) > 0.5) {
        std::fprintf(stderr, "dj-ui: Play and Cue are not one width (%.0f against %.0f)\n",
                     playButton->width(), cueButton->width());
        return 1;
    }
    const QRectF gridRow = rectOf(QStringLiteral("gridedit_") + deck);
    const QRectF gridBlock = rectOf(QStringLiteral("gridBlock_") + deck);
    if (gridRow.isEmpty() || gridBlock.isEmpty()) {
        std::fprintf(stderr, "dj-ui: the deck %s has no grid row to centre\n", qPrintable(deck));
        return 1;
    }
    const double off = std::abs(gridBlock.center().x() - gridRow.center().x());
    std::fprintf(stdout,
                 "dj-ui: the grid tools are %.0f px wide and stand %.0f px off the middle of their row\n",
                 gridBlock.width(), off);
    if (off > 2.0) {
        std::fprintf(stderr, "dj-ui: the grid tools stand %.0f px off the middle of the deck\n", off);
        return 1;
    }
    return 0;
}

// layoutCheckOption: one check of the shell, out of main.
//
// Split out of main: a function of ten thousand lines loses its debug information, and GCC
// says so (`variable tracking size limit exceeded`). The checks are the tests; a named
// function is where a failure is read.

bool writeClickTrack(const QString &path, double bpm, double seconds)
{
    const int rate = 48000;
    const int frames = int(seconds * rate);
    const int clickFrames = rate / 100; // 10 ms: an onset, not a note
    QByteArray audio(size_t(frames) * 2, '\0');
    const double beatFrames = 60.0 / bpm * rate;
    // Noise, not a tone: a tempo detector votes on spectral flux, and a burst with energy across the
    // spectrum is what it hears as a beat. A square wave only moves two partials and reads as nothing.
    quint32 noise = 0x9e3779b9u;
    for (double beat = 0.0; beat + clickFrames < frames; beat += beatFrames) {
        const int start = int(beat);
        for (int at = 0; at < clickFrames; ++at) {
            noise = noise * 1664525u + 1013904223u;
            const double decay = 1.0 - double(at) / double(clickFrames);
            const double sample = (double((noise >> 9) & 0x7fff) / 16384.0 - 1.0) * decay;
            const short value = short(12000.0 * sample);
            audio[start * 2 + at * 2] = char(value & 0xff);
            audio[start * 2 + at * 2 + 1] = char((value >> 8) & 0xff);
        }
    }
    QFile file(path);
    if (!file.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
        return false;
    }
    const auto put32 = [&file](quint32 value) {
        char bytes[4]{char(value & 0xff), char((value >> 8) & 0xff), char((value >> 16) & 0xff),
                      char((value >> 24) & 0xff)};
        file.write(bytes, 4);
    };
    const auto put16 = [&file](quint16 value) {
        char bytes[2]{char(value & 0xff), char((value >> 8) & 0xff)};
        file.write(bytes, 2);
    };
    file.write("RIFF", 4);
    put32(quint32(36 + audio.size()));
    file.write("WAVEfmt ", 8);
    put32(16);
    put16(1);          // pcm
    put16(1);          // mono
    put32(quint32(rate));
    put32(quint32(rate * 2));
    put16(2);
    put16(16);
    file.write("data", 4);
    put32(quint32(audio.size()));
    file.write(audio);
    return file.error() == QFileDevice::NoError;
}


QVariantMap deckById(const QVariantMap &snapshot, const QString &deck)
{
    const QVariantList decks = snapshot.value(QStringLiteral("decks")).toList();
    for (const QVariant &entry : decks) {
        const QVariantMap deckState = entry.toMap();
        if (deckState.value(QStringLiteral("deck")).toString() == deck) {
            return deckState;
        }
    }
    return {};
}

bool deckHasHotcue(const QVariantMap &snapshot, const QString &deck, int slot)
{
    const QVariantList decks = snapshot.value(QStringLiteral("decks")).toList();
    for (const QVariant &entry : decks) {
        const QVariantMap deckState = entry.toMap();
        if (deckState.value(QStringLiteral("deck")).toString() != deck) {
            continue;
        }
        const QVariantList cues = deckState.value(QStringLiteral("hotcues")).toList();
        if (slot >= cues.size()) {
            return false;
        }
        return !cues.at(slot).isNull() && cues.at(slot).isValid();
    }
    return false;
}

bool deckIsPlaying(const QVariantMap &snapshot, const QString &deck)
{
    const QVariant decks = snapshot.value(QStringLiteral("decks"));
    if (!decks.isValid()) {
        return false;
    }
    const QVariantList list = decks.toList();
    for (const QVariant &entry : list) {
        const QVariantMap deckState = entry.toMap();
        if (deckState.value(QStringLiteral("deck")).toString() == deck) {
            return deckState.value(QStringLiteral("playing")).toBool();
        }
    }
    return false;
}

} // namespace checks
