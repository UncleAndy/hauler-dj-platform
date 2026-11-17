#include "BridgeClient.h"
#include "Scenarios.h"
#include "UiSettings.h"
#include "LayoutSettings.h"
#include "SessionState.h"
#include "Waveform.h"
#include "WaveformItem.h"
#include "WaveformPainter.h"

#include <QCommandLineOption>
#include <QCommandLineParser>
#include <QElapsedTimer>
#include <QFile>
#include <QFileInfo>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QKeyEvent>
#include <QWheelEvent>
#include <QEventLoop>
#include <QGuiApplication>
#include <QPalette>
#include <QDir>
#include <QDirIterator>
#include <QThread>
#include <QMap>
#include <QTemporaryDir>
#include <QImage>
#include <algorithm>
#include <QQuickWindow>
#include <QLinearGradient>
#include <QPainter>
#include <QQmlApplicationEngine>
#include <QQmlComponent>
#include <QQmlEngine>
#include <QQmlContext>
#include <QString>
#include <QQuickItemGrabResult>
#include <QSharedPointer>
#include <QTimer>

#include <cstdio>
#include <functional>
#include <memory>

namespace {

int g_warningCount = 0;

void collectMessages(QtMsgType type, const QMessageLogContext &context, const QString &message)
{
    if (type != QtDebugMsg && type != QtInfoMsg) {
        ++g_warningCount;
    }
    const char *category = context.category != nullptr ? context.category : "default";
    std::fprintf(stderr, "qt.%s: %s\n", category, qPrintable(message));
}

QString defaultSocketPath()
{
    const QByteArray runtime = qgetenv("XDG_RUNTIME_DIR");
    if (!runtime.isEmpty()) {
        return QString::fromLocal8Bit(runtime) + QStringLiteral("/hauler-dj-engine.sock");
    }
    return QStringLiteral("/tmp/hauler-dj-engine.sock");
}

} // namespace

#include "checks/Checks.h"

using namespace checks;

int main(int argc, char *argv[])
{
    qInstallMessageHandler(collectMessages);

    QGuiApplication app(argc, argv);

    // Controls follow the application palette, and the palette is the measured Traktor scheme:
    // neutral dark greys with one blue accent. Without this the shell's own background is dark
    // while every control stays light, which is exactly wrong for a booth.
    // The Controls style reads the palette; Fusion is the one that honours it on every platform.
    qputenv("QT_QUICK_CONTROLS_STYLE", "Fusion");
    QPalette palette;
    palette.setColor(QPalette::Window, QColor(QStringLiteral("#181818")));
    palette.setColor(QPalette::WindowText, QColor(QStringLiteral("#c8c8c8")));
    palette.setColor(QPalette::Base, QColor(QStringLiteral("#101010")));
    palette.setColor(QPalette::AlternateBase, QColor(QStringLiteral("#202020")));
    palette.setColor(QPalette::Text, QColor(QStringLiteral("#c8c8c8")));
    palette.setColor(QPalette::Button, QColor(QStringLiteral("#282828")));
    palette.setColor(QPalette::ButtonText, QColor(QStringLiteral("#c8c8c8")));
    palette.setColor(QPalette::BrightText, QColor(QStringLiteral("#f0f3f6")));
    palette.setColor(QPalette::Highlight, QColor(QStringLiteral("#5090f0")));
    palette.setColor(QPalette::HighlightedText, QColor(QStringLiteral("#0b0b0b")));
    palette.setColor(QPalette::ToolTipBase, QColor(QStringLiteral("#202020")));
    palette.setColor(QPalette::ToolTipText, QColor(QStringLiteral("#c8c8c8")));
    palette.setColor(QPalette::PlaceholderText, QColor(QStringLiteral("#8a8a8a")));
    palette.setColor(QPalette::Disabled, QPalette::Text, QColor(QStringLiteral("#6a6a6a")));
    palette.setColor(QPalette::Disabled, QPalette::ButtonText, QColor(QStringLiteral("#6a6a6a")));
    QGuiApplication::setPalette(palette);

    QGuiApplication::setPalette(palette);

    QCoreApplication::setApplicationName(QStringLiteral("dj-platform"));
    QCoreApplication::setApplicationVersion(QStringLiteral("0.1.0"));
    QCoreApplication::setOrganizationName(QStringLiteral("DJ Platform"));

    QCommandLineParser parser;
    parser.setApplicationDescription(
        QStringLiteral("DJ Platform UI shell: renders the engine snapshot, forwards commands."));
    parser.addHelpOption();
    parser.addVersionOption();
    const QCommandLineOption smokeOption(
        QStringLiteral("smoke"),
        QStringLiteral("Load the QML scene, report Qt warnings, then exit without a window."));
    const QCommandLineOption connectOption(
        QStringLiteral("connect"),
        QStringLiteral("Engine socket to connect to (default: %1).").arg(defaultSocketPath()),
        QStringLiteral("path"));
    const QCommandLineOption stripCheckOption(
        QStringLiteral("strip-check"),
        QStringLiteral("With --connect and --load: load the track into deck A and require the deck's "
                       "strip to receive the envelope the engine published."));
    const QCommandLineOption keyCheckOption(
        QStringLiteral("key-check"),
        QStringLiteral("With --connect and --load: require the deck's header to show the track's key "
                       "and the engine to have written the same key beside the file."));
    const QCommandLineOption exitAfterOption(
        QStringLiteral("exit-after"),
        QStringLiteral("Close the window after this many milliseconds: the shutdown path, exercised "
                       "instead of described."),
        QStringLiteral("ms"));
    const QCommandLineOption gridReconnectOption(
        QStringLiteral("grid-reconnect-check"),
        QStringLiteral("With --connect only: the deck is already loaded, as it is for a client that "
                       "reconnects to a running engine, and every grid button must still work."));
    const QCommandLineOption metronomeOption(
        QStringLiteral("metronome-check"),
        QStringLiteral("With --connect and --load: require the deck's metronome button to reach the "
                       "engine, the state to come back, and the click to be played on the beats."));
    const QCommandLineOption scratchOption(
        QStringLiteral("scratch-check"),
        QStringLiteral("With --connect and --load: require a drag on the waveform to take the record in "
                       "hand — the transport stops, the material moves the way the hand goes, a still hand "
                       "stops it, and letting go gives the record back to the motor."));
    const QCommandLineOption tapOption(
        QStringLiteral("tap-check"),
        QStringLiteral("With --connect and --load: require the tapped grid to be put on the taps — the "
                       "tempo from their slope and a beat on the line through them."));
    const QCommandLineOption gridLinesOption(
        QStringLiteral("grid-lines-check"),
        QStringLiteral("With --connect and --load: require the strip to draw the beat lines of the grid, "
                       "with a bar line every fourth beat, and the spacing to follow the tempo."));
    const QCommandLineOption gridCreateOption(
        QStringLiteral("grid-create-check"),
        QStringLiteral("With --connect and --load: on a track whose tempo could not be measured, the "
                       "grid tools must be there, the ones that read a grid must be grey, and placing a "
                       "beat or typing a tempo must make the grid."));
    const QCommandLineOption gridCheckOption(
        QStringLiteral("grid-check"),
        QStringLiteral("With --connect and --load: move the grid, halve it, double it and type a tempo, "
                       "requiring the deck and the file to follow every edit."));
    const QCommandLineOption loopBeatsOption(
        QStringLiteral("loop-beats-check"),
        QStringLiteral("Check that a loop of four beats is four beats of the grid, and that the arrows "
                       "move it by a beat without shortening it (DJP-DECK-005)."));
    const QCommandLineOption fourDeckOption(
        QStringLiteral("four-deck-check"),
        QStringLiteral("With --connect and --load: put the same file on four decks, play all of them, and "
                       "require each to move, each to reach the master, the four panels to exist, and the "
                       "crossfader to hold A and C on one side and B and D on the other."));
    const QCommandLineOption syncSoakOption(
        QStringLiteral("sync-soak"),
        QStringLiteral("With --connect: two decks on click tracks of known tempo, ten minutes under the "
                       "null sink, phase read from the snapshots, with the master's tempo changed, the "
                       "follower paused and started, a loop on and off, and a tempo the fader cannot "
                       "reach (DJP-SYNC-001/002, AC-SYNC)."));
    const QCommandLineOption soakSecondsOption(
        QStringLiteral("soak-seconds"),
        QStringLiteral("How long the soak runs, in seconds (default 600)."),
        QStringLiteral("seconds"),
        QStringLiteral("600"));
    const QCommandLineOption syncCheckOption(
        QStringLiteral("sync-check"),
        QStringLiteral("Load two decks, make one the master through its button, put the other on its "
                       "beat, and require the state and the phase meter to agree with the engine."));
    const QCommandLineOption transportExtrasOption(
        QStringLiteral("transport-extras-check"),
        QStringLiteral("With --connect and --load: press a jump and require the play head on a beat of "
                       "the grid, and switch slip and reverse on and off (DJP-DECK-006/007)."));
    const QCommandLineOption gridHereOption(
        QStringLiteral("grid-here-check"),
        QStringLiteral("With --connect and --load: seek off the grid, press the button between the arrows, "
                       "and require a beat of the grid to land on the play head — and to survive a reload."));
    const QCommandLineOption tempoRecheckOption(
        QStringLiteral("tempo-recheck"),
        QStringLiteral("With --connect and --load: require the deck's re-measure button to ask before "
                       "it runs, then confirm it and require the new grid on disk."));
    const QCommandLineOption cueWriteOption(
        QStringLiteral("cue-write-check"),
        QStringLiteral("With --connect and --load: seek, store hotcue 2 and set a loop, then require the "
                       "snapshot to show them. The pair of checks proves a reload brings them back."));
    const QCommandLineOption cueReadOption(
        QStringLiteral("cue-read-check"),
        QStringLiteral("With --connect and --load: expect hotcue 2 at 48000 and a loop 96000..144000 "
                       "from the file alone, sending no cue commands."));
    const QCommandLineOption masterCheckOption(
        QStringLiteral("master-check"),
        QStringLiteral("With --connect: the master knob must change the published gain, the master "
                       "meter must see the signal, and the knob must sit above a shorter meter."));
    const QCommandLineOption curveCheckOption(
        QStringLiteral("curve-check"),
        QStringLiteral("With --connect: send each crossfader law and require the snapshot to report "
                       "it back."));
    const QCommandLineOption pageCheckOption(
        QStringLiteral("collection-page-check"),
        QStringLiteral("Compare the index page with the walk over the collection, field by field."));
    const QCommandLineOption scrollCheckOption(
        QStringLiteral("scroll-check"),
        QStringLiteral("The list keeps its place when the page is replaced (needs --collection-root)."));
    const QCommandLineOption importCheckOption(
        QStringLiteral("import-check"),
        QStringLiteral("With --connect: pick two rows in the browser, add them to the collection, "
                       "answer the duplicate question and require the files to be there."));
    const QCommandLineOption sortCheckOption(
        QStringLiteral("sort-check"),
        QStringLiteral("From the collection: a click on a column sorts by it, a second click turns it "
                       "around, a Shift click adds the next key — and the rail's order."));
    const QCommandLineOption endCheckOption(
        QStringLiteral("end-check"),
        QStringLiteral("Play a deck to its last frame and require it to stand still and take a track again."));
    const QCommandLineOption lockCheckOption(
        QStringLiteral("lock-check"),
        QStringLiteral("Press the grid lock on a deck and require the collection's row to wear it at once, "
                       "then press again and require it gone."));
    const QCommandLineOption genreCheckOption(
        QStringLiteral("genre-check"),
        QStringLiteral("From a collection: the genre chips say how many tracks each genre has, a chip "
                       "narrows the list, two chips are a union, and reset brings everything back."));
    const QCommandLineOption collectionProbeOption(
        QStringLiteral("collection-probe"),
        QStringLiteral("Time the listing of a collection root: how many rows, and how long the walk "
                       "takes cold and warm. Read only."));
    const QCommandLineOption collectionLoadCheckOption(
        QStringLiteral("collection-load-check"),
        QStringLiteral("From a real collection, read only: choose a row and load it into deck A — the "
                       "engine must report the track in the deck, not only the panel its message."));
    const QCommandLineOption analyzeCheckOption(
        QStringLiteral("analyze-check"),
        QStringLiteral("From the collection: Analyze… on one track and on three — a progress window, "
                       "the parallel box, and the numbers in the rows afterwards."));
    const QCommandLineOption importFixtureOption(
        QStringLiteral("import-fixture"),
        QStringLiteral("A real track the import check copies twice."), QStringLiteral("path"));
    const QCommandLineOption windowCheckOption(
        QStringLiteral("window-check"),
        QStringLiteral("With --session-file: restore the remembered window shape, toggle it with F11 "
                       "and require the file to follow."));
    const QCommandLineOption gainCurveOption(
        QStringLiteral("gain-curve"),
        QStringLiteral("Law of the level knobs: linear (straight through) or cubic (soft around unity, "
                       "the default)."),
        QStringLiteral("kind"));
    const QCommandLineOption gainCurveMiddleOption(
        QStringLiteral("gain-curve-middle"),
        QStringLiteral("How much of the straight rate a level knob keeps at unity, 0.05 to 1 "
                       "(1 is straight through; the default is 0.1)."),
        QStringLiteral("share"));
    const QCommandLineOption faderCurveOption(
        QStringLiteral("fader-curve"),
        QStringLiteral("Law of the channel faders: linear, cubic (soft around the top) or logarithmic "
                       "(B-taper, the default: a quarter of the throw is about ten decibels)."),
        QStringLiteral("kind"));
    const QCommandLineOption faderCurveMiddleOption(
        QStringLiteral("fader-curve-middle"),
        QStringLiteral("The same setting for the channel faders, which are ridden rather than set, "
                       "0.05 to 1 (1 is straight through; the default is 0.5)."),
        QStringLiteral("share"));
    const QCommandLineOption gainCurveCheckOption(
        QStringLiteral("gain-curve-check"),
        QStringLiteral("Measure the level knobs' law at unity and at the end of their travel and "
                       "require the configured curve to be in force."));
    const QCommandLineOption tempoCheckOption(
        QStringLiteral("tempo-check"),
        QStringLiteral("With --connect and --load: require the engine to measure a tempo and the "
                       "deck to show it."));
    const QCommandLineOption eqCheckOption(
        QStringLiteral("eq-check"),
        QStringLiteral("With --connect: send three band gains to one deck and require the snapshot "
                       "to apply them and the strip's knobs to show those numbers."));
    const QCommandLineOption loopOption(
        QStringLiteral("loop-check"),
        QStringLiteral("With --connect and --load: press Loop twice and require the engine to hold a "
                       "loop between the two playhead positions, then require shift on the second "
                       "press to cancel the pair."));
    const QCommandLineOption scenarioOption(
        QStringLiteral("scenario-check"),
        QStringLiteral("With --connect and --load: drive the loops and cue points through the widgets "
                       "and require every user story to hold. Prints one line per case."));
    const QCommandLineOption padsOption(
        QStringLiteral("pads-check"),
        QStringLiteral("With --connect and --load: click an empty pad to set it, name and colour it, "
                       "jump to it by clicking again and clear it with shift, requiring the engine to "
                       "report each step."));
    const QCommandLineOption transportOption(
        QStringLiteral("transport-check"),
        QStringLiteral("With --connect and --load: play the deck, bring the head back to the cue point "
                       "with a tap of Cue, set the point by holding the same button, scrub the "
                       "overview with the hand, and require the clock to show the time left rather "
                       "than the length."));
    const QCommandLineOption playlistCheckOption(
        QStringLiteral("playlist-check"),
        QStringLiteral("With --connect, --import-fixture, a second fixture and --collection-root: make "
                       "a playlist in a folder, add tracks, remove the dups, drag a row, rename, move "
                       "and delete it, requiring the tree, the rows and the mirror file to agree."));
    const QCommandLineOption historyCheckOption(
        QStringLiteral("history-check"),
        QStringLiteral("With --connect and --load: play the deck below and above the engine's "
                       "--history-min-seconds, once with a pause in the middle, and require the "
                       "journal in the snapshot to hold exactly the plays that count."));
    const QCommandLineOption pitchOption(
        QStringLiteral("pitch-check"),
        QStringLiteral("With --connect: drag a deck's pitch fader and require the engine to play at "
                       "the rate the hand asked for, then double click it back to the file's own "
                       "speed."));
    const QCommandLineOption neutralOption(
        QStringLiteral("neutral-check"),
        QStringLiteral("With --connect: seat every mouse-driven control away from its neutral "
                       "position, double click it, and require the engine to report the neutral "
                       "value."));
    const QCommandLineOption wheelCheckOption(
        QStringLiteral("wheel-check"),
        QStringLiteral("With --connect: scroll over a channel knob and require the deck's gain to "
                       "change in the engine."));
    const QCommandLineOption spaceCheckOption(
        QStringLiteral("space-check"),
        QStringLiteral("With --connect and --load: prove Space no longer touches playback and folds "
                       "the browser instead."));
    const QCommandLineOption refusalCheckOption(
        QStringLiteral("refusal-check"),
        QStringLiteral("With --connect and --load: play deck A, ask the shell to load into it, and "
                       "require the deck's buttons to signal the refusal."));
    const QCommandLineOption busyCheckOption(
        QStringLiteral("load-busy-check"),
        QStringLiteral("With --connect and --load: prove a playing deck refuses a load (Busy) and "
                       "accepts it once stopped."));
    const QCommandLineOption gainCheckOption(
        QStringLiteral("gain-check"),
        QStringLiteral("With --connect: send a channel gain and require the snapshot to carry it "
                       "for that deck and no other."));
    const QCommandLineOption selfCheckOption(
        QStringLiteral("self-check"),
        QStringLiteral("Connect, press Play, require the ack and the snapshot, then exit."));
    const QCommandLineOption dialogCheckOption(
        QStringLiteral("dialog-check"),
        QStringLiteral("Check the in-app file browser without a display: the QML module loads and "
                       "lists a directory."));
    const QCommandLineOption outsideColorOption(
        QStringLiteral("waveform-outside-color"),
        QStringLiteral("Colour of the regions before the start and after the end of a track "
                       "(default: #101010, the window background)."),
        QStringLiteral("colour"));
    const QCommandLineOption scrubOption(
        QStringLiteral("scrub-mode"),
        QStringLiteral("audible (a hand on the strip is heard as a scratch) or silent (the default)."),
        QStringLiteral("mode"));
    const QCommandLineOption styleOption(
        QStringLiteral("waveform-style"),
        QStringLiteral("coloured (default: bass warm, treble cold) or plain."),
        QStringLiteral("style"));
    const QCommandLineOption barWidthOption(
        QStringLiteral("waveform-bar-width"),
        QStringLiteral("Width of one waveform bar in pixels (default 1: one bar per pixel)."),
        QStringLiteral("px"));
    const QCommandLineOption barGapOption(
        QStringLiteral("waveform-bar-gap"),
        QStringLiteral("Space between bars in pixels (default 0: a continuous waveform)."),
        QStringLiteral("px"));
    const QCommandLineOption bandGainOption(
        QStringLiteral("waveform-band-gain"),
        QStringLiteral("Trim the three bands before colouring, comma separated "
                       "(default 1,1,1). Raise the low gain to see more bass colour."),
        QStringLiteral("low,mid,high"));
    const QCommandLineOption layoutOption(
        QStringLiteral("layout"),
        QStringLiteral("Arrange the shell like a preset: traktor (default, measured from the "
                       "reference) or two-deck."),
        QStringLiteral("preset"));
    const QCommandLineOption layoutFileOption(
        QStringLiteral("layout-file"),
        QStringLiteral("Where the layout is remembered (default $XDG_CONFIG_HOME/hauler-dj/"
                       "layout.json). A preset on the command line overrides the file."),
        QStringLiteral("path"));
    const QCommandLineOption sessionFileOption(
        QStringLiteral("session-file"),
        QStringLiteral("Where the shell remembers the last folder it loaded from (default "
                       "$XDG_CONFIG_HOME/hauler-dj/session.json)."),
        QStringLiteral("path"));
    const QCommandLineOption channelOrderOption(
        QStringLiteral("channel-order"),
        QStringLiteral("Order of the mixer strips, left to right (default CABD). A permutation of "
                       "A, B, C and D."),
        QStringLiteral("ABCD"));
    const QCommandLineOption collectionRootOption(
        QStringLiteral("collection-root"),
        QStringLiteral("A folder the browser's Collection section lists. Repeat it for a collection "
                       "spread over several disks: every root is listed in one list, each with its own "
                       "metadata. Remembered for the next start; without it the collection lives in "
                       "$XDG_DATA_HOME/hauler-dj/collection."),
        QStringLiteral("path"));
    const QCommandLineOption layoutCheckOption(
        QStringLiteral("layout-check"),
        QStringLiteral("Load the shell, print each region's share of the window and fail when it "
                       "differs from the layout by more than two points."));
    const QCommandLineOption contrastOption(
        QStringLiteral("waveform-contrast"),
        QStringLiteral("How much the strip separates loud from quiet: the height is raised to this "
                       "power (default 1.4; 0.5..3). 1.0 draws the amplitudes as they are."),
        QStringLiteral("power"));
    const QCommandLineOption zoomOption(
        QStringLiteral("waveform-zoom"),
        QStringLiteral("Zoom the strip starts at, as a factor (default 10; 1..100). A double click "
                       "returns to it."),
        QStringLiteral("factor"));
    const QCommandLineOption frameRateOption(
        QStringLiteral("fps"),
        QStringLiteral("Frames drawn per second (default 20, from display.frame_hz in the settings; "
                       "5..240). The strip is projected between engine snapshots, so a higher rate "
                       "smooths a fast scroll without asking the engine for more state."),
        QStringLiteral("hz"));
    const QCommandLineOption benchOption(
        QStringLiteral("waveform-bench"),
        QStringLiteral("Redraw the strip N times and report the cost of a frame, in milliseconds."),
        QStringLiteral("frames"));
    const QCommandLineOption themeCheckOption(
        QStringLiteral("ui-theme-check"),
        QStringLiteral("Grab the shell and fail unless it is dark: a light panel means the palette "
                       "was lost."));
    const QCommandLineOption browserStartOption(
        QStringLiteral("browser-start-check"),
        QStringLiteral("Prove the browser opens the folder remembered from the last load, and that a "
                       "new load moves it."));
    const QCommandLineOption browserCheckOption(
        QStringLiteral("browser-check"),
        QStringLiteral("Load the shell, pick a track in the browser and act on it, the way a click "
                       "does, then print what happened."));
    const QCommandLineOption browserRestoreOption(
        QStringLiteral("browser-restore-check"),
        QStringLiteral("Start the shell on a layout file written by an earlier run and require the "
                       "browser's rail, the browser's text and the deck's zoom to be what that file "
                       "says."));
    const QCommandLineOption screenshotOption(
        QStringLiteral("ui-screenshot"),
        QStringLiteral("Load the shell, wait, grab it into OUT.png and exit. Verifies the theme "
                       "without a display."),
        QStringLiteral("OUT.png"));
    const QCommandLineOption spanOption(
        QStringLiteral("waveform-span"),
        QStringLiteral("Window to render for --waveform-png: START,SPAN as fractions of the track "
                       "(default 0,1). Use a small span to check a zoomed view."),
        QStringLiteral("START,SPAN"));
    const QCommandLineOption waveformPngOption(
        QStringLiteral("waveform-png"),
        QStringLiteral("Render a waveform envelope to an image and exit: PATH OUT.png"),
        QStringLiteral("PATH,OUT"));
    const QCommandLineOption expectSpreadOption(
        QStringLiteral("waveform-expect-spread"),
        QStringLiteral("Fail unless every band claims at least 10% of the loaded track's columns."));
    const QCommandLineOption bandColorOption(
        QStringLiteral("waveform-band-colors"),
        QStringLiteral("Three colours for bass, mids and treble, comma separated "
                       "(default #ff2f1f,#35d13a,#2f6bff)."),
        QStringLiteral("low,mid,high"));
    const QCommandLineOption loadSecondOption(
        QStringLiteral("load-b"),
        QStringLiteral("With --self-check: load this file into deck B as well."),
        QStringLiteral("path"));
    const QCommandLineOption loadOption(
        QStringLiteral("load"),
        QStringLiteral("With --self-check: load this file into deck A first."),
        QStringLiteral("path"));
    parser.addOption(smokeOption);
    parser.addOption(connectOption);
    parser.addOption(selfCheckOption);
    parser.addOption(gainCheckOption);
    parser.addOption(gainCurveOption);
    parser.addOption(gainCurveMiddleOption);
    parser.addOption(faderCurveOption);
    parser.addOption(faderCurveMiddleOption);
    parser.addOption(gainCurveCheckOption);
    parser.addOption(eqCheckOption);
    parser.addOption(tempoCheckOption);
    parser.addOption(neutralOption);
    parser.addOption(pitchOption);
    parser.addOption(transportOption);
    parser.addOption(scenarioOption);
    parser.addOption(padsOption);
    parser.addOption(loopOption);
    parser.addOption(busyCheckOption);
    parser.addOption(historyCheckOption);
    parser.addOption(playlistCheckOption);
    parser.addOption(refusalCheckOption);
    parser.addOption(spaceCheckOption);
    parser.addOption(wheelCheckOption);
    parser.addOption(windowCheckOption);
    parser.addOption(curveCheckOption);
    parser.addOption(masterCheckOption);
    parser.addOption(pageCheckOption);
    parser.addOption(scrollCheckOption);
    parser.addOption(importCheckOption);
    parser.addOption(analyzeCheckOption);
    parser.addOption(sortCheckOption);
    parser.addOption(lockCheckOption);
    parser.addOption(endCheckOption);
    parser.addOption(genreCheckOption);
    parser.addOption(collectionLoadCheckOption);
    parser.addOption(collectionProbeOption);
    parser.addOption(importFixtureOption);
    parser.addOption(gridReconnectOption);
    parser.addOption(metronomeOption);
    parser.addOption(scratchOption);
    parser.addOption(tapOption);
    parser.addOption(gridLinesOption);
    parser.addOption(gridCheckOption);
    parser.addOption(gridCreateOption);
    parser.addOption(loopBeatsOption);
    parser.addOption(fourDeckOption);
    parser.addOption(syncSoakOption);
    parser.addOption(soakSecondsOption);
    parser.addOption(syncCheckOption);
    parser.addOption(transportExtrasOption);
    parser.addOption(gridHereOption);
    parser.addOption(tempoRecheckOption);
    parser.addOption(cueWriteOption);
    parser.addOption(cueReadOption);
    parser.addOption(exitAfterOption);
    parser.addOption(stripCheckOption);
    parser.addOption(keyCheckOption);
    parser.addOption(loadOption);
    parser.addOption(loadSecondOption);
    parser.addOption(outsideColorOption);
    parser.addOption(scrubOption);
    parser.addOption(styleOption);
    parser.addOption(bandColorOption);
    parser.addOption(expectSpreadOption);
    parser.addOption(waveformPngOption);
    parser.addOption(spanOption);
    parser.addOption(screenshotOption);
    parser.addOption(browserCheckOption);
    parser.addOption(browserRestoreOption);
    parser.addOption(browserStartOption);
    parser.addOption(themeCheckOption);
    parser.addOption(benchOption);
    parser.addOption(frameRateOption);
    parser.addOption(zoomOption);
    parser.addOption(contrastOption);
    parser.addOption(layoutOption);
    parser.addOption(layoutFileOption);
    parser.addOption(collectionRootOption);
    parser.addOption(layoutCheckOption);
    parser.addOption(channelOrderOption);
    parser.addOption(sessionFileOption);
    parser.addOption(bandGainOption);
    parser.addOption(barWidthOption);
    parser.addOption(barGapOption);
    parser.addOption(dialogCheckOption);
    parser.process(app);

    auto bridge = std::make_unique<BridgeClient>();
    qmlRegisterSingletonInstance("DjPlatform.Ui", 1, 0, "Bridge", bridge.get());
    FileBrowser browser;
    // The collection is the engine's index, not this process's walk: the browser asks, and only a shell
    // without an engine falls back to reading  itself.
    browser.attachBridge(bridge.get());
    SessionState session;
    if (parser.isSet(sessionFileOption)) {
        session.setFile(parser.value(sessionFileOption));
    }
    session.load();
    // The browser opens where the last track came from; an empty state falls back to the music
    // folder, which is what FileBrowser does when given no path.
    browser.open(session.folder());
    // The rows the two places remember: a folder and a collection are visited for different reasons,
    // and neither should lose its place because the DJ looked at the other.
    browser.seedCursors(session.cursor(), session.collectionCursor());
    // Every load, from the browser or from a deck's dialog, moves that folder.
    bridge->setLoadFolderHandler([&session, &browser](const QString &folder) {
        // A track taken out of the collection lives in the collection: neither the file list nor the
        // next start belongs in its folder. Only a load made from the file list says where the file
        // list is — and it moves the list whether or not the session already knew that folder.
        if (browser.mode() != QLatin1String("files")) {
            return;
        }
        if (session.rememberFolder(folder)) {
            session.save();
        }
        browser.rememberFolder(folder);
    });
    // The folder the DJ browsed to is worth remembering even if he loaded nothing: a session that only
    // remembers loads sends him back to the same place every start, which is what he reported.
    QObject::connect(&browser, &FileBrowser::changed, [&session, &browser]() {
        if (browser.mode() != QLatin1String("files")) {
            return;
        }
        const QString folder = browser.folder();
        if (!folder.isEmpty() && session.rememberFolder(folder)) {
            session.save();
        }
    });
    qmlRegisterSingletonInstance("DjPlatform.Ui", 1, 0, "Browser", &browser);
    // A type, not a singleton: two decks hold two envelopes, and a singleton made the second deck
    // paint the first deck's waveform.
    qmlRegisterType<Waveform>("DjPlatform.Ui", 1, 0, "WaveformEnvelope");
    qmlRegisterType<WaveformItem>("DjPlatform.Ui", 1, 0, "WaveformStrip");
    Waveform waveform; // used by the headless self-check
    UiSettings settings;
    LayoutSettings layout;
    if (parser.isSet(outsideColorOption)
        && !settings.setWaveformOutsideColor(parser.value(outsideColorOption))) {
        std::fprintf(stderr, "dj-ui: %s is not a colour\n",
                     qPrintable(parser.value(outsideColorOption)));
        return 2;
    }
    if (parser.isSet(scrubOption)) {
        const QString mode = parser.value(scrubOption);
        if (mode != QStringLiteral("audible") && mode != QStringLiteral("silent")) {
            std::fprintf(stderr, "dj-ui: --scrub-mode is \"audible\" or \"silent\", not \"%s\"\n",
                         qPrintable(mode));
            return 2;
        }
        settings.setScrubAudible(mode == QStringLiteral("audible"));
    }
    if (parser.isSet(styleOption) && !settings.setWaveformStyle(parser.value(styleOption))) {
        std::fprintf(stderr, "dj-ui: %s is not a waveform style (coloured or plain)\n",
                     qPrintable(parser.value(styleOption)));
        return 2;
    }
    if (parser.isSet(bandColorOption)) {
        const QStringList parts = parser.value(bandColorOption).split(',');
        const QStringList bands = {QStringLiteral("low"), QStringLiteral("mid"), QStringLiteral("high")};
        if (parts.size() != 3) {
            std::fprintf(stderr, "dj-ui: --waveform-band-colors needs three colours\n");
            return 2;
        }
        for (int index = 0; index < 3; ++index) {
            if (!settings.setWaveformBandColor(bands.at(index), parts.at(index).trimmed())) {
                std::fprintf(stderr, "dj-ui: %s is not a colour for band %s\n",
                             qPrintable(parts.at(index)), qPrintable(bands.at(index)));
                return 2;
            }
        }
    }
    if (parser.isSet(barWidthOption)) {
        bool ok = false;
        const int width = parser.value(barWidthOption).toInt(&ok);
        if (!ok || !settings.setWaveformBarWidth(width)) {
            std::fprintf(stderr, "dj-ui: --waveform-bar-width needs 1..32\n");
            return 2;
        }
    }
    if (parser.isSet(barGapOption)) {
        bool ok = false;
        const int gap = parser.value(barGapOption).toInt(&ok);
        if (!ok || !settings.setWaveformBarGap(gap)) {
            std::fprintf(stderr, "dj-ui: --waveform-bar-gap needs 0..32\n");
            return 2;
        }
    }
    if (parser.isSet(contrastOption)) {
        bool ok = false;
        const double power = parser.value(contrastOption).toDouble(&ok);
        if (!ok || power < 0.5 || power > 3.0) {
            std::fprintf(stderr, "dj-ui: --waveform-contrast needs 0.5..3\n");
            return 2;
        }
        Waveform::setDefaultContrast(power);
    }
    if (parser.isSet(zoomOption)) {
        bool ok = false;
        const double factor = parser.value(zoomOption).toDouble(&ok);
        if (!ok || !settings.setWaveformZoom(factor)) {
            std::fprintf(stderr, "dj-ui: --waveform-zoom needs 1..400\n");
            return 2;
        }
    }
    // The shell's own number from the settings file: the engine reads the file (it owns `dj-config`) and
    // publishes the frame rate, and the shell follows it once, unless `--fps` was given. A shell that read
    // TOML itself would be a second reader of one truth.
    if (!parser.isSet(frameRateOption) && bridge) {
        auto followed = std::make_shared<bool>(false);
        QObject::connect(bridge.get(), &BridgeClient::snapshotChanged, bridge.get(),
                         [&settings, followed, client = bridge.get()]() {
                             if (*followed) {
                                 return;
                             }
                             const int hz = client->snapshot()
                                                .value(QStringLiteral("frame_hz"))
                                                .toInt();
                             if (hz >= 5 && settings.setFrameHz(hz)) {
                                 *followed = true;
                                 std::fprintf(stdout, "dj-ui: the frame rate is %d, from settings\n", hz);
                             }
                         });
    }
    if (parser.isSet(frameRateOption)) {
        bool ok = false;
        const int hz = parser.value(frameRateOption).toInt(&ok);
        if (!ok || !settings.setFrameHz(hz)) {
            std::fprintf(stderr, "dj-ui: --fps needs 5..240\n");
            return 2;
        }
    }
    double bandGain[3] = {1.0, 1.0, 1.0};
    if (parser.isSet(bandGainOption)) {
        const QStringList parts = parser.value(bandGainOption).split(',');
        bool ok = parts.size() == 3;
        double gains[3] = {1.0, 1.0, 1.0};
        for (int index = 0; ok && index < 3; ++index) {
            gains[index] = parts.at(index).trimmed().toDouble(&ok);
            ok = ok && gains[index] > 0.0;
        }
        if (!ok) {
            std::fprintf(stderr, "dj-ui: --waveform-band-gain needs three positive numbers\n");
            return 2;
        }
        bandGain[0] = gains[0];
        bandGain[1] = gains[1];
        bandGain[2] = gains[2];
        waveform.setBandGain(bandGain[0], bandGain[1], bandGain[2]);
    }
    waveform.setPalette(QColor(settings.waveformLowColor()), QColor(settings.waveformMidColor()),
                        QColor(settings.waveformHighColor()));
    if (parser.isSet(layoutFileOption)) {
        layout.setFile(parser.value(layoutFileOption));
    }
    // The file is the user's last arrangement; a preset named on the command line wins over it, so a
    // single run can be compared against the measured default without editing anything.
    layout.load();
    // The remembered folder is the DJ's choice, wherever it is: a folder inside the collection is a
    // perfectly good place to browse from. Only a folder that no longer exists falls back to the home
    // folder, and `open` already does that on its own.
    if (parser.isSet(layoutOption) && !layout.applyPreset(parser.value(layoutOption))) {
        std::fprintf(stderr, "dj-ui: --layout expects traktor or two-deck\n");
        return 2;
    }
    // The deck's modes are settings the DJ leaves behind: the file carries them across a restart, and a
    // flag given on the command line wins over a saved value (owner's decision, 14.11). The decks and the
    // mixer read `settings`; `layout` is what the file holds, and it is read before this point.
    settings.setSnapGrid(layout.snapGrid());
    settings.setQuantizeGrid(layout.quantizeGrid());
    if (parser.isSet(scrubOption)) {
        // The flag is a decision too: it becomes the saved one, so the next start has it without a flag.
        layout.setScrubAudible(settings.scrubAudible());
    } else {
        settings.setScrubAudible(layout.scrubAudible());
    }
    if (parser.isSet(collectionRootOption)) {
        layout.setCollectionRoots(parser.values(collectionRootOption));
    } else if (layout.collectionRoots().isEmpty()) {
        // No root chosen yet: the collection lives in the shell's own data folder, and that choice is
        // written to the layout file when the shell exits.
        layout.setCollectionRoots({browser.defaultCollectionRoot()});
    }
    if (parser.isSet(channelOrderOption)
        && !layout.setChannelOrder(parser.value(channelOrderOption))) {
        std::fprintf(stderr, "dj-ui: --channel-order needs each of A, B, C and D once\n");
        return 2;
    }

    if (parser.isSet(gainCurveOption)
        && !settings.setGainCurve(parser.value(gainCurveOption).trimmed())) {
        std::fprintf(stderr, "dj-ui: %s is not a gain curve (linear or cubic)\n",
                     qPrintable(parser.value(gainCurveOption)));
        return 2;
    }
    if (parser.isSet(gainCurveMiddleOption)) {
        bool ok = false;
        const double middle = parser.value(gainCurveMiddleOption).toDouble(&ok);
        if (!ok || !settings.setGainCurveMiddle(middle)) {
            std::fprintf(stderr, "dj-ui: --gain-curve-middle wants a number from 0.05 to 1\n");
            return 2;
        }
    }
    if (parser.isSet(faderCurveOption)
        && !settings.setFaderCurve(parser.value(faderCurveOption).trimmed())) {
        std::fprintf(stderr, "dj-ui: %s is not a fader curve (linear, cubic or logarithmic)\n",
                     qPrintable(parser.value(faderCurveOption)));
        return 2;
    }
    if (parser.isSet(faderCurveMiddleOption)) {
        bool ok = false;
        const double middle = parser.value(faderCurveMiddleOption).toDouble(&ok);
        if (!ok || !settings.setFaderCurveMiddle(middle)) {
            std::fprintf(stderr, "dj-ui: --fader-curve-middle wants a number from 0.05 to 1\n");
            return 2;
        }
    }
    qmlRegisterSingletonInstance("DjPlatform.Ui", 1, 0, "UiSettings", &settings);
    // Not "Layout": that name belongs to QtQuick.Layouts' attached property.
    qmlRegisterSingletonInstance("DjPlatform.Ui", 1, 0, "LayoutState", &layout);
    // The arrangement is the user's, and it is written when the shell exits: a root adopted by
    // default must reach the file even if nothing was dragged and no flag was typed.
    QObject::connect(&app, &QCoreApplication::aboutToQuit, [&layout, &browser, &session]() {
        // Where the cursor stood is part of the session, like the folder: a browser that forgets the
        // row makes the DJ find it again every evening.
        session.rememberCursor(browser.filesCursor());
        session.rememberCollectionCursor(browser.collectionCursor());
        session.save();
        layout.save();
    });

    if (parser.isSet(waveformPngOption)) {
        // The envelope drawn exactly as the deck draws it, but into a file: no display, no engine,
        // and something that can be measured afterwards instead of described.
        const QStringList parts = parser.value(waveformPngOption).split(',');
        if (parts.size() != 2) {
            std::fprintf(stderr, "dj-ui: --waveform-png needs PATH and OUT.png\n");
            return 2;
        }
        Waveform envelope;
        envelope.setPalette(QColor(settings.waveformLowColor()),
                            QColor(settings.waveformMidColor()),
                            QColor(settings.waveformHighColor()));
        envelope.setBandGain(bandGain[0], bandGain[1], bandGain[2]);
        if (!envelope.load(parts.at(0))) {
            std::fprintf(stderr, "dj-ui: %s\n", qPrintable(envelope.error()));
            return 1;
        }
        const int width = 1200;
        const int height = 160;
        const int barWidth = settings.waveformBarWidth();
        const int barGap = settings.waveformBarGap();
        const int barCount = qMax(1, width / (barWidth + barGap));
        double windowStart = 0.0;
        double windowSpan = 1.0;
        if (parser.isSet(spanOption)) {
            const QStringList span = parser.value(spanOption).split(',');
            bool ok = span.size() == 2;
            if (ok) {
                windowStart = span.at(0).toDouble(&ok);
                windowSpan = span.at(1).toDouble(&ok);
            }
            if (!ok || windowSpan <= 0.0) {
                std::fprintf(stderr, "dj-ui: --waveform-span needs START,SPAN within 0..1\n");
                return 2;
            }
        }
        const QVariantList pixels = envelope.profile(barCount, windowStart, windowSpan);
        QImage image(width, height, QImage::Format_RGB32);
        image.fill(QColor(settings.waveformStyle() == QStringLiteral("coloured")
                              ? QStringLiteral("#141414")
                              : QStringLiteral("#101010")));
        QPainter painter(&image);
        const double middle = height / 2.0;
        int painted = 0;
        for (int index = 0; index < pixels.size(); ++index) {
            const QVariantMap pixel = pixels.at(index).toMap();
            if (pixel.value(QStringLiteral("outside")).toBool()) {
                continue;
            }
            const double half = qMax(0.5, pixel.value(QStringLiteral("amplitude")).toDouble() * middle);
            // Full brightness at the centre axis, about half at the tips — the glow the deck draws.
            const QColor colour(pixel.value(QStringLiteral("rgb")).toString());
            static const double glow[3] = {0.42, 1.0, 0.42};
            const double x = index * (barWidth + barGap);
            for (int slice = 0; slice < 3; ++slice) {
                const double from = middle - half + (2 * half) * slice / 3.0;
                const double to = middle - half + (2 * half) * (slice + 1) / 3.0;
                QColor shaded = colour;
                shaded.setRedF(qBound(0.0, colour.redF() * glow[slice], 1.0));
                shaded.setGreenF(qBound(0.0, colour.greenF() * glow[slice], 1.0));
                shaded.setBlueF(qBound(0.0, colour.blueF() * glow[slice], 1.0));
                painter.fillRect(QRectF(x, from, barWidth, qMax(1.0, to - from)), shaded);
            }
            ++painted;
        }
        painter.end();
        if (!image.save(parts.at(1))) {
            std::fprintf(stderr, "dj-ui: cannot write %s\n", qPrintable(parts.at(1)));
            return 1;
        }
        // The numbers behind the picture: what a colour does globally is not visible by eye when
        // one band leads everywhere, and that is exactly the complaint to check for.
        // Nine buckets, and "white" first: with the bands driving RGB directly, dense material
        // legitimately comes out pale, and calling that "blue" because the blue channel is a shade
        // higher would misreport the picture.
        int hues[8] = {0, 0, 0, 0, 0, 0, 0, 0};
        const char *names[8] = {"white", "red", "orange", "yellow",
                                "green", "cyan", "blue", "violet"};
        for (int index = 0; index < pixels.size(); ++index) {
            const QVariantMap pixel = pixels.at(index).toMap();
            if (pixel.value(QStringLiteral("outside")).toBool()
                || pixel.value(QStringLiteral("amplitude")).toDouble() * middle < 0.5) {
                continue;
            }
            const QColor colour(pixel.value(QStringLiteral("rgb")).toString());
            int bucket = 0;
            if (colour.saturationF() > 0.15) {
                const int hue = int(colour.hsvHueF() * 360.0);
                if (hue < 20 || hue >= 340) {
                    bucket = 1;
                } else if (hue < 45) {
                    bucket = 2;
                } else if (hue < 70) {
                    bucket = 3;
                } else if (hue < 160) {
                    bucket = 4;
                } else if (hue < 210) {
                    bucket = 5;
                } else if (hue < 265) {
                    bucket = 6;
                } else {
                    bucket = 7;
                }
            }
            ++hues[bucket];
        }
        std::fprintf(stdout, "dj-ui: wrote %s, %d of %d columns painted\n",
                     qPrintable(parts.at(1)), painted, width);
        // How jagged the outline is: the mean distance of a bar from the average of its neighbours,
        // in pixels. A pixel moiré shows up here as a number around two; a smooth outline is under
        // one. Printed and asserted, because "looks grainy" is not something a gate can read.
        double roughness = 0.0;
        int compared = 0;
        for (int index = 1; index + 1 < pixels.size(); ++index) {
            const QVariantMap bar = pixels.at(index).toMap();
            if (bar.value(QStringLiteral("outside")).toBool()) {
                continue;
            }
            const double left = pixels.at(index - 1)
                                    .toMap()
                                    .value(QStringLiteral("amplitude"))
                                    .toDouble();
            const double right = pixels.at(index + 1)
                                     .toMap()
                                     .value(QStringLiteral("amplitude"))
                                     .toDouble();
            const double here = bar.value(QStringLiteral("amplitude")).toDouble();
            roughness += qAbs(here - 0.5 * (left + right)) * middle;
            ++compared;
        }
        if (compared > 0) {
            std::fprintf(stdout, "dj-ui: roughness %.2f px\n", roughness / compared);
        }
        std::fprintf(stdout, "dj-ui: hues");
        for (int index = 0; index < 8; ++index) {
            if (hues[index] > 0) {
                std::fprintf(stdout, " %s %d", names[index], hues[index]);
            }
        }
        std::fprintf(stdout, "\n");
        return 0;
    }

    if (parser.isSet(busyCheckOption)) {
        BridgeClient busyBridge;
        return runBusyCheck(busyBridge, parser.value(connectOption), parser.value(loadOption),
                            parser.value(loadSecondOption), 15000);
    }

    if (parser.isSet(historyCheckOption)) {
        BridgeClient historyBridge;
        return runHistoryCheck(historyBridge, parser.value(connectOption), parser.value(loadOption),
                               60000);
    }

    if (parser.isSet(gainCheckOption)) {
        BridgeClient gainBridge;
        return runGainCheck(gainBridge, parser.value(connectOption), 5000);
    }

    if (parser.isSet(selfCheckOption)) {
        const QString path = parser.isSet(connectOption) ? parser.value(connectOption)
                                                         : defaultSocketPath();
        return runSelfCheck(
            *bridge, waveform, path, 20000,
            parser.isSet(loadOption) ? parser.value(loadOption) : QString(),
            parser.isSet(loadSecondOption) ? parser.value(loadSecondOption) : QString(),
            parser.isSet(expectSpreadOption));
    }

    if (parser.isSet(dialogCheckOption)) {
        // The load dialog must list a directory without any desktop package: no GTK, no portal and
        // no QML extras, because a missing schema or module otherwise aborts the whole UI.
        browser.open(QString());
        const QVariantList entries = browser.entries();
        if (!browser.error().isEmpty()) {
            std::fprintf(stderr, "dj-ui: the file browser failed: %s\n", qPrintable(browser.error()));
            return 1;
        }
        std::fprintf(stdout, "dj-ui: file browser lists %d entries in %s\n",
                     static_cast<int>(entries.size()), qPrintable(browser.folder()));
        return 0;
    }

    QQmlApplicationEngine engine;
    engine.loadFromModule(QStringLiteral("DjPlatform.Ui"), QStringLiteral("Main"));

    if (engine.rootObjects().isEmpty()) {
        std::fprintf(stderr, "dj-ui: the QML scene did not load\n");
        return 1;
    }

    // The shape the window was left in: window, maximized or full screen. Restored before any check
    // runs, so a check sees the same window a person would.
    auto *mainWindow = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
    if (mainWindow != nullptr) {
        // What full screen returns to. It lives in the session file too: a restart that lands in full
        // screen must still know whether it came from a maximized window or a plain one.
        auto restoreTo = std::make_shared<QString>(
            session.restoreTo().isEmpty() ? QStringLiteral("window") : session.restoreTo());
        // The scene shows its window as a plain one while loading. That first transition is not a
        // choice of the person, and writing it down would overwrite the shape the file remembers —
        // which is exactly what happened before this guard: every start forgot a maximized window.
        auto settled = std::make_shared<bool>(false);
        // Where the window is on its way to, when a transition takes two steps: leaving full screen goes
        // through a plain window before it becomes a maximized one, and that first step must not be
        // recorded as the shape the person chose.
        auto pending = std::make_shared<QString>();

        // F11 belongs to the window, not to whichever control happens to hold the focus: a filter sees
        // the key whatever the focus is, and a shortcut on the QML item would only fire when the shell
        // itself had it.
        class FullScreenKey : public QObject
        {
        public:
            FullScreenKey(QQuickWindow *window, std::shared_ptr<QString> restoreTo,
                          std::shared_ptr<QString> pending,
                          std::function<void(const QString &)> remember)
                : QObject(window)
                , m_window(window)
                , m_restoreTo(std::move(restoreTo))
                , m_pending(std::move(pending))
                , m_remember(std::move(remember))
            {
            }

        protected:
            bool eventFilter(QObject *watched, QEvent *event) override
            {
                if (event->type() != QEvent::KeyPress) {
                    return QObject::eventFilter(watched, event);
                }
                auto *key = static_cast<QKeyEvent *>(event);
                if (key->key() != Qt::Key_F11 || key->isAutoRepeat()) {
                    return QObject::eventFilter(watched, event);
                }
                if (m_window->visibility() == QQuickWindow::FullScreen) {
                    // Back to the shape it came from. showNormal() first: a window manager may ignore
                    // a request to become maximized while the window is still full screen.
                    const bool toMaximized = *m_restoreTo == QLatin1String("maximized");
                    *m_pending = toMaximized ? QStringLiteral("maximized") : QStringLiteral("window");
                    m_window->showNormal();
                    if (toMaximized) {
                        m_window->showMaximized();
                    }
                } else {
                    *m_restoreTo = m_window->visibility() == QQuickWindow::Maximized
                        ? QStringLiteral("maximized")
                        : QStringLiteral("window");
                    m_remember(*m_restoreTo);
                    m_window->showFullScreen();
                }
                return true;
            }

        private:
            QQuickWindow *m_window = nullptr;
            std::shared_ptr<QString> m_restoreTo;
            std::shared_ptr<QString> m_pending;
            std::function<void(const QString &)> m_remember;
        };

        mainWindow->installEventFilter(new FullScreenKey(
            mainWindow, restoreTo, pending, [&session](const QString &shape) {
                if (session.rememberRestoreTo(shape)) {
                    session.save();
                }
            }));

        QObject::connect(
            mainWindow, &QQuickWindow::visibilityChanged, mainWindow,
            [&session, mainWindow, settled, restoreTo, pending]() {
                if (!*settled) {
                    return;
                }
                QString shape;
                switch (mainWindow->visibility()) {
                case QQuickWindow::FullScreen:
                    shape = QStringLiteral("fullscreen");
                    break;
                case QQuickWindow::Maximized:
                    shape = QStringLiteral("maximized");
                    break;
                case QQuickWindow::Windowed:
                    shape = QStringLiteral("window");
                    break;
                default:
                    return;
                }
                if (!pending->isEmpty()) {
                    // A step on the way to the target: not the person's choice yet.
                    if (*pending != shape) {
                        return;
                    }
                    pending->clear();
                }
                bool changed = false;
                if (shape == QLatin1String("fullscreen")) {
                    changed = session.rememberWindow(shape);
                } else {
                    const bool wasFull = session.window() == QLatin1String("fullscreen");
                    changed = session.rememberWindow(shape);
                    if (wasFull) {
                        *restoreTo = shape;
                        changed = session.rememberRestoreTo(shape) || changed;
                    }
                }
                if (changed) {
                    session.save();
                }
            });

        // Applied once the window exists: on a real display a mode set before the surface is mapped is
        // dropped, and the window opens plain.
        const QString wanted = session.window();
        QTimer::singleShot(0, mainWindow, [mainWindow, wanted, settled]() {
            if (wanted == QLatin1String("maximized")) {
                mainWindow->showMaximized();
            } else if (wanted == QLatin1String("fullscreen")) {
                mainWindow->showFullScreen();
            }
            *settled = true;
        });
    }

    if (parser.isSet(cueWriteOption) || parser.isSet(cueReadOption)) {
        // A13: cues and loops are decisions about a file, so they must outlive the process. The write
        // check makes them, the read check finds them with no command of its own.
        const bool writing = parser.isSet(cueWriteOption);
        const QString fixture = parser.value(loadOption);
        if (fixture.isEmpty()) {
            std::fprintf(stderr, "dj-ui: the cue check needs --load\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        QElapsedTimer linking;
        linking.start();
        while (!bridge->isConnected() && linking.elapsed() < 3000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(10);
        }
        if (!bridge->isConnected()) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        const auto deckA = [&]() {
            const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant &entry : decks) {
                const QVariantMap deck = entry.toMap();
                if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    return deck;
                }
            }
            return QVariantMap();
        };
        const auto sourceFrame = [&]() {
            return deckA().value(QStringLiteral("source_frame")).toULongLong();
        };
        const auto waitFor = [&](const std::function<bool()> &ready, int milliseconds) {
            QElapsedTimer timer;
            timer.start();
            while (timer.elapsed() < milliseconds) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                if (ready()) {
                    return true;
                }
                QThread::msleep(10);
            }
            return ready();
        };

        // Earlier checks in the same engine leave a deck playing or loaded; start from an empty one,
        // because this check is about metadata and not about what ran before it.
        bridge->play(QStringLiteral("A"), false);
        bridge->eject(QStringLiteral("A"));
        waitFor([&]() {
            const QVariant load = deckA().value(QStringLiteral("load"));
            return load.typeId() == QMetaType::QString && load.toString() == QLatin1String("Empty");
        }, 2000);
        bridge->load(QStringLiteral("A"), fixture);

        const bool loaded = waitFor([&]() {
            const QVariant load = deckA().value(QStringLiteral("load"));
            if (load.typeId() == QMetaType::QString) {
                // A plain string is a state with no data: Empty, Ready.
                return load.toString() == QLatin1String("Ready");
            }
            // An object is Loading { progress } or Failed { message }; only the failure is final.
            return load.toMap().keys().value(0) == QLatin1String("Failed");
        }, 8000);
        if (!loaded) {
            std::fprintf(stderr, "dj-ui: the cue check could not load the track\n");
            return 1;
        }

        if (writing) {
            // A deck left playing by an earlier check would run past the frame while we look at it;
            // the cue has to land on a known frame, so stop first.
            bridge->play(QStringLiteral("A"), false);
            bridge->seek(QStringLiteral("A"), 48000);
            if (!waitFor([&]() { return sourceFrame() >= 48000; }, 3000)) {
                std::fprintf(stderr, "dj-ui: the deck did not reach frame 48000\n");
                return 1;
            }
            bridge->cue(QStringLiteral("A"), QStringLiteral("Set"), 2);
            // A loop is only armed around the playhead, which is how a hand makes one; park inside the
            // span first, then arm it, then save it into a pad.
            bridge->seek(QStringLiteral("A"), 120000);
            if (!waitFor([&]() { return sourceFrame() >= 120000; }, 3000)) {
                std::fprintf(stderr, "dj-ui: the deck did not reach frame 120000\n");
                return 1;
            }
            bridge->setLoop(QStringLiteral("A"), true, 96000, 144000);
            // A loop that is only active lives in memory; what a restart must bring back is the pad it was
            // saved into (DJP-DECK-004), so the check stores it there too.
            bridge->setPadLoop(QStringLiteral("A"), 6);
            // The check is about a file, not about memory: the engine writes queued edits on a thread
            // of its own, and this process is taken down the moment the check returns. So wait for the
            // file to hold the edit — otherwise a restart proves nothing and the last edit can die
            // with the process, which is exactly how this check used to fail now and then.
            const QString sidecar =
                QFileInfo(fixture).absolutePath() + QStringLiteral("/.djmeta/") +
                QFileInfo(fixture).fileName() + QStringLiteral(".json");
            const bool stored = waitFor(
                [&]() {
                    QFile file(sidecar);
                    if (!file.open(QIODevice::ReadOnly)) {
                        return false;
                    }
                    const QJsonObject meta = QJsonDocument::fromJson(file.readAll()).object();
                    bool cueStored = false;
                    const QJsonArray cues = meta.value(QStringLiteral("cues")).toArray();
                    for (const QJsonValue &value : cues) {
                        const QJsonObject storedCue = value.toObject();
                        cueStored = cueStored ||
                                    (storedCue.value(QStringLiteral("slot")).toInt() == 2 &&
                                     storedCue.value(QStringLiteral("frame")).toVariant().toULongLong() ==
                                         48000);
                    }
                    bool loopStored = false;
                    const QJsonArray loops = meta.value(QStringLiteral("loops")).toArray();
                    for (const QJsonValue &value : loops) {
                        const QJsonObject storedLoop = value.toObject();
                        loopStored = loopStored ||
                                     (storedLoop.value(QStringLiteral("slot")).toInt() == 6 &&
                                      storedLoop.value(QStringLiteral("start")).toVariant().toULongLong() ==
                                          96000 &&
                                      storedLoop.value(QStringLiteral("end")).toVariant().toULongLong() ==
                                          144000);
                    }
                    return cueStored && loopStored;
                },
                3000);
            if (!stored) {
                std::fprintf(stderr, "dj-ui: the cue never reached %s\n", qPrintable(sidecar));
                return 1;
            }
            std::fprintf(stdout, "dj-ui: the cue and the pad loop are in %s\n", qPrintable(sidecar));
        }

        quint64 cue = 0;
        quint64 loopStart = 0;
        quint64 loopEnd = 0;
        const bool seen = waitFor(
            [&]() {
                const QVariantMap deck = deckA();
                const QVariantList hotcues = deck.value(QStringLiteral("hotcues")).toList();
                cue = hotcues.size() > 2 ? hotcues.at(2).toULongLong() : 0;
                loopStart = 0;
                loopEnd = 0;
                // pad_loops is positional: entry i belongs to pad i.
                const QVariantList loops = deck.value(QStringLiteral("pad_loops")).toList();
                if (loops.size() > 6) {
                    const QVariantMap loop = loops.at(6).toMap();
                    loopStart = loop.value(QStringLiteral("start")).toULongLong();
                    loopEnd = loop.value(QStringLiteral("end")).toULongLong();
                }
                return cue == 48000 && loopStart == 96000 && loopEnd == 144000;
            },
            6000);
        std::fprintf(stdout, "dj-ui: %s hotcue 2 at %llu, pad loop %llu..%llu\n",
                     writing ? "stored" : "restored", static_cast<unsigned long long>(cue),
                     static_cast<unsigned long long>(loopStart),
                     static_cast<unsigned long long>(loopEnd));
        if (!seen) {
            std::fprintf(stderr, "dj-ui: the cue or the pad loop did not survive\n");
            return 1;
        }
        std::fprintf(stdout, "dj-ui: %s\n", writing ? "the cue and loop reached the engine"
                                                    : "the reload brought the cue and the pad loop back");
        return 0;
    }

    if (parser.isSet(masterCheckOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        QQuickItem *content = window->contentItem();

        // The window is sized by the session and is not shown in a headless run: give it the real
        // size, or every measurement below is taken against a 0x0 window and lies small.
        window->resize(1366, 768);
        window->show();
        QCoreApplication::processEvents(QEventLoop::AllEvents, 100);

        const auto item = [&](const QString &name) { return findItem(content, name); };
        auto *knob = item(QStringLiteral("masterKnob"));
        auto *meter = item(QStringLiteral("masterMeter"));
        auto *column = item(QStringLiteral("masterColumn"));
        auto *stripA = item(QStringLiteral("strip_A"));
        auto *stripB = item(QStringLiteral("strip_B"));
        auto *meterA = item(QStringLiteral("meter_A"));
        if (knob == nullptr || meter == nullptr || column == nullptr || stripA == nullptr
            || stripB == nullptr || meterA == nullptr) {
            std::fprintf(stderr, "dj-ui: the master column is not in the mixer\n");
            return 1;
        }
        // Between A and B, not at an end.
        const double columnMid = column->mapToItem(content, QPointF(column->width() / 2.0, 0)).x();
        const double aMid = stripA->mapToItem(content, QPointF(stripA->width() / 2.0, 0)).x();
        const double bMid = stripB->mapToItem(content, QPointF(stripB->width() / 2.0, 0)).x();
        std::fprintf(stdout, "dj-ui: the master column sits at %.0f px, between A at %.0f and B at %.0f\n",
                     columnMid, aMid, bMid);
        if (!(aMid < columnMid && columnMid < bMid)) {
            std::fprintf(stderr, "dj-ui: the master column is not between the A and B strips\n");
            return 1;
        }
        // Round, and above the meter.
        std::fprintf(stdout, "dj-ui: the master knob is %.0f x %.0f px\n", knob->width(),
                     knob->height());
        if (std::abs(knob->width() - knob->height()) > 1.0) {
            std::fprintf(stderr, "dj-ui: the master knob is not round\n");
            return 1;
        }
        // Every meter in the mixer is the same height as the master's, and they end on one line: the
        // meter band is defined by the master column, where the knob takes the room above it.
        const double knobBottom = knob->mapToItem(content, QPointF(0, knob->height())).y();
        const double meterTop = meter->mapToItem(content, QPointF(0, 0)).y();
        const double meterBottom = meter->mapToItem(content, QPointF(0, meter->height())).y();
        const double deckMeterBottom = meterA->mapToItem(content, QPointF(0, meterA->height())).y();
        std::fprintf(stdout, "dj-ui: the master meter starts %.0f px below the knob and ends %.0f px "
                             "from the deck meters; heights %.0f and %.0f px\n",
                     meterTop - knobBottom, meterBottom - deckMeterBottom, meter->height(),
                     meterA->height());
        auto *row = item(QStringLiteral("stripRow"));
        if (meterTop < knobBottom) {
            std::fprintf(stderr, "dj-ui: the master knob overlaps the meter\n");
            return 1;
        }
        std::fprintf(stdout, "dj-ui: the meter band is %.0f px inside a %.0f px strip row\n",
                     meter->height(), row != nullptr ? row->height() : -1.0);
        if (std::abs(meter->height() - meterA->height()) > 1.0) {
            std::fprintf(stderr, "dj-ui: the deck meters are not the master's height\n");
            return 1;
        }
        if (std::abs(meterBottom - deckMeterBottom) > 2.0) {
            std::fprintf(stderr, "dj-ui: the meters do not line up at the bottom\n");
            return 1;
        }

        // Three colours, and none of them the accent: the master must be findable by colour alone.
        const QString green = settings.themeMeterGreen();
        const QString yellow = settings.themeMeterYellow();
        const QString red = settings.themeMeterRed();
        std::fprintf(stdout, "dj-ui: master meter colours %s %s %s\n", qPrintable(green),
                     qPrintable(yellow), qPrintable(red));
        if (green == yellow || yellow == red || green == red || green == settings.themeAccent()) {
            std::fprintf(stderr, "dj-ui: the master meter colours do not separate from each other\n");
            return 1;
        }



        bridge->connectToEngine(parser.value(connectOption));
        QElapsedTimer linking;
        linking.start();
        while (!bridge->isConnected() && linking.elapsed() < 3000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(10);
        }
        if (!bridge->isConnected()) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        // The knob's command must land in the published gain, which is what the label and the pointer
        // read: a knob that moves a value nobody publishes is a knob that springs back.
        const auto gainNow = [&]() {
            return bridge->snapshot().value(QStringLiteral("master_gain")).toDouble();
        };
        bridge->setMasterGain(0.5);
        QElapsedTimer waiting;
        waiting.start();
        double seen = gainNow();
        while (waiting.elapsed() < 3000 && std::abs(seen - 0.5) > 0.01) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            seen = gainNow();
            QThread::msleep(10);
        }
        std::fprintf(stdout, "dj-ui: master gain 0.5 -> snapshot says %.2f\n", seen);
        if (std::abs(seen - 0.5) > 0.01) {
            std::fprintf(stderr, "dj-ui: the master gain did not reach the engine\n");
            return 1;
        }

        // And the meter must see the signal, not just exist: load a track, play it, watch the peak.
        const QString fixture = parser.value(loadOption);
        if (!fixture.isEmpty()) {
            // Another check may have left deck A playing to its end, where a play command produces
            // silence. Empty the deck first: this check is about the master meter, not about what ran
            // before it in the same engine.
            bridge->play(QStringLiteral("A"), false);
            bridge->eject(QStringLiteral("A"));
            QElapsedTimer clearing;
            clearing.start();
            // A snapshot still saying Ready describes the track that was there a moment ago: wait for
            // the deck to report itself empty, or the load below is believed before it happened and a
            // play command lands on nothing.
            while (clearing.elapsed() < 2000) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                const QVariantList decksNow =
                    bridge->snapshot().value(QStringLiteral("decks")).toList();
                bool ready = false;
                for (const QVariant &entry : decksNow) {
                    const QVariantMap deck = entry.toMap();
                    if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                        const QVariant load = deck.value(QStringLiteral("load"));
                        const QString name = load.typeId() == QMetaType::QString
                            ? load.toString()
                            : load.toMap().keys().value(0);
                        ready = name == QLatin1String("Ready");
                    }
                }
                if (!ready) {
                    break;
                }
                QThread::msleep(10);
            }
            // A load can be refused for a moment after the eject: try, wait for Ready, try again.
            const auto loadState = [&]() {
                const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
                for (const QVariant &entry : decks) {
                    const QVariantMap deck = entry.toMap();
                    if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                        const QVariant load = deck.value(QStringLiteral("load"));
                        // serde writes a variant with data as an object with one key, and one without
                        // as a plain string.
                        return load.typeId() == QMetaType::QString
                            ? load.toString()
                            : load.toMap().keys().value(0);
                    }
                }
                return QString();
            };
            QString load;
            for (int attempt = 0; attempt < 3 && load != QLatin1String("Ready"); ++attempt) {
                bridge->load(QStringLiteral("A"), fixture);
                QElapsedTimer settling;
                settling.start();
                while (settling.elapsed() < 4000) {
                    QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                    load = loadState();
                    if (load == QLatin1String("Ready") || load == QLatin1String("Failed")) {
                        break;
                    }
                    QThread::msleep(20);
                }
            }
            if (load != QLatin1String("Ready")) {
                std::fprintf(stderr, "dj-ui: the master check could not load a track\n");
                return 1;
            }
            bridge->play(QStringLiteral("A"), true);
            QElapsedTimer playing;
            playing.start();
            double peak = 0.0;
            while (playing.elapsed() < 3000 && peak <= 0.0) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                peak = bridge->snapshot().value(QStringLiteral("master")).toMap()
                           .value(QStringLiteral("peak_left")).toDouble();
                QThread::msleep(20);
            }
            // Where the signal stops, if it stops: the deck's own meter and the transport state say
            // whether the master is silent because nothing plays or because nothing is routed.
            bool playingA = false;
            double deckPeak = 0.0;
            const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant &entry : decks) {
                const QVariantMap deck = entry.toMap();
                if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    playingA = deck.value(QStringLiteral("playing")).toBool();
                    deckPeak = deck.value(QStringLiteral("levels")).toMap()
                                   .value(QStringLiteral("peak_left")).toDouble();
                }
            }
            std::fprintf(stdout, "dj-ui: deck A playing %s, its own peak %.3f, master peak %.3f\n",
                         playingA ? "yes" : "no", deckPeak, peak);
            // The live meter, still bound to the snapshot: -9 dB is well inside the green.
            std::fprintf(stdout, "dj-ui: the master meter reads %s at %.3f\n",
                         qPrintable(meter->property("zone").toString()), peak);
            if (meter->property("zone").toString() != QLatin1String("green")) {
                std::fprintf(stderr, "dj-ui: the master meter colours the live level wrongly\n");
                return 1;
            }

            if (peak <= 0.0) {
                std::fprintf(stderr, "dj-ui: the master meter stays at zero with a deck playing\n");
                return 1;
            }
            bridge->play(QStringLiteral("A"), false);
        }
        // The colour rules, asked of the meter itself: green means room, yellow means close to
        // 0 dB, red means at or past it. A meter whose colours were squeezed into its own bar showed
        // red at the top of every level.
        const auto zoneAt = [&](double linear) {
            // The zone is a property of the meter, so ask it: feeding it a level and reading the
            // answer tests the rule, not a copy of it.
            meter->setProperty("peak", QVariant(linear));
            return meter->property("zone").toString();
        };
        const QString zoneLow = zoneAt(0.1);    // -20 dB
        const QString zoneAt6 = zoneAt(0.45);   // -6.9 dB
        const QString zoneNear = zoneAt(0.6);   // -4.4 dB
        const QString zoneFull = zoneAt(1.0);   // 0 dB, the top edge
        std::fprintf(stdout, "dj-ui: master meter zones: -20 dB %s, -6.9 dB %s, -4.4 dB %s, 0 dB %s\n",
                     qPrintable(zoneLow), qPrintable(zoneAt6), qPrintable(zoneNear),
                     qPrintable(zoneFull));
        if (zoneLow != QLatin1String("green") || zoneAt6 != QLatin1String("green")
            || zoneNear != QLatin1String("yellow") || zoneFull != QLatin1String("red")) {
            std::fprintf(stderr, "dj-ui: the master meter colours do not follow the level\n");
            return 1;
        }
        std::fprintf(stdout, "dj-ui: master meter yellow from %.0f dB, red from %.0f dB, top %.0f dB\n",
                     meter->property("yellowFromDb").toDouble(),
                     meter->property("redFromDb").toDouble(),
                     meter->property("ceilingDb").toDouble());

        // And the picture itself: a gradient runs top-down while the thresholds are measured from the
        // bottom, so the colours can be logical in the properties and upside down on screen. Drive the
        // meter past 0 dB — that is the only place red exists — and read the pixels: red at the top,
        // yellow under it, green at the foot.
        meter->setProperty("peak", QVariant(1.0));
        QCoreApplication::processEvents(QEventLoop::AllEvents, 50);
        const QImage frame = window->grabWindow();
        if (frame.isNull()) {
            std::fprintf(stdout, "dj-ui: the window would not render, so the meter colours are unread\n");
        } else {
            const QPointF origin = meter->mapToItem(content, QPointF(0, 0));
            const double ratio = content->width() > 0
                ? static_cast<double>(frame.width()) / content->width() : 1.0;
            const auto sample = [&](double downFromTop) {
                const int x = static_cast<int>((origin.x() + meter->width() / 2.0) * ratio);
                const int y = static_cast<int>((origin.y() + downFromTop) * ratio);
                return frame.pixelColor(x, y);
            };
            const double height = meter->height();
            const QColor head = sample(height * 0.03);
            const QColor middle = sample(height * 0.12);
            const QColor foot = sample(height * 0.90);
            std::fprintf(stdout, "dj-ui: at full scale the master meter reads %s at the top, %s under "
                                 "it and %s at the foot\n",
                         qPrintable(head.name()), qPrintable(middle.name()), qPrintable(foot.name()));
            if (!(head.red() > head.green() + 40 && head.red() > head.blue() + 40)) {
                std::fprintf(stderr, "dj-ui: the master meter is not red at the top\n");
                return 1;
            }
            if (!(middle.red() > 150 && middle.green() > 120 && middle.blue() < 100)) {
                std::fprintf(stderr, "dj-ui: the master meter is not yellow under the red\n");
                return 1;
            }
            if (!(foot.green() > foot.red() + 20)) {
                std::fprintf(stderr, "dj-ui: the master meter is not green at the bottom\n");
                return 1;
            }
        }

        std::fprintf(stdout, "dj-ui: the master knob and meter work\n");
        return 0;
    }

    if (parser.isSet(curveCheckOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        QElapsedTimer linking;
        linking.start();
        while (!bridge->isConnected() && linking.elapsed() < 3000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(10);
        }
        if (!bridge->isConnected()) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        const QStringList laws = {QStringLiteral("constant_power"), QStringLiteral("linear"),
                                  QStringLiteral("smooth"), QStringLiteral("sharp")};
        const auto published = [&]() {
            return bridge->snapshot().value(QStringLiteral("crossfader_curve")).toString();
        };
        for (const QString &law : laws) {
            bridge->setCrossfaderCurve(law);
            QElapsedTimer waiting;
            waiting.start();
            QString seen = published();
            while (waiting.elapsed() < 3000 && seen != law) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                seen = published();
                QThread::msleep(10);
            }
            std::fprintf(stdout, "dj-ui: curve %s -> snapshot says %s\n", qPrintable(law),
                         qPrintable(seen.isEmpty() ? QStringLiteral("nothing") : seen));
            // The buttons must show the same law the engine published: exactly one highlighted.
            QCoreApplication::processEvents();
            QStringList lit;
            QStringList dark;
            for (const QString &candidate : laws) {
                auto *button = findItem(window->contentItem(),
                                        QStringLiteral("curveButton_") + candidate);
                if (button == nullptr) {
                    std::fprintf(stderr, "dj-ui: no button for %s\n", qPrintable(candidate));
                    return 1;
                }
                const QColor fill = button->property("color").value<QColor>();
                if (fill == QColor(QStringLiteral("#181818"))) {
                    lit.append(candidate);
                } else {
                    dark.append(candidate);
                }
            }
            std::fprintf(stdout, "dj-ui:   highlighted %s, plain %s\n",
                         qPrintable(lit.join(QStringLiteral(","))),
                         qPrintable(dark.join(QStringLiteral(","))));
            if (lit.size() != 1 || lit.first() != law) {
                std::fprintf(stderr, "dj-ui: the selector does not show the law in force\n");
                return 1;
            }
            if (seen != law) {
                std::fprintf(stderr, "dj-ui: the engine did not take the %s law\n", qPrintable(law));
                return 1;
            }
        }

        bridge->setCrossfaderCurve(QStringLiteral("constant_power"));

        // The cut-in law carries its own number: the engine must publish the object, the button must light,
        // and the field must show the number in force. A law that travelled without its value would open
        // somewhere other than the DJ sees.
        const auto cutInPublished = [&]() {
            const QVariant raw = bridge->snapshot().value(QStringLiteral("crossfader_curve"));
            const QVariantMap map = raw.toMap();
            if (!map.contains(QStringLiteral("cut"))) {
                return -1;
            }
            return map.value(QStringLiteral("cut")).toMap().value(QStringLiteral("cut_in")).toInt();
        };
        for (const int percent : {5, 7}) {
            bridge->setCrossfaderCurve(QStringLiteral("cut:%1").arg(percent));
            QElapsedTimer waiting;
            waiting.start();
            int seen = cutInPublished();
            while (waiting.elapsed() < 3000 && seen != percent) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                seen = cutInPublished();
                QThread::msleep(10);
            }
            QCoreApplication::processEvents();
            auto *button = findItem(window->contentItem(), QStringLiteral("curveButton_cut"));
            if (button == nullptr) {
                std::fprintf(stderr, "dj-ui: the curve row has no cut button\n");
                return 1;
            }
            const bool lit = button->property("lit").toBool();
            auto *value = findItem(window->contentItem(), QStringLiteral("cutInValue"));
            const QString shown =
                value == nullptr ? QStringLiteral("nothing") : value->property("text").toString();
            std::fprintf(stdout, "dj-ui: cut-in %d -> engine says %d, button %s, field says %s\n",
                         percent, seen, lit ? "lit" : "plain", qPrintable(shown));
            if (seen != percent) {
                std::fprintf(stderr, "dj-ui: the engine did not take the cut-in at %d percent\n", percent);
                return 1;
            }
            if (!lit) {
                std::fprintf(stderr, "dj-ui: the cut button does not show the law in force\n");
                return 1;
            }
            if (value == nullptr || !shown.startsWith(QString::number(percent))) {
                std::fprintf(stderr, "dj-ui: the cut-in field says \"%s\" while the engine has %d\n",
                             qPrintable(shown), percent);
                return 1;
            }
        }

        // Back to the default, so the next check in the set meets an ordinary law.
        bridge->setCrossfaderCurve(QStringLiteral("constant_power"));

        std::fprintf(stdout, "dj-ui: every crossfader law travels to the engine and back\n");
        return 0;
    }

    if (parser.isSet(windowCheckOption)) {
        if (!parser.isSet(sessionFileOption) || session.file().isEmpty()) {
            std::fprintf(stderr, "dj-ui: --window-check needs --session-file\n");
            return 1;
        }
        if (mainWindow == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        // The restore is applied from the event loop, once the window exists — that is how it behaves
        // on a real display. So the check waits for it instead of reading the shape a microsecond
        // after the scene was built.
        QElapsedTimer settling;
        settling.start();
        while (settling.elapsed() < 2000 && !mainWindow->isVisible()) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(5);
        }
        QCoreApplication::processEvents();

        const auto stateOf = [&]() {
            switch (mainWindow->visibility()) {
            case QQuickWindow::FullScreen:
                return QStringLiteral("fullscreen");
            case QQuickWindow::Maximized:
                return QStringLiteral("maximized");
            default:
                return QStringLiteral("window");
            }
        };
        const auto remembers = [&]() {
            QFile file(session.file());
            if (!file.open(QIODevice::ReadOnly)) {
                return QString();
            }
            const QJsonObject root = QJsonDocument::fromJson(file.readAll()).object();
            return root.value(QStringLiteral("window")).toString();
        };
        const auto pressF11 = [&]() {
            QKeyEvent press(QEvent::KeyPress, Qt::Key_F11, Qt::NoModifier);
            QCoreApplication::sendEvent(mainWindow, &press);
            QKeyEvent release(QEvent::KeyRelease, Qt::Key_F11, Qt::NoModifier);
            QCoreApplication::sendEvent(mainWindow, &release);
            QCoreApplication::processEvents();
        };

        const QString wanted = session.window();
        const QString opened = stateOf();
        const QString writtenAtStart = remembers();
        std::fprintf(stdout, "dj-ui: the window opened %s (wanted %s, file says %s)\n",
                     qPrintable(opened), qPrintable(wanted), qPrintable(writtenAtStart));
        if (opened != wanted) {
            std::fprintf(stderr, "dj-ui: the window shape was not restored\n");
            return 1;
        }
        // The scene shows a plain window while it loads; that must not be written over the memory.
        if (writtenAtStart != wanted) {
            std::fprintf(stderr, "dj-ui: the start overwrote the remembered shape\n");
            return 1;
        }
        // F11 toggles: from full screen it leaves — back to the shape remembered as its origin — and
        // from anything else it fills.
        const QString expected = opened == QLatin1String("fullscreen")
            ? (session.restoreTo().isEmpty() ? QStringLiteral("window") : session.restoreTo())
            : QStringLiteral("fullscreen");
        pressF11();
        const QString afterFirst = stateOf();
        const QString afterFirstFile = remembers();
        std::fprintf(stdout, "dj-ui: F11 -> %s, file says %s\n", qPrintable(afterFirst),
                     qPrintable(afterFirstFile));
        if (afterFirst != expected || afterFirstFile != expected) {
            std::fprintf(stderr, "dj-ui: F11 gave %s, wanted %s\n", qPrintable(afterFirst),
                         qPrintable(expected));
            return 1;
        }
        pressF11();
        const QString back = stateOf();
        const QString backFile = remembers();
        std::fprintf(stdout, "dj-ui: F11 again -> %s, file says %s\n", qPrintable(back),
                     qPrintable(backFile));
        if (back != opened || backFile != opened) {
            std::fprintf(stderr, "dj-ui: leaving full screen did not return to %s\n",
                         qPrintable(opened));
            return 1;
        }
        std::fprintf(stdout, "dj-ui: the window keeps its shape across F11 and a restart\n");
        return 0;
    }

    // A frame of the strip, timed: the same code the deck runs, painted into an image instead of a
    // window, so the number means something on a machine without a display.
    if (parser.isSet(benchOption)) {
        const QString path = parser.value(loadOption);
        Waveform envelope;
        if (path.isEmpty()) {
            // A five-minute track at one hundred columns per second: the size a real envelope has,
            // so the figure does not flatter a tiny fixture.
            envelope.loadSynthetic(30000);
        } else if (!envelope.load(path)) {
            std::fprintf(stderr, "dj-ui: %s\n", qPrintable(envelope.error()));
            return 1;
        }
        bool ok = false;
        const int frames = parser.value(benchOption).toInt(&ok);
        const int count = ok && frames > 0 ? frames : 200;
        QImage image(1200, 160, QImage::Format_RGB32);
        WaveformStyle style;
        style.background = QColor(QStringLiteral("#141414"));
        style.outside = QColor(QStringLiteral("#101010"));
        style.flat = QColor(QStringLiteral("#5b636d"));
        style.border = QColor(QStringLiteral("#383838"));
        style.accent = QColor(QStringLiteral("#5090f0"));
        style.marker = QColor(QStringLiteral("#e2c07f"));
        style.playhead = QColor(QStringLiteral("#f0f3f6"));
        style.barWidth = settings.waveformBarWidth();
        style.barGap = settings.waveformBarGap();

        std::vector<double> costs;
        costs.reserve(std::size_t(count));
        for (int frame = 0; frame < count; ++frame) {
            // Scroll across the track, the way a second of playback at fiftyfold zoom does.
            const double start = 0.5 * double(frame) / double(count);
            QElapsedTimer timer;
            timer.start();
            QPainter painter(&image);
            paintWaveform(painter, envelope, style, image.rect(), start, 0.1, start + 0.05, -1.0,
                          {}, {}, false, 0.0, 0.0, false, -1.0, {}, {});
            painter.end();
            costs.push_back(double(timer.nsecsElapsed()) / 1.0e6);
        }
        std::sort(costs.begin(), costs.end());
        const auto at = [&costs](double share) {
            const std::size_t index = std::size_t(qBound(0.0, share, 1.0) * double(costs.size() - 1));
            return costs[index];
        };
        std::fprintf(stdout,
                     "dj-ui: frame cost p50 %.2f ms, p99 %.2f ms, max %.2f ms over %d frames "
                     "at %d Hz (%d ms apart)\n",
                     at(0.50), at(0.99), costs.back(), count, settings.frameHz(),
                     settings.frameInterval());
        // A frame has a 16.7 ms budget at 60 Hz; taking more than half of it to paint the strip
        // means the strip, not the display, sets the frame rate.
        if (at(0.99) > 8.0) {
            std::fprintf(stderr, "dj-ui: a frame costs %.2f ms, too much to scroll smoothly\n",
                         at(0.99));
            return 1;
        }
        return 0;
    }

    // The arrangement, measured from the objects that were actually built: a layout setting that the
    // shell quietly ignores is worse than no setting at all.
    if (parser.isSet(layoutCheckOption)) {
        return runLayoutCheck(engine, layout, settings);
    }

    if (parser.isSet(wheelCheckOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        QElapsedTimer linking;
        linking.start();
        while (!bridge->isConnected() && linking.elapsed() < 3000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(10);
        }
        if (!bridge->isConnected()) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        QEventLoop settle;
        QTimer::singleShot(600, &settle, &QEventLoop::quit);
        settle.exec();

        // Two controls, two numbers: the wheel over the gain knob moves the trim and must leave the
        // fader alone. A single number shared by both was the defect this check exists for.
        auto *trimA = findItem(window->contentItem(), QStringLiteral("trim_A"));
        if (trimA == nullptr) {
            std::fprintf(stderr, "dj-ui: no gain knob for deck A\n");
            return 1;
        }
        const auto fieldOfA = [&](const QString &field) {
            const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant &entry : decks) {
                const QVariantMap deck = entry.toMap();
                if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    return deck.value(field).toDouble();
                }
            }
            return 0.0;
        };
        const double gainBefore = fieldOfA(QStringLiteral("linear_gain"));
        const double faderBefore = fieldOfA(QStringLiteral("fader"));
        const QPointF centre = trimA->mapToScene(QPointF(trimA->width() / 2.0,
                                                          trimA->height() / 2.0));
        for (int notch = 0; notch < 3; ++notch) {
            QWheelEvent event(centre, window->mapToGlobal(centre.toPoint()), QPoint(),
                              QPoint(0, 120), Qt::NoButton, Qt::NoModifier, Qt::NoScrollPhase, false);
            QCoreApplication::sendEvent(window, &event);
            QCoreApplication::processEvents();
            QThread::msleep(60);
        }
        QElapsedTimer waiting;
        waiting.start();
        double gainAfter = gainBefore;
        while (waiting.elapsed() < 3000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            gainAfter = fieldOfA(QStringLiteral("linear_gain"));
            if (std::abs(gainAfter - gainBefore) > 0.01) {
                break;
            }
            QThread::msleep(20);
        }
        const double faderAfter = fieldOfA(QStringLiteral("fader"));
        std::fprintf(stdout, "dj-ui: wheel over the gain knob: gain %.3f -> %.3f, fader %.3f\n",
                     gainBefore, gainAfter, faderAfter);
        if (std::abs(gainAfter - gainBefore) <= 0.01) {
            std::fprintf(stderr, "dj-ui: the wheel did not reach the engine\n");
            return 1;
        }
        if (std::abs(faderAfter - faderBefore) > 0.01) {
            std::fprintf(stderr, "dj-ui: the gain wheel moved the fader\n");
            return 1;
        }
        bridge->setChannelGain(QStringLiteral("A"), 1.0);

        // A notch is a fixed turn, not a fixed number of decibels: scroll one notch up and one back down
        // and the pointer must land where it started. The decibels need not come back — the two halves of
        // the law are asymmetric on purpose — which is why the angle is what is measured here.
        const auto pump = [&]() {
            for (int step = 0; step < 40; ++step) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 10);
                QThread::msleep(5);
            }
        };
        const auto wheelAngle = [&](int notches) {
            QWheelEvent event(centre, window->mapToGlobal(centre.toPoint()), QPoint(),
                              QPoint(0, 120 * notches), Qt::NoButton, Qt::NoModifier,
                              Qt::NoScrollPhase, false);
            QCoreApplication::sendEvent(window, &event);
            pump();
            return trimA->property("angle").toDouble();
        };
        pump();
        const double angleAtRest = trimA->property("angle").toDouble();
        const double angleUp = wheelAngle(1);
        const double angleDown = wheelAngle(-1);
        const double angleBack = wheelAngle(-1);
        const double angleUpAgain = wheelAngle(1);
        std::fprintf(stdout,
                     "dj-ui: one notch turns the gain knob %+.1f deg up and %+.1f deg down (back at "
                     "%.1f and %.1f deg)\n",
                     angleUp - angleAtRest, angleBack - angleDown, angleDown, angleUpAgain);
        if (std::abs((angleUp - angleAtRest) - 13.5) > 1.0
            || std::abs((angleBack - angleDown) + 13.5) > 1.0) {
            std::fprintf(stderr, "dj-ui: the wheel turns the gain knob by different steps each way\n");
            return 1;
        }
        std::fprintf(stdout, "dj-ui: the wheel trims the gain and leaves the fader where it was\n");
        return 0;
    }

    if (parser.isSet(gainCurveCheckOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        // The knobs are drawn at the size a person sees: the law is measured in pixels of hand.
        window->resize(1366, 768);
        window->show();
        QCoreApplication::processEvents(QEventLoop::AllEvents, 100);
        QEventLoop settle;
        QTimer::singleShot(200, &settle, &QEventLoop::quit);
        settle.exec();
        QQuickItem *content = window->contentItem();
        auto *trim = findItem(content, QStringLiteral("trim_A"));
        auto *master = findItem(content, QStringLiteral("masterKnob"));
        if (trim == nullptr || master == nullptr) {
            std::fprintf(stderr, "dj-ui: no level knobs in the mixer\n");
            return 1;
        }
        // Both knobs answer the same questions, which is what makes one check enough.
        // A QML function takes and returns `QVariant`, not `double`, whatever it is written as.
        const auto ask = [&](QQuickItem *knob, const char *name, double argument, double *out) {
            QVariant answer;
            if (!QMetaObject::invokeMethod(knob, name, Q_RETURN_ARG(QVariant, answer),
                                           Q_ARG(QVariant, QVariant(argument)))) {
                std::fprintf(stderr, "dj-ui: the knob does not answer %s\n", name);
                return false;
            }
            *out = answer.toDouble();
            return true;
        };
        const double capSide = trim->property("capSide").toDouble();
        const double degreesPerPixel = 270.0 / capSide;
        // A rate, not an average: a hair of a degree and a hundredth of a pixel, so the numbers below are
        // the slope at a spot rather than a chord across a fifth of the travel. The chord is what made an
        // earlier version of this check pass for the wrong shape.
        const double hair = 0.01;
        const double trimRange = 40.0;
        const double masterRange = 60.0;
        const double masterTravel = master->property("travelPixels").toDouble();
        // The trim: ends exact, then the slope at unity and at the very end of the cut.
        double unity = 0.0;
        double bottom = 0.0;
        double top = 0.0;
        double firstHair = 0.0;
        double lastHairBefore = 0.0;
        double lastHair = 0.0;
        if (!ask(trim, "dbOf", 0.0, &unity) || !ask(trim, "dbOf", -135.0, &bottom)
            || !ask(trim, "dbOf", 135.0, &top) || !ask(trim, "dbOf", -hair, &firstHair)
            || !ask(trim, "dbOf", -135.0 + hair, &lastHairBefore)
            || !ask(trim, "dbOf", -135.0, &lastHair)) {
            return 1;
        }
        const double trimAtUnity = std::abs(firstHair) / hair * degreesPerPixel;
        const double trimAtEnd = std::abs(lastHair - lastHairBefore) / hair * degreesPerPixel;
        // The master drags in pixels, so its law takes two arguments.
        const auto drag = [&](double pixels, double fromDb, double *out) {
            QVariant answer;
            if (!QMetaObject::invokeMethod(master, "dbForPixels", Q_RETURN_ARG(QVariant, answer),
                                           Q_ARG(QVariant, QVariant(pixels)),
                                           Q_ARG(QVariant, QVariant(fromDb)))) {
                std::fprintf(stderr, "dj-ui: the master does not answer dbForPixels\n");
                return false;
            }
            *out = answer.toDouble();
            return true;
        };
        double masterFirst = 0.0;
        double masterLastBefore = 0.0;
        double masterLast = 0.0;
        if (!drag(0.01, 0.0, &masterFirst) || !drag(masterTravel - 0.01, 0.0, &masterLastBefore)
            || !drag(masterTravel, 0.0, &masterLast)) {
            return 1;
        }
        const double masterAtUnity = std::abs(masterFirst) / 0.01;
        const double masterAtEnd = std::abs(masterLast - masterLastBefore) / 0.01;
        std::fprintf(stdout,
                     "dj-ui: the gain curve is %s at %.2f; the trim moves %.3f dB per pixel at unity "
                     "and %.3f at the end of the cut; the master moves %.3f dB per pixel at unity and "
                     "%.3f at the bottom\n",
                     qPrintable(settings.gainCurve()), settings.gainCurveMiddle(), trimAtUnity,
                     trimAtEnd, masterAtUnity, masterAtEnd);
        if (std::abs(unity) > 0.01 || std::abs(bottom + 40.0) > 0.01 || std::abs(top - 6.0) > 0.01) {
            std::fprintf(stderr,
                         "dj-ui: the ends of the trim are not where they were: %.2f / %.2f / %.2f\n",
                         unity, bottom, top);
            return 1;
        }
        // What the law promises, whatever the configured shape: the straight rate scaled by the
        // configured share at unity, and by the rest of the average slope at the end.
        const double middle = settings.gainCurveMiddle();
        const bool curved = settings.gainCurve() == QStringLiteral("cubic");
        const double promisedTrim = trimRange * (curved ? middle : 1.0) / 135.0 * degreesPerPixel;
        const double promisedTrimEnd =
            trimRange * (curved ? 2.0 - middle : 1.0) / 135.0 * degreesPerPixel;
        const double promisedMaster =
            masterRange * (curved ? middle : 1.0) / masterTravel;
        const auto near = [](double measured, double promised) {
            return std::abs(measured - promised) <= std::max(1e-4, promised * 0.05);
        };
        if (!near(trimAtUnity, promisedTrim)) {
            std::fprintf(stderr,
                         "dj-ui: the trim moves %.3f dB per pixel at unity, and the curve promises "
                         "%.3f\n",
                         trimAtUnity, promisedTrim);
            return 1;
        }
        if (!near(trimAtEnd, promisedTrimEnd)) {
            std::fprintf(stderr,
                         "dj-ui: the trim moves %.3f dB per pixel at the end of the cut, and the "
                         "curve promises %.3f\n",
                         trimAtEnd, promisedTrimEnd);
            return 1;
        }
        if (!near(masterAtUnity, promisedMaster)) {
            std::fprintf(stderr,
                         "dj-ui: the master moves %.3f dB per pixel at unity, and the curve promises "
                         "%.3f\n",
                         masterAtUnity, promisedMaster);
            return 1;
        }
        // The channel faders: same family, their own middle, and the ends fixed at unity and silence.
        auto *fader = findItem(content, QStringLiteral("fader_A"));
        if (fader == nullptr) {
            std::fprintf(stderr, "dj-ui: no channel fader in the mixer\n");
            return 1;
        }
        double faderTop = 0.0;
        double faderBottom = 0.0;
        double faderFirst = 0.0;
        double faderLastBefore = 0.0;
        if (!ask(fader, "valueAtHandle", 1.0, &faderTop) || !ask(fader, "valueAtHandle", 0.0, &faderBottom)
            || !ask(fader, "valueAtHandle", 1.0 - hair, &faderFirst)
            || !ask(fader, "valueAtHandle", hair, &faderLastBefore)) {
            return 1;
        }
        const double faderAtTop = (faderTop - faderFirst) / hair;
        const double faderAtBottom = faderLastBefore / hair;
        // The fader's own shape, not the knobs': the two are configured apart, and a check that reads the
        // wrong one passes for the wrong reason.
        const bool faderCubic = settings.faderCurve() == QStringLiteral("cubic");
        const double promisedFader = faderCubic ? settings.faderCurveMiddle() : 1.0;
        const double promisedFaderBottom = faderCubic ? 2.0 - settings.faderCurveMiddle() : 1.0;
        std::fprintf(stdout,
                     "dj-ui: the channel fader moves %.3f of its range per unit of travel at the top "
                     "(curve %s at %.2f) and %.3f at the bottom\n",
                     faderAtTop, qPrintable(settings.faderCurve()), settings.faderCurveMiddle(),
                     faderAtBottom);
        if (std::abs(faderTop - 1.0) > 0.001 || std::abs(faderBottom) > 0.001) {
            std::fprintf(stderr,
                         "dj-ui: the fader's ends are not unity and silence: %.3f / %.3f\n",
                         faderTop, faderBottom);
            return 1;
        }
        const bool logarithmic = settings.faderCurve() == QStringLiteral("logarithmic");
        if (logarithmic) {
            // A B-taper is a promise about decibels, not about slopes: a quarter of the throw takes the
            // level down about ten decibels and half of it about eighteen, which is what makes the top
            // third of a fader the fine one.
            double quarter = 0.0;
            double half = 0.0;
            if (!ask(fader, "valueAtHandle", 0.75, &quarter) || !ask(fader, "valueAtHandle", 0.5, &half)) {
                return 1;
            }
            const double quarterDb = 20.0 * std::log10(std::max(quarter, 1e-9));
            const double halfDb = 20.0 * std::log10(std::max(half, 1e-9));
            std::fprintf(stdout,
                         "dj-ui: the fader at a quarter of the throw is %.1f dB, at half %.1f dB, and "
                         "it moves %.2f times as fast at the top as at the bottom\n",
                         quarterDb, halfDb, faderAtBottom > 0.0 ? faderAtTop / faderAtBottom : 0.0);
            if (quarterDb > -8.0 || quarterDb < -14.0) {
                std::fprintf(stderr,
                             "dj-ui: a quarter of the fader's throw takes only %.1f dB, so the top is not "
                             "the fine part\n",
                             quarterDb);
                return 1;
            }
            if (halfDb > -14.0 || halfDb < -24.0) {
                std::fprintf(stderr, "dj-ui: half the throw takes %.1f dB\n", halfDb);
                return 1;
            }
            if (!(faderAtTop > faderAtBottom * 3.0)) {
                std::fprintf(stderr,
                             "dj-ui: the fader is not steeper at the top (%.3f against %.3f per unit of "
                             "travel)\n",
                             faderAtTop, faderAtBottom);
                return 1;
            }
        } else if (!near(faderAtTop, promisedFader) || !near(faderAtBottom, promisedFaderBottom)) {
            std::fprintf(stderr,
                         "dj-ui: the fader moves %.3f at the top and %.3f at the bottom, and the curve "
                         "promises %.3f and %.3f\n",
                         faderAtTop, faderAtBottom, promisedFader, promisedFaderBottom);
            return 1;
        }
        if (curved && !(trimAtEnd > trimAtUnity * 1.3)) {
            std::fprintf(stderr,
                         "dj-ui: the end of the trim is not faster than its middle (%.3f against "
                         "%.3f dB per pixel)\n",
                         trimAtEnd, trimAtUnity);
            return 1;
        }
        std::fprintf(stdout, "dj-ui: the level knobs follow the %s curve\n",
                     qPrintable(settings.gainCurve()));
        return 0;
    }

    if (parser.isSet(tempoCheckOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        const QString fixture = parser.value(loadOption);
        if (fixture.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --tempo-check needs --load\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        QElapsedTimer linking;
        linking.start();
        while (!bridge->isConnected() && linking.elapsed() < 3000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(10);
        }
        if (!bridge->isConnected()) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        const auto deckA = [&]() {
            const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant &entry : decks) {
                const QVariantMap deck = entry.toMap();
                if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    return deck;
                }
            }
            return QVariantMap();
        };
        bridge->load(QStringLiteral("A"), fixture);
        double bpm = 0.0;
        quint64 firstBeat = 0;
        // The loader decodes the file twice (level and tempo) before it says Ready, so the wait is
        // generous on purpose: a slow first load is not a defect.
        bool measured = false;
        QElapsedTimer waiting;
        waiting.start();
        while (waiting.elapsed() < 20000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            const QVariantMap track = deckA().value(QStringLiteral("track")).toMap();
            const QVariant tempo = track.value(QStringLiteral("bpm"));
            if (tempo.isValid() && !tempo.isNull()) {
                bpm = tempo.toDouble();
                firstBeat = track.value(QStringLiteral("first_beat_frame")).toULongLong();
                measured = true;
                break;
            }
            QThread::msleep(10);
        }
        if (!measured) {
            std::fprintf(stderr, "dj-ui: the engine measured no tempo\n");
            return 1;
        }
        // What the DJ sees, not only what the snapshot holds: the label is part of the feature.
        // The label is a QQuickItem, not a plain QObject child: the scene is a tree of items, and a
        // `findChild` on the window finds the C++ wrappers only by accident.
        auto *label = findItem(window->contentItem(), QStringLiteral("bpm_A"));
        const QString shown = label != nullptr ? label->property("text").toString() : QString();
        std::fprintf(stdout, "dj-ui: tempo %.2f BPM, first beat %llu, label \"%s\"\n", bpm,
                     static_cast<unsigned long long>(firstBeat), qPrintable(shown));
        if (bpm <= 0.0 || firstBeat == 0) {
            std::fprintf(stderr, "dj-ui: the deck published no usable grid\n");
            return 1;
        }
        if (!shown.contains(QStringLiteral("BPM"))) {
            std::fprintf(stderr, "dj-ui: the deck shows no tempo (label \"%s\")\n",
                         qPrintable(shown));
            return 1;
        }
        std::fprintf(stdout, "dj-ui: the measured tempo reached the deck\n");
        return 0;
    }

    if (parser.isSet(loopBeatsOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        const QString fixture = parser.value(loadOption);
        if (fixture.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --loop-beats-check needs --load\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        const auto waitFor = [](const std::function<bool()> &ready, int budget) {
            QElapsedTimer timer;
            timer.start();
            while (timer.elapsed() < budget) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                if (ready()) {
                    return true;
                }
                QThread::msleep(10);
            }
            return ready();
        };
        if (!waitFor([&]() { return bridge->isConnected(); }, 3000)) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        const auto deckA = [&]() {
            const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant &entry : decks) {
                const QVariantMap deck = entry.toMap();
                if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    return deck;
                }
            }
            return QVariantMap();
        };
        const auto gridFirstBeat = [&]() -> qint64 {
            const QVariantList segments = deckA()
                                              .value(QStringLiteral("track"))
                                              .toMap()
                                              .value(QStringLiteral("grid"))
                                              .toMap()
                                              .value(QStringLiteral("segments"))
                                              .toList();
            if (segments.isEmpty()) {
                return -1;
            }
            return segments.first().toMap().value(QStringLiteral("frame")).toLongLong();
        };
        const auto loopRegion = [&]() {
            return deckA().value(QStringLiteral("loop_region")).toMap();
        };
        // A Rectangle with a MouseArea has no `clicked` signal: the press is a mouse event at its centre.
        const auto clickAt = [](QQuickItem *item) {
            const QPointF centre = item->mapToScene(QPointF(item->width() / 2.0, item->height() / 2.0));
            QMouseEvent press(QEvent::MouseButtonPress, centre, centre, Qt::LeftButton,
                              Qt::LeftButton, Qt::NoModifier);
            QMouseEvent release(QEvent::MouseButtonRelease, centre, centre, Qt::LeftButton,
                                Qt::NoButton, Qt::NoModifier);
            QCoreApplication::sendEvent(item->window(), &press);
            QCoreApplication::sendEvent(item->window(), &release);
            QCoreApplication::processEvents();
        };
        const auto click = [&](const QString &name) {
            auto *item = findItem(window->contentItem(), name);
            if (item == nullptr) {
                std::fprintf(stderr, "dj-ui: no %s to press\n", qPrintable(name));
                return false;
            }
            QMetaObject::invokeMethod(item, "clicked");
            QCoreApplication::processEvents();
            return true;
        };

        bridge->load(QStringLiteral("A"), fixture);
        const QString wantName = QFileInfo(fixture).fileName();
        if (!waitFor(
                [&]() {
                    const QString held = deckA().value(QStringLiteral("path")).toString();
                    return !held.isEmpty() && held.endsWith(wantName) && gridFirstBeat() >= 0;
                },
                20000)) {
            std::fprintf(stderr, "dj-ui: the deck published no grid to loop by\n");
            return 1;
        }
        // The grid's own numbers, not the deck's headline tempo: the loop is built from the segment the
        // play head is in, and the check has to measure the same thing the deck did.
        const QVariantList segments = deckA()
                                          .value(QStringLiteral("track"))
                                          .toMap()
                                          .value(QStringLiteral("grid"))
                                          .toMap()
                                          .value(QStringLiteral("segments"))
                                          .toList();
        if (segments.isEmpty()) {
            std::fprintf(stderr, "dj-ui: the deck published no grid segments\n");
            return 1;
        }
        const QVariantMap first = segments.first().toMap();
        const qint64 rate = deckA().value(QStringLiteral("source_rate")).toLongLong();
        const double bpm = first.value(QStringLiteral("bpm")).toDouble();
        if (rate <= 0 || !(bpm > 0.0)) {
            std::fprintf(stderr, "dj-ui: the deck published no rate or tempo\n");
            return 1;
        }
        const double period = rate * 60.0 / bpm;
        const qint64 anchor = first.value(QStringLiteral("frame")).toLongLong();
        const auto onGrid = [&](qint64 frame) {
            const double lines = (static_cast<double>(frame) - static_cast<double>(anchor)) / period;
            const double off = std::fabs(lines - std::round(lines)) * period;
            return off <= 2.0;
        };
        // Snap and quantize, on a pair of presses made off the grid on purpose: the start goes to the
        // nearest line and the end is a whole number of beats after it (DJP-DECK-005).
        const auto seekTo = [&](qint64 frame) {
            bridge->seek(QStringLiteral("A"), static_cast<qulonglong>(frame));
            return waitFor([&]() {
                return std::llabs(deckA().value(QStringLiteral("source_frame")).toLongLong() - frame)
                    <= 2;
            }, 5000);
        };
        const auto loopStart = [&]() -> qint64 {
            const QVariantMap region = loopRegion();
            return region.isEmpty() ? -1 : region.value(QStringLiteral("start")).toLongLong();
        };
        const qint64 markFrom = anchor + static_cast<qint64>(period * 3.0) + 137;
        const qint64 markTo = anchor + static_cast<qint64>(period * 6.4) - 211;
        if (!seekTo(markFrom)) {
            std::fprintf(stderr, "dj-ui: the play head did not reach the snap mark\n");
            return 1;
        }
        auto *panelItem = findItem(window->contentItem(), QStringLiteral("deckPanel_A"));
        if (panelItem == nullptr) {
            std::fprintf(stderr, "dj-ui: no deck panel to press Loop on\n");
            return 1;
        }
        const auto pressLoop = [&]() {
            QMetaObject::invokeMethod(panelItem, "pressLoop", Q_ARG(QVariant, QVariant(false)));
            QCoreApplication::processEvents();
            return true;
        };
        if (!pressLoop()) {
            return 1;
        }
        if (!seekTo(markTo)) {
            std::fprintf(stderr, "dj-ui: the play head did not reach the quantize mark\n");
            return 1;
        }
        if (!pressLoop()) {
            return 1;
        }
        if (!waitFor([&]() { return loopStart() >= 0; }, 5000)) {
            std::fprintf(stderr, "dj-ui: the two presses made no loop\n");
            return 1;
        }
        const qint64 snappedStart = loopStart();
        const qint64 snappedEnd = loopRegion().value(QStringLiteral("end")).toLongLong();
        if (!onGrid(snappedStart) || !onGrid(snappedEnd)) {
            std::fprintf(stderr, "dj-ui: the loop edges are off the grid: %lld..%lld (anchor %lld, "
                                 "period %.1f)\n",
                         snappedStart, snappedEnd, anchor, period);
            return 1;
        }
        const double beats = static_cast<double>(snappedEnd - snappedStart) / period;
        if (std::fabs(beats - std::round(beats)) > 0.02 || std::round(beats) < 1.0) {
            std::fprintf(stderr, "dj-ui: the loop is %.3f beats long, not a whole number\n", beats);
            return 1;
        }
        const auto snappedBeats = static_cast<qint64>(std::llround(beats));
        // The sizes and the switches live in the strip that folds, so the DJ opens it first: the check
        // does the same thing (owner's decision, 14.11).
        if (!click(QStringLiteral("loopJumpToggle_A"))) {
            return 1;
        }
        // The layout needs a frame before the strip's own buttons can be hit.
        for (int tick = 0; tick < 40; ++tick) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(10);
        }
        // The switches are the DJ's: turned off, the very same pair must land where the hand was and not
        // on the grid. A switch that only looks lit is worse than none.
        // The switches are global, in the mixer: the modes are one decision for the set, and the decks
        // read them (owner's decision, 14.11).
        auto *snapSwitch = findItem(window->contentItem(), QStringLiteral("snapGlobal"));
        auto *quantSwitch = findItem(window->contentItem(), QStringLiteral("quantizeGlobal"));
        if (snapSwitch == nullptr || quantSwitch == nullptr) {
            std::fprintf(stderr, "dj-ui: no snap or quantize switch\n");
            return 1;
        }
        if (!snapSwitch->property("on").toBool() || !quantSwitch->property("on").toBool()) {
            std::fprintf(stderr, "dj-ui: the switches do not start on\n");
            return 1;
        }
        clickAt(snapSwitch);
        clickAt(quantSwitch);
        if (snapSwitch->property("on").toBool() || quantSwitch->property("on").toBool()) {
            std::fprintf(stderr, "dj-ui: the switches did not turn off\n");
            return 1;
        }
        // Clear it, so the size loop below is not refused for crossing this one.
        bridge->setLoop(QStringLiteral("A"), false, 0, 0);
        if (!waitFor([&]() { return loopStart() < 0; }, 5000)) {
            std::fprintf(stderr, "dj-ui: the loop outlived the clear\n");
            return 1;
        }
        const qint64 rawFrom = anchor + static_cast<qint64>(period * 2.3) + 211;
        const qint64 rawTo = anchor + static_cast<qint64>(period * 5.7) - 173;
        if (!seekTo(rawFrom)) {
            return 1;
        }
        if (!pressLoop()) {
            return 1;
        }
        if (!seekTo(rawTo)) {
            return 1;
        }
        if (!pressLoop()) {
            return 1;
        }
        if (!waitFor([&]() { return loopStart() >= 0; }, 5000)) {
            std::fprintf(stderr, "dj-ui: the two presses with the switches off made no loop\n");
            return 1;
        }
        const qint64 rawStart = loopStart();
        if (onGrid(rawStart)) {
            std::fprintf(stderr,
                         "dj-ui: the loop snapped to %lld with snap and quantize off — the switches do "
                         "nothing\n",
                         rawStart);
            return 1;
        }
        bridge->setLoop(QStringLiteral("A"), false, 0, 0);
        if (!waitFor([&]() { return loopStart() < 0; }, 5000)) {
            return 1;
        }
        // Back on for the rest of the check, as a deck starts.
        clickAt(snapSwitch);
        clickAt(quantSwitch);
        const auto four = static_cast<qint64>(std::llround(4.0 * period));
        if (!click(QStringLiteral("loopSize_A_4"))) {
            return 1;
        }
        const auto loopLength = [&]() -> qint64 {
            const QVariantMap region = loopRegion();
            if (region.isEmpty()) {
                return -1;
            }
            return region.value(QStringLiteral("end")).toLongLong()
                 - region.value(QStringLiteral("start")).toLongLong();
        };
        if (!waitFor([&]() { return std::llabs(loopLength() - four) <= 2; }, 5000)) {
            std::fprintf(stderr, "dj-ui: a four beat loop came back %lld frames long, wanted %lld (start %s)\n",
                         loopLength(), four, qPrintable(loopRegion().value(QStringLiteral("start")).toString()));
            return 1;
        }
        const qint64 start = loopRegion().value(QStringLiteral("start")).toLongLong();
        // The start has to be on a beat of the grid, not on the play head: that is the whole point.
        const double where = std::fmod(static_cast<double>(start - gridFirstBeat()), period);
        if (where > 2.0 && period - where > 2.0) {
            std::fprintf(stderr, "dj-ui: the loop starts %.1f frames off the beat\n", where);
            return 1;
        }
        if (!click(QStringLiteral("loopOn_A"))) {
            return 1;
        }
        const auto beatStep = static_cast<qint64>(std::llround(period));
        if (!waitFor(
                [&]() {
                    const qint64 moved = loopRegion().value(QStringLiteral("start")).toLongLong()
                                       - start;
                    return std::llabs(moved - beatStep) <= 2
                        && std::llabs(loopLength() - four) <= 2;
                },
                5000)) {
            std::fprintf(stderr, "dj-ui: the arrow moved the loop to frame %s, and it is %lld long\n",
                         qPrintable(loopRegion().value(QStringLiteral("start")).toString()),
                         loopLength());
            return 1;
        }
        // Every loop is the same loop: the one the size button made has to light the Loop button, and a
        // press inside it has to put the mode out — that is how a DJ leaves a loop.
        auto *loopButton = findItem(window->contentItem(), QStringLiteral("loop_A"));
        if (loopButton == nullptr) {
            std::fprintf(stderr, "dj-ui: no Loop button to read\n");
            return 1;
        }
        if (!waitFor([&]() { return deckA().value(QStringLiteral("loop_mode")).toBool(); }, 5000)) {
            std::fprintf(stderr, "dj-ui: the loop did not put the mode on\n");
            return 1;
        }
        if (!loopButton->property("active").toBool()) {
            std::fprintf(stderr, "dj-ui: the Loop button is not lit over an active loop\n");
            return 1;
        }
        if (!pressLoop()) {
            return 1;
        }
        if (!waitFor(
                [&]() {
                    return !deckA().value(QStringLiteral("loop_mode")).toBool()
                        && !loopButton->property("active").toBool();
                },
                5000)) {
            std::fprintf(stderr, "dj-ui: a press on the lit Loop button left the mode on\n");
            return 1;
        }
        std::fprintf(stdout,
                     "dj-ui: a four beat loop is %lld frames of the grid (a beat is %.1f), it starts on a "
                     "beat at frame %lld, and one press of the arrow moved it %.1f frames without "
                     "changing its length; two presses off the grid snapped to %lld..%lld (%lld whole "
                     "beats), with the switches off the same pair landed on %lld (%s the grid), the size "
                     "loop lit the Loop button, and a press inside it put the mode out\n",
                     four, period, start, static_cast<double>(beatStep), snappedStart, snappedEnd,
                     snappedBeats, rawStart, onGrid(rawStart) ? "on" : "off");
        return 0;
    }

    if (parser.isSet(transportExtrasOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        const QString fixture = parser.value(loadOption);
        if (fixture.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --transport-extras-check needs --load\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        const auto waitFor = [](const std::function<bool()> &ready, int budget) {
            QElapsedTimer timer;
            timer.start();
            while (timer.elapsed() < budget) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                if (ready()) {
                    return true;
                }
                QThread::msleep(10);
            }
            return ready();
        };
        if (!waitFor([&]() { return bridge->isConnected(); }, 3000)) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        const auto deckA = [&]() {
            const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant &entry : decks) {
                const QVariantMap deck = entry.toMap();
                if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    return deck;
                }
            }
            return QVariantMap();
        };
        const auto firstBeat = [&]() -> qint64 {
            const QVariantList segments = deckA()
                                              .value(QStringLiteral("track"))
                                              .toMap()
                                              .value(QStringLiteral("grid"))
                                              .toMap()
                                              .value(QStringLiteral("segments"))
                                              .toList();
            if (segments.isEmpty()) {
                return -1;
            }
            return segments.first().toMap().value(QStringLiteral("frame")).toLongLong();
        };
        const auto playHead = [&]() -> qint64 {
            return deckA().value(QStringLiteral("source_frame")).toLongLong();
        };
        const auto press = [&](const QString &name) {
            auto *item = findItem(window->contentItem(), name);
            if (item == nullptr) {
                std::fprintf(stderr, "dj-ui: no %s to press\n", qPrintable(name));
                return false;
            }
            QMetaObject::invokeMethod(item, "clicked");
            QCoreApplication::processEvents();
            return true;
        };

        bridge->load(QStringLiteral("A"), fixture);
        const QString wantName = QFileInfo(fixture).fileName();
        if (!waitFor(
                [&]() {
                    const QString held = deckA().value(QStringLiteral("path")).toString();
                    return !held.isEmpty() && held.endsWith(wantName) && firstBeat() >= 0;
                },
                20000)) {
            std::fprintf(stderr, "dj-ui: the deck published no grid to jump on\n");
            return 1;
        }
        const qint64 rate = deckA().value(QStringLiteral("source_rate")).toLongLong();
        const double bpm = deckA().value(QStringLiteral("local_bpm")).toDouble();
        if (rate <= 0 || !(bpm > 0.0)) {
            std::fprintf(stderr, "dj-ui: the deck published no rate or tempo\n");
            return 1;
        }
        const double period = rate * 60.0 / bpm;
        const qint64 first = firstBeat();
        // Between two beats and not on a round frame: a jump that only looks right from a beat proves
        // nothing about landing on one.
        const qint64 start = first + static_cast<qint64>(period * 1.5) + 311;
        bridge->seek(QStringLiteral("A"), start);
        if (!waitFor([&]() { return std::llabs(playHead() - start) <= 2; }, 5000)) {
            std::fprintf(stderr, "dj-ui: the play head did not reach frame %lld (%lld)\n", start,
                         playHead());
            return 1;
        }
        // Every length the panel promises is a button: twelve of them, and a missing one is a jump a DJ
        // cannot make.
        for (const QString &name : {QStringLiteral("m64"), QStringLiteral("m32"), QStringLiteral("m16"),
                                    QStringLiteral("m8"), QStringLiteral("m4"), QStringLiteral("m1"),
                                    QStringLiteral("p1"), QStringLiteral("p4"), QStringLiteral("p8"),
                                    QStringLiteral("p16"), QStringLiteral("p32"), QStringLiteral("p64")}) {
            if (findItem(window->contentItem(), QStringLiteral("jump_A_") + name) == nullptr) {
                std::fprintf(stderr, "dj-ui: the panel has no jump button for %s\n", qPrintable(name));
                return 1;
            }
        }
        if (!press(QStringLiteral("jump_A_p1"))) {
            return 1;
        }
        const auto onTheGrid = [&]() {
            const double place = static_cast<double>(playHead() - first) / period;
            return std::fabs(place - std::round(place)) * period <= 2.0;
        };
        if (!waitFor([&]() { return playHead() > start && onTheGrid(); }, 5000)) {
            std::fprintf(stderr,
                         "dj-ui: the jump did not land on a beat (start %lld, head %lld, period "
                         "%.1f)\n",
                         start, playHead(), period);
            return 1;
        }
        const qint64 jump = playHead() - start;
        // Slip: on, the state carries a virtual head; off, it carries none.
        if (!press(QStringLiteral("slip_A"))) {
            return 1;
        }
        if (!waitFor(
                [&]() {
                    const QVariantMap deck = deckA();
                    return deck.value(QStringLiteral("slip")).toBool()
                           && deck.value(QStringLiteral("slip_frame")).isValid();
                },
                3000)) {
            std::fprintf(stderr, "dj-ui: slip did not reach the state\n");
            return 1;
        }
        // The virtual head has to be on the picture, not only in the state: a jump under slip moves the
        // sound and leaves the virtual head behind, and both strips have to show that difference.
        const qint64 beforeSlipJump = playHead();
        if (!press(QStringLiteral("jump_A_p1"))) {
            return 1;
        }
        if (!waitFor([&]() { return playHead() != beforeSlipJump; }, 5000)) {
            std::fprintf(stderr, "dj-ui: the jump under slip did not move the sound\n");
            return 1;
        }
        auto *overview = findItem(window->contentItem(), QStringLiteral("overviewStrip_A"));
        if (overview == nullptr) {
            std::fprintf(stderr, "dj-ui: the overview strip is not there to draw the virtual head\n");
            return 1;
        }
        if (overview->property("drawnSlip").toInt() != 1) {
            std::fprintf(stderr, "dj-ui: the overview drew no virtual head (slip %.4f)\n",
                         overview->property("slipRatio").toDouble());
            return 1;
        }
        const double slipMark = overview->property("slipRatio").toDouble();
        const double heardMark = overview->property("playheadRatio").toDouble();
        if (std::fabs(slipMark - heardMark) < 1e-4) {
            std::fprintf(stderr,
                         "dj-ui: the virtual head sits on the play head after a jump (both %.4f)\n",
                         slipMark);
            return 1;
        }
        auto *strip = findItem(window->contentItem(), QStringLiteral("waveStrip_A"));
        if (strip == nullptr || strip->property("slipRatio").toDouble() < 0.0) {
            std::fprintf(stderr, "dj-ui: the main strip was never told where the virtual head is\n");
            return 1;
        }
        if (!press(QStringLiteral("slip_A"))) {
            return 1;
        }
        if (!waitFor(
                [&]() { return !deckA().value(QStringLiteral("slip")).toBool(); }, 3000)) {
            std::fprintf(stderr, "dj-ui: slip stayed on after the second press\n");
            return 1;
        }
        // Reverse: the button exists and the material turns around under it. A track still streaming
        // from disk refuses — the deck says so in its state and the head keeps going forward — so the
        // press is repeated until the entry is in the cache, and only then is the head expected to
        // walk down. Nothing here reads a timer into the verdict: the state is the verdict.
        if (findItem(window->contentItem(), QStringLiteral("reverse_A")) == nullptr) {
            std::fprintf(stderr, "dj-ui: the panel has no reverse button\n");
            return 1;
        }
        // Reverse needs a moving deck with material on both sides of the head. Whether the deck is
        // paused has nothing to do with reverse, so the check puts it in a state where the question is
        // only about reverse: somewhere in the middle of the file, playing.
        const qint64 target = first + static_cast<qint64>(period * 8.0);
        bridge->seek(QStringLiteral("A"), target);
        waitFor([&]() { return std::llabs(playHead() - target) <= 2; }, 5000);
        if (!deckA().value(QStringLiteral("playing")).toBool() && !press(QStringLiteral("play_A"))) {
            return 1;
        }
        if (!waitFor([&]() { return playHead() > target; }, 4000)) {
            std::fprintf(stderr, "dj-ui: the deck is not moving before the reverse test (%lld)\n",
                         playHead());
            return 1;
        }
        // The cache: the engine holds this track in memory, and the deck says so in its own state.
        // The handover happens a few seconds after the load (the entry is built behind the deck), so
        // the check waits instead of reading a single snapshot.
        if (!waitFor(
                [&]() {
                    return deckA().value(QStringLiteral("cached")).toBool()
                        && deckA().value(QStringLiteral("cached_bytes")).toDouble() > 0.0;
                },
                20000)) {
            std::fprintf(stderr,
                         "dj-ui: the deck never reported its material in memory (cache off?)\n");
            return 1;
        }
        const double cachedMb =
            deckA().value(QStringLiteral("cached_bytes")).toDouble() / (1024.0 * 1024.0);
        const double budgetMb = bridge->snapshot()
                                    .value(QStringLiteral("cache_budget_bytes"))
                                    .toDouble()
            / (1024.0 * 1024.0);
        std::fprintf(stdout,
                     "dj-ui: the track is in memory: %.1f MB of the %.0f MB budget (%.1f%%)\n",
                     cachedMb, budgetMb, budgetMb > 0.0 ? 100.0 * cachedMb / budgetMb : 0.0);
        const qint64 beforeReverse = playHead();
        bool accepted = false;
        QElapsedTimer reverseWait;
        reverseWait.start();
        while (reverseWait.elapsed() < 12'000) {
            if (!press(QStringLiteral("reverse_A"))) {
                return 1;
            }
            if (waitFor(
                    [&]() { return deckA().value(QStringLiteral("reverse")).toBool(); }, 800)) {
                accepted = true;
                break;
            }
            // Refused: the track is not in the cache yet. Wait a moment and ask again.
            waitFor([]() { return false; }, 250);
        }
        if (!accepted) {
            std::fprintf(stderr,
                         "dj-ui: reverse was never accepted (the track never reached the cache)\n");
            return 1;
        }
        if (!waitFor([&]() { return playHead() < beforeReverse; }, 4000)) {
            std::fprintf(stderr,
                         "dj-ui: reverse was on but the play head did not walk down (%lld from "
                         "%lld)\n",
                         playHead(), beforeReverse);
            return 1;
        }
        const qint64 downhill = playHead();
        if (!press(QStringLiteral("reverse_A"))) {
            return 1;
        }
        if (!waitFor(
                [&]() { return !deckA().value(QStringLiteral("reverse")).toBool(); }, 3000)) {
            std::fprintf(stderr, "dj-ui: reverse stayed on after the second press\n");
            return 1;
        }
        if (!waitFor([&]() { return playHead() > downhill; }, 4000)) {
            std::fprintf(stderr,
                         "dj-ui: the deck did not play forward again after reverse was switched off "
                         "(%lld from %lld)\n",
                         playHead(), downhill);
            return 1;
        }
        // Hand the deck back quiet. The next check loads its own file, and a deck that is still
        // playing refuses a load on purpose — leaving it running would make this check's leftovers
        // look like a broken load in someone else's check.
        if (deckA().value(QStringLiteral("playing")).toBool()
            && !press(QStringLiteral("play_A"))) {
            return 1;
        }
        if (!waitFor([&]() { return !deckA().value(QStringLiteral("playing")).toBool(); }, 3000)) {
            std::fprintf(stderr, "dj-ui: the deck would not stop for the next check\n");
            return 1;
        }
        std::fprintf(stdout,
                     "dj-ui: a jump of one beat moved the play head %lld frames and landed on the "
                     "grid, slip drew its virtual head on both "
                     "strips (%.4f against the sound at %.4f), and reverse walked the head down to "
                     "%lld and forward again, and the deck was left paused for the next check\n",
                     jump, slipMark, heardMark, downhill);
        return 0;
    }

    if (parser.isSet(gridHereOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        const QString fixture = parser.value(loadOption);
        if (fixture.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --grid-here-check needs --load\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        const auto waitFor = [](const std::function<bool()> &ready, int budget) {
            QElapsedTimer timer;
            timer.start();
            while (timer.elapsed() < budget) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                if (ready()) {
                    return true;
                }
                QThread::msleep(10);
            }
            return ready();
        };
        if (!waitFor([&]() { return bridge->isConnected(); }, 3000)) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        const auto deckA = [&]() {
            const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant &entry : decks) {
                const QVariantMap deck = entry.toMap();
                if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    return deck;
                }
            }
            return QVariantMap();
        };
        const auto firstBeat = [&]() -> qint64 {
            const QVariantList segments = deckA()
                                              .value(QStringLiteral("track"))
                                              .toMap()
                                              .value(QStringLiteral("grid"))
                                              .toMap()
                                              .value(QStringLiteral("segments"))
                                              .toList();
            if (segments.isEmpty()) {
                return -1;
            }
            return segments.first().toMap().value(QStringLiteral("frame")).toLongLong();
        };
        const auto playHead = [&]() -> qint64 {
            return deckA().value(QStringLiteral("source_frame")).toLongLong();
        };
        const auto click = [&](const QString &name) {
            auto *item = findItem(window->contentItem(), name);
            if (item == nullptr) {
                std::fprintf(stderr, "dj-ui: no %s to press\n", qPrintable(name));
                return false;
            }
            QMetaObject::invokeMethod(item, "clicked");
            QCoreApplication::processEvents();
            return true;
        };

        bridge->load(QStringLiteral("A"), fixture);
        const QString wantName = QFileInfo(fixture).fileName();
        if (!waitFor(
                [&]() {
                    const QString held = deckA().value(QStringLiteral("path")).toString();
                    return !held.isEmpty() && held.endsWith(wantName) && firstBeat() >= 0;
                },
                20000)) {
            std::fprintf(stderr, "dj-ui: the deck published no grid to put on the head\n");
            return 1;
        }
        const qint64 rate = deckA().value(QStringLiteral("source_rate")).toLongLong();
        const double bpm = deckA().value(QStringLiteral("local_bpm")).toDouble();
        if (rate <= 0 || !(bpm > 0.0)) {
            std::fprintf(stderr, "dj-ui: the deck published no rate or tempo\n");
            return 1;
        }
        const double period = rate * 60.0 / bpm;
        const qint64 before = firstBeat();
        // Half a beat and a few frames past it: as far from a beat of the grid as the middle of the beat
        // is, and not on a round number of frames either.
        const qint64 target = before + static_cast<qint64>(period / 2.0) + 137;
        // The track has to reach the frame the head is sent to: what is left of it is the deck's own word.
        const qint64 left = deckA().value(QStringLiteral("remaining_frames")).toLongLong();
        if (left > 0 && target > playHead() + left - 1) {
            std::fprintf(stderr, "dj-ui: the fixture is too short for the check\n");
            return 1;
        }
        bridge->seek(QStringLiteral("A"), target);
        if (!waitFor([&]() { return std::llabs(playHead() - target) <= 2; }, 5000)) {
            std::fprintf(stderr, "dj-ui: the play head did not reach frame %lld (%lld)\n", target,
                         playHead());
            return 1;
        }
        // The head has to be off the grid to begin with, or the check would prove nothing.
        const auto offTheGrid = [&]() {
            const double place = static_cast<double>(playHead() - firstBeat()) / period;
            return std::fabs(place - std::round(place)) * period > 2.0;
        };
        if (!offTheGrid()) {
            std::fprintf(stderr, "dj-ui: the play head started on a beat; nothing to prove\n");
            return 1;
        }
        // The mark on the button has to be a mark: the glyph for a target is drawn as a dot in most fonts,
        // and a dot is a button nobody presses. The ring is the mark, and it is measured, not assumed.
        auto *mark = findItem(window->contentItem(), QStringLiteral("gridHereRing_A"));
        if (mark == nullptr) {
            std::fprintf(stderr, "dj-ui: the button between the arrows has no mark of its own\n");
            return 1;
        }
        if (mark->width() < 8.0 || mark->height() < 8.0) {
            std::fprintf(stderr, "dj-ui: the mark on the button is %.1f by %.1f pixels\n",
                         mark->width(), mark->height());
            return 1;
        }
        if (!click(QStringLiteral("gridHere_A"))) {
            return 1;
        }
        if (!waitFor([&]() { return firstBeat() != before && !offTheGrid(); }, 5000)) {
            std::fprintf(stderr,
                         "dj-ui: the grid did not come to the play head (first beat %lld -> %lld, head "
                         "%lld, period %.1f)\n",
                         before, firstBeat(), playHead(), period);
            return 1;
        }
        // Where a beat sits against the head, in frames: the button promises the beat *here*.
        const double landed = static_cast<double>(playHead() - firstBeat()) / period;
        const double error = std::fabs(landed - std::round(landed)) * period;
        if (error > 2.0) {
            std::fprintf(stderr, "dj-ui: a beat landed %.1f frames from the play head\n", error);
            return 1;
        }
        const qint64 placed = firstBeat();
        // The edit is the file's, not the session's: a reload has to bring the same grid back.
        bridge->load(QStringLiteral("A"), fixture);
        if (!waitFor([&]() { return std::llabs(firstBeat() - placed) <= 2; }, 20000)) {
            std::fprintf(stderr, "dj-ui: the reload moved the grid (%lld -> %lld)\n", placed,
                         firstBeat());
            return 1;
        }
        std::fprintf(stdout,
                     "dj-ui: the grid came to the play head: a beat landed %.1f frames from frame %lld, "
                     "the mark on the button is %.0f by %.0f pixels, and the file kept the grid across a "
                     "reload\n",
                     error, target, mark->width(), mark->height());
        return 0;
    }

    if (parser.isSet(tempoRecheckOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        const QString fixture = parser.value(loadOption);
        if (fixture.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --tempo-recheck needs --load\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        const auto waitFor = [](const std::function<bool()> &ready, int budget) {
            QElapsedTimer timer;
            timer.start();
            while (timer.elapsed() < budget) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                if (ready()) {
                    return true;
                }
                QThread::msleep(10);
            }
            return ready();
        };
        if (!waitFor([&]() { return bridge->isConnected(); }, 3000)) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        const auto deckA = [&]() {
            const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant &entry : decks) {
                const QVariantMap deck = entry.toMap();
                if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    return deck;
                }
            }
            return QVariantMap();
        };
        bridge->load(QStringLiteral("A"), fixture);
        if (!waitFor(
                [&]() {
                    const QVariant tempo = deckA()
                                               .value(QStringLiteral("track"))
                                               .toMap()
                                               .value(QStringLiteral("bpm"));
                    return tempo.isValid() && !tempo.isNull();
                },
                20000)) {
            std::fprintf(stderr, "dj-ui: the engine measured no tempo to re-measure\n");
            return 1;
        }
        const QString sidecar =
            QFileInfo(fixture).absolutePath() + QStringLiteral("/.djmeta/") +
            QFileInfo(fixture).fileName() + QStringLiteral(".json");
        const auto revision = [&]() {
            QFile file(sidecar);
            if (!file.open(QIODevice::ReadOnly)) {
                return -1;
            }
            const QJsonObject meta = QJsonDocument::fromJson(file.readAll()).object();
            return meta.value(QStringLiteral("revision")).toInt(-1);
        };
        // The load itself writes the measured grid, on the engine's writer thread: wait for that write
        // before capturing the revision, or the check would credit the button with the loader's work.
        if (!waitFor([&]() { return revision() > 0; }, 8000)) {
            std::fprintf(stderr, "dj-ui: the load wrote no grid to %s\n", qPrintable(sidecar));
            return 1;
        }
        const int before = revision();
        auto *button = findItem(window->contentItem(), QStringLiteral("tempo_A"));
        if (button == nullptr) {
            std::fprintf(stderr, "dj-ui: the deck has no re-measure button\n");
            return 1;
        }
        auto *confirm = findItem(window->contentItem(), QStringLiteral("tempoGo_A"));
        // Nothing is asked of a person who has clicked nothing yet. The popup's content may not exist
        // until it is first opened, which is also a way of asking nothing.
        if (confirm != nullptr && confirm->isVisible()) {
            std::fprintf(stderr, "dj-ui: the re-measure asks nothing before it runs\n");
            return 1;
        }
        QMetaObject::invokeMethod(button, "clicked");
        QCoreApplication::processEvents();
        if (confirm == nullptr) {
            confirm = findItem(window->contentItem(), QStringLiteral("tempoGo_A"));
        }
        if (confirm == nullptr) {
            std::fprintf(stderr, "dj-ui: the re-measure opened no question at all\n");
            return 1;
        }
        if (!waitFor([&]() { return confirm->isVisible(); }, 2000)) {
            std::fprintf(stderr, "dj-ui: the re-measure button did not ask for confirmation\n");
            return 1;
        }
        // The question alone must change nothing: what is written follows the answer, not the click.
        waitFor([]() { return false; }, 300);
        if (revision() != before) {
            std::fprintf(stderr, "dj-ui: the grid was rewritten before the question was answered\n");
            return 1;
        }
        auto *strip = findItem(window->contentItem(), QStringLiteral("bpm_A"));
        // The question must fit in its window: a sentence that runs off the edge is a sentence nobody
        // finishes reading. Wrapping is what makes the window narrow instead of as wide as the text.
        auto *question = findItem(window->contentItem(), QStringLiteral("tempoText_A"));
        if (question == nullptr) {
            std::fprintf(stderr, "dj-ui: the question has no text to read\n");
            return 1;
        }
        const double questionWidth = question->width();
        const double unwrapped = question->property("implicitWidth").toDouble();
        const double questionHeight = question->height();
        if (questionWidth > 300.0 || unwrapped <= questionWidth || questionHeight < 24.0) {
            std::fprintf(
                stderr,
                "dj-ui: the question does not wrap (%.0f px wide, %.0f px unwrapped, %.0f px tall)\n",
                questionWidth, unwrapped, questionHeight);
            return 1;
        }
        auto *badge = findItem(window->contentItem(), QStringLiteral("analyzing_A"));
        const QString shownBefore = strip != nullptr ? strip->property("text").toString() : QString();
        QMetaObject::invokeMethod(confirm, "clicked");
        // The screen has to say that the work is happening: the engine publishes the flag, the panel
        // draws a small window, and the check reads the panel rather than the snapshot behind it.
        const bool announced = waitFor(
            [&]() {
                return badge != nullptr && badge->isVisible() &&
                       deckA().value(QStringLiteral("analyzing")).toBool();
            },
            5000);
        if (!announced) {
            std::fprintf(stderr, "dj-ui: the re-measure ran without saying so\n");
            return 1;
        }
        if (!waitFor([&]() { return revision() > before; }, 20000)) {
            std::fprintf(stderr, "dj-ui: the confirmed re-measure never reached the metadata\n");
            return 1;
        }
        // ... and the number on the strip is the new one: the file and the screen must not disagree.
        // The publisher assembles the description at display rate, so the new number arrives a frame or
        // two after the file does — waiting for it is the difference between a slow screen and a stale one.
        waitFor(
            [&]() {
                return strip != nullptr && strip->property("text").toString() != shownBefore;
            },
            5000);
        const QVariant tempo =
            deckA().value(QStringLiteral("track")).toMap().value(QStringLiteral("bpm"));
        const QString shownAfter = strip != nullptr ? strip->property("text").toString() : QString();
        if (shownAfter.isEmpty() || shownAfter == shownBefore) {
            std::fprintf(stderr,
                         "dj-ui: the strip still shows \"%s\" after the measurement of %.2f BPM\n",
                         qPrintable(shownAfter), tempo.toDouble());
            return 1;
        }
        std::fprintf(stdout,
                     "dj-ui: the re-measure asked, was announced \"%s\", confirmed, and reached the "
                     "strip: \"%s\" -> \"%s\", revision %d -> %d\n",
                     deckA().value(QStringLiteral("analyzing")).toBool() ? "yes" : "seen",
                     qPrintable(shownBefore), qPrintable(shownAfter), before, revision());
        return 0;
    }

    if (parser.isSet(gridReconnectOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        const auto waitFor = [](const std::function<bool()> &ready, int budget) {
            QElapsedTimer timer;
            timer.start();
            while (timer.elapsed() < budget) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                if (ready()) {
                    return true;
                }
                QThread::msleep(10);
            }
            return ready();
        };
        bridge->connectToEngine(parser.value(connectOption));
        if (!waitFor([&]() { return bridge->isConnected(); }, 3000)) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        // No load here: this client must find the deck as the previous one left it. That is the whole
        // point — a load is what used to tell the engine its tempo base, and a reconnected client never
        // loads anything.
        const auto deckA = [&]() {
            const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant &entry : decks) {
                const QVariantMap deck = entry.toMap();
                if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    return deck;
                }
            }
            return QVariantMap();
        };
        const auto segments = [&]() {
            return deckA()
                .value(QStringLiteral("track"))
                .toMap()
                .value(QStringLiteral("grid"))
                .toMap()
                .value(QStringLiteral("segments"))
                .toList();
        };
        if (!waitFor([&]() { return !segments().isEmpty(); }, 10000)) {
            std::fprintf(stderr, "dj-ui: the deck holds no grid; load one before this check\n");
            return 1;
        }
        const QVariantMap first = segments().first().toMap();
        const double rate = deckA().value(QStringLiteral("source_rate")).toDouble();
        const double bpm = first.value(QStringLiteral("bpm")).toDouble();
        if (rate <= 0.0 || bpm <= 0.0) {
            std::fprintf(stderr, "dj-ui: the deck published no rate or no tempo\n");
            return 1;
        }
        const qint64 start = std::llround(first.value(QStringLiteral("frame")).toDouble());
        const qint64 beat = std::llround(rate * 60.0 / bpm);
        auto *strip = findItem(window->contentItem(), QStringLiteral("waveStrip_A"));
        auto *right = findItem(window->contentItem(), QStringLiteral("gridBeatRight_A"));
        auto *left = findItem(window->contentItem(), QStringLiteral("gridBeatLeft_A"));
        if (strip == nullptr || right == nullptr || left == nullptr) {
            std::fprintf(stderr, "dj-ui: the deck has no grid buttons to press\n");
            return 1;
        }
        const auto firstFrame = [&]() {
            const QVariantList list = segments();
            return list.isEmpty() ? -1.0 : list.first().toMap().value(QStringLiteral("frame")).toDouble();
        };
        QMetaObject::invokeMethod(right, "clicked");
        if (!waitFor([&]() { return std::llround(firstFrame()) == start + beat; }, 5000)) {
            std::fprintf(stderr,
                         "dj-ui: on a reconnected client the grid stayed at %.0f instead of moving to "
                         "%lld\n",
                         firstFrame(), static_cast<long long>(start + beat));
            return 1;
        }
        QMetaObject::invokeMethod(left, "clicked");
        if (!waitFor([&]() { return std::llround(firstFrame()) == start; }, 5000)) {
            std::fprintf(stderr, "dj-ui: the grid did not come back on a reconnected client (%.0f)\n",
                         firstFrame());
            return 1;
        }
        std::fprintf(stdout,
                     "dj-ui: the grid moves on a reconnected client: %lld -> %lld and back at %.2f "
                     "BPM\n",
                     static_cast<long long>(start), static_cast<long long>(start + beat), bpm);
        return 0;
    }

    if (parser.isSet(tapOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        const QString fixture = parser.value(loadOption);
        if (fixture.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --tap-check needs --load\n");
            return 1;
        }
        const auto waitFor = [](const std::function<bool()> &ready, int budget) {
            QElapsedTimer timer;
            timer.start();
            while (timer.elapsed() < budget) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                if (ready()) {
                    return true;
                }
                QThread::msleep(10);
            }
            return ready();
        };
        bridge->connectToEngine(parser.value(connectOption));
        if (!waitFor([&]() { return bridge->isConnected(); }, 3000)) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        const auto deckA = [&]() {
            const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant &entry : decks) {
                const QVariantMap deck = entry.toMap();
                if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    return deck;
                }
            }
            return QVariantMap();
        };
        const auto grid = [&]() {
            return deckA()
                .value(QStringLiteral("track"))
                .toMap()
                .value(QStringLiteral("grid"))
                .toMap();
        };
        bridge->load(QStringLiteral("A"), fixture);
        const QString wantName = QFileInfo(fixture).fileName();
        const auto loadArrived = [&]() {
            const QString held = deckA().value(QStringLiteral("path")).toString();
            return !held.isEmpty() && held.endsWith(wantName);
        };
        if (!waitFor(
                [&]() {
                    return loadArrived() && !grid().value(QStringLiteral("segments")).toList().isEmpty();
                },
                20000)) {
            std::fprintf(stderr, "dj-ui: the deck published no grid to tap onto\n");
            return 1;
        }
        auto *panel = findItem(window->contentItem(), QStringLiteral("deckPanel_A"));
        if (panel == nullptr) {
            std::fprintf(stderr, "dj-ui: there is no deck panel to tap on\n");
            return 1;
        }
        // Four taps on a 120 BPM line that starts 0.32 s into the file, with the deck's position
        // jittering by a snapshot the way a real hand does: the grid has to come out on the line
        // through the taps, not on the play head and not on their median gap alone.
        constexpr double kRate = 48000.0;
        constexpr double kPeriod = 24000.0;
        // A quarter of a beat after the first frame: a grid put on the play head instead of on the taps
        // is a quarter of a beat away, which the check has to notice.
        constexpr double kFirst = 12000.0;
        // The play head is moved away from the taps' own phase first: a grid put on the play head
        // instead of on the taps then lands a third of a beat off, which the check has to notice.
        bool moved = false;
        for (int attempt = 0; attempt < 3 && !moved; ++attempt) {
            bridge->seek(QStringLiteral("A"), 100'000);
            moved = waitFor(
                [&]() { return deckA().value(QStringLiteral("source_frame")).toDouble() >= 99'999.0; },
                3000);
        }
        if (!moved) {
            std::fprintf(stderr, "dj-ui: the deck never moved to where the taps are not\n");
            return 1;
        }
        // Exact taps: a hand that jitters in milliseconds jitters in frames by the same amount, and a series
        // that jitters in one and not the other is not a hand. What the leg proves is the line through the
        // taps — the grid has to pass through them, and a grid put on the play head is a quarter of a beat
        // away from the first one.
        QVariantList taps;
        for (int index = 0; index < 4; ++index) {
            QVariantMap tap;
            tap.insert(QStringLiteral("ms"), 10'000 + 500 * index);
            tap.insert(QStringLiteral("frame"), kFirst + kPeriod * index);
            taps.append(tap);
        }
        QVariant accepted;
        QMetaObject::invokeMethod(panel, "applyTaps", Q_RETURN_ARG(QVariant, accepted),
                                  Q_ARG(QVariant, taps));
        if (!accepted.toBool()) {
            std::fprintf(stderr, "dj-ui: the taps made no grid\n");
            return 1;
        }
        const auto firstBeat = [&]() {
            const QVariantList segments = grid().value(QStringLiteral("segments")).toList();
            if (segments.isEmpty()) {
                return -1.0;
            }
            return segments.first().toMap().value(QStringLiteral("frame")).toDouble();
        };
        const auto tempo = [&]() {
            const QVariantList segments = grid().value(QStringLiteral("segments")).toList();
            return segments.isEmpty()
                       ? 0.0
                       : segments.first().toMap().value(QStringLiteral("bpm")).toDouble();
        };
        const double tempoBefore = tempo();
        const double firstBefore = std::max(0.0, firstBeat());
        // The old grid is a grid too: the wait is for the tapped one, which has a different tempo or a
        // different first beat, and for the taps to sit on it.
        const auto tapsOnBeats = [&]() {
            const double period = kRate * 60.0 / tempo();
            if (!(period > 0.0) || firstBeat() < 0.0) {
                return false;
            }
            for (int index = 0; index < taps.size(); ++index) {
                const double frame = taps.at(index).toMap().value(QStringLiteral("frame")).toDouble();
                const double beat = (frame - firstBeat()) / period;
                if (std::abs(beat - std::round(beat)) > 0.08) {
                    return false;
                }
            }
            return true;
        };
        if (!waitFor(
                [&]() {
                    const bool changed = std::abs(tempo() - tempoBefore) > 0.05 ||
                                         std::abs(firstBeat() - firstBefore) > 1.0;
                    return changed && std::abs(tempo() - 120.0) < 0.2 && tapsOnBeats();
                },
                8000)) {
            std::fprintf(stderr, "dj-ui: the tapped grid reads %.2f BPM with its first beat at %.0f\n",
                         tempo(), firstBeat());
            return 1;
        }
        // Every tap has to land on a beat: the grid passes through the hand, not near it.
        const double period = kRate * 60.0 / tempo();
        for (int index = 0; index < taps.size(); ++index) {
            const double frame = taps.at(index).toMap().value(QStringLiteral("frame")).toDouble();
            const double beat = (frame - firstBeat()) / period;
            const double off = std::abs(beat - std::round(beat));
            if (off > 0.08) {
                std::fprintf(stderr,
                             "dj-ui: tap %d at frame %.0f is %.2f of a beat off the grid (first beat "
                             "%.0f, %.2f BPM)\n",
                             index, frame, off, firstBeat(), tempo());
                return 1;
            }
        }
        std::fprintf(stdout,
                     "dj-ui: the taps made the grid: %.2f BPM, first beat %.0f on the line through "
                     "four taps\n",
                     tempo(), firstBeat());

        // The real path, which the arrays above do not touch: a playing deck and taps of the hand on the
        // button. The reference is the deck's own head, measured from what it publishes — the beat the hand
        // beat out is `speed × interval` frames, a number that owes nothing to the wall clock or the pitch.
        // The phase is the first beat against the head at the first tap: a grid put on a stale snapshot is
        // tens of milliseconds behind, and the check names it in milliseconds.
        auto *tapButton = findItem(window->contentItem(), QStringLiteral("gridTap_A"));
        if (tapButton == nullptr) {
            std::fprintf(stderr, "dj-ui: there is no Tap button to press\n");
            return 1;
        }
        const auto headLine = [&](QElapsedTimer &clock, double &slope, double &intercept) {
            QVector<QPointF> path;
            for (int index = 0; index < 80; ++index) {
                QCoreApplication::processEvents();
                path.append(QPointF(clock.nsecsElapsed() / 1e6,
                                    deckA().value(QStringLiteral("source_frame")).toDouble()));
                QThread::msleep(8);
            }
            double sumX = 0;
            double sumY = 0;
            for (const QPointF &sample : path) {
                sumX += sample.x();
                sumY += sample.y();
            }
            const double meanX = sumX / path.size();
            const double meanY = sumY / path.size();
            double num = 0;
            double den = 0;
            for (const QPointF &sample : path) {
                num += (sample.x() - meanX) * (sample.y() - meanY);
                den += (sample.x() - meanX) * (sample.x() - meanX);
            }
            slope = den > 0.0 ? num / den : 0.0;
            intercept = meanY - slope * meanX;
            return slope > 0.0;
        };
        const auto tapLeg = [&](double ratio, const char *name) {
            bridge->setTempo(QStringLiteral("A"), ratio);
            waitFor([&]() {
                return std::abs(deckA().value(QStringLiteral("tempo_ratio")).toDouble() - ratio) < 0.002;
            },
                     5000);
            // From the start: the fixture is eight seconds long, and a deck that reached its end stands
            // still and refuses to play again until it is moved. The taps of the leg before are old enough
            // by then to start a new series (the button forgets a series after two seconds).
            bridge->seek(QStringLiteral("A"), 0);
            waitFor([&]() { return deckA().value(QStringLiteral("source_frame")).toDouble() < 1000.0; }, 5000);
            bridge->play(QStringLiteral("A"), true);
            if (!waitFor([&]() { return deckA().value(QStringLiteral("playing")).toBool(); }, 10000)) {
                std::fprintf(stderr,
                             "dj-ui: the deck never played for the %s taps (load %s, playing %s, ratio "
                             "%.3f, frame %.0f)\n",
                             name, qPrintable(deckA().value(QStringLiteral("load")).toString()),
                             deckA().value(QStringLiteral("playing")).toBool() ? "yes" : "no",
                             deckA().value(QStringLiteral("tempo_ratio")).toDouble(),
                             deckA().value(QStringLiteral("source_frame")).toDouble());
                return false;
            }
            // A moment for the deck to settle: the first tenths of a second after a start are not the speed
            // the taps are beat against.
            QThread::msleep(300);
            QCoreApplication::processEvents();
            // The reference is the deck's own head across the whole span of the taps: measured before them
            // and after them, so a transport that runs a little slower than real time is measured, not
            // assumed.
            QElapsedTimer clock;
            clock.start();
            double slopeBefore = 0.0;
            double interceptBefore = 0.0;
            if (!headLine(clock, slopeBefore, interceptBefore)) {
                std::fprintf(stderr, "dj-ui: the head did not move for the %s taps\n", name);
                return false;
            }
            const double base = clock.nsecsElapsed() / 1e6;
            // The head at the first tap, from the line through the published samples: a single sample is
            // quantised to the publish step, and that step is exactly what this check is about.
            double headAtBase = slopeBefore * base + interceptBefore;
            double tapsMs[5] = {0, 0, 0, 0, 0};
            // Where the button stands before the taps: the row of grid tools is centred, and anything that
            // appears beside the button while a series is running moves it out from under the hand — the
            // second tap lands somewhere else, and the DJ sees the click miss (owner's report, 15.11.2026).
            const QPointF atBefore = tapButton->mapToScene(
                QPointF(tapButton->width() / 2.0, tapButton->height() / 2.0));
            const QPointF at = tapButton->mapToScene(
                QPointF(tapButton->width() / 2.0, tapButton->height() / 2.0));
            for (int index = 0; index < 5; ++index) {
                const double want = base + 500.0 * index;
                while (clock.nsecsElapsed() / 1e6 < want) {
                    QCoreApplication::processEvents();
                    QThread::msleep(1);
                }
                const int countedBefore = panel->property("taps").toList().size();
                for (const QEvent::Type type : {QEvent::MouseButtonPress, QEvent::MouseButtonRelease}) {
                    QMouseEvent event(type, at, window->mapToGlobal(at.toPoint()), Qt::LeftButton,
                                      Qt::LeftButton, Qt::NoModifier);
                    QCoreApplication::sendEvent(window, &event);
                    QCoreApplication::processEvents();
                }
                // The press may never arrive in an offscreen window (the same flakiness the columns had):
                // a tap that was not counted falls back to the button's own signal, and says so.
                if (panel->property("taps").toList().size() == countedBefore) {
                    if (index == 0) {
                        std::fprintf(stderr, "dj-ui: the press on Tap did not land, pressing the signal\n");
                    }
                    QMetaObject::invokeMethod(tapButton, "clicked");
                    QCoreApplication::processEvents();
                }
                tapsMs[index] = clock.nsecsElapsed() / 1e6;
            }
            double slopeAfter = 0.0;
            double interceptAfter = 0.0;
            headLine(clock, slopeAfter, interceptAfter);
            const double endMs = clock.nsecsElapsed() / 1e6;
            const double headAtEnd = slopeAfter * endMs + interceptAfter;
            const QPointF atAfter = tapButton->mapToScene(
                QPointF(tapButton->width() / 2.0, tapButton->height() / 2.0));
            if (std::abs(atAfter.x() - atBefore.x()) > 0.5
                || std::abs(atAfter.y() - atBefore.y()) > 0.5) {
                std::fprintf(stderr,
                             "dj-ui: the Tap button moved under the hand: (%.1f, %.1f) -> (%.1f, %.1f)\n",
                             atBefore.x(), atBefore.y(), atAfter.x(), atAfter.y());
                return false;
            }
            const double speed = endMs > base ? (headAtEnd - headAtBase) / (endMs - base) : 0.0;
            if (!(speed > 0.0)) {
                std::fprintf(stderr, "dj-ui: the head gave no speed for the %s taps\n", name);
                return false;
            }
            const double periodWall = (tapsMs[4] - tapsMs[0]) / 4.0;
            const double wantBpm = 60.0 * kRate / (speed * periodWall);
            double wantFrames[5] = {0, 0, 0, 0, 0};
            for (int index = 0; index < 5; ++index) {
                wantFrames[index] = headAtBase + speed * (tapsMs[index] - base);
            }
            const auto worstOffBeat = [&]() {
                const double period = kRate * 60.0 / tempo();
                if (!(period > 0.0) || firstBeat() < 0.0) {
                    return 1.0e9;
                }
                double worst = 0.0;
                for (int index = 0; index < 5; ++index) {
                    const double beat = (wantFrames[index] - firstBeat()) / period;
                    const double off = std::abs(beat - std::round(beat)) * period;
                    worst = std::max(worst, off);
                }
                return worst;
            };
            const bool landed = waitFor(
                [&]() {
                    // The head is published twenty times a second, so a tap's frame is only known to half a
                    // step: the thresholds are the ones the measurement can resolve, not the ones the code
                    // would like. The old path — a grid put on the snapshot, no extrapolation — stood 68 ms
                    // behind the hand and fails both.
                    return std::abs(tempo() - wantBpm) <= wantBpm * 0.006
                           && worstOffBeat() * 1000.0 / kRate <= 40.0;
                },
                8000);
            const double phaseMs = worstOffBeat() * 1000.0 / kRate;
            bridge->play(QStringLiteral("A"), false);
            waitFor([&]() { return deckA().value(QStringLiteral("playing")).toBool() == false; }, 5000);
            if (!landed) {
                std::fprintf(stderr,
                             "dj-ui: the %s taps made %.2f BPM where the hand beat %.2f; worst tap %.1f ms "
                             "off a beat (first beat %.0f, speed %.4f frames/ms, period wall %.1f ms)\n",
                             name, tempo(), wantBpm, phaseMs, firstBeat(), speed, periodWall);
                return false;
            }
            std::fprintf(stdout,
                         "dj-ui: the %s taps of the hand: %.2f BPM (the hand beat %.2f), every tap within "
                         "%.1f ms of a beat\n",
                         name, tempo(), wantBpm, phaseMs);
            return true;
        };
        const bool plain = tapLeg(1.0, "plain");
        const bool pitched = tapLeg(1.08, "pitched");
        bridge->setTempo(QStringLiteral("A"), 1.0);
        if (!plain || !pitched) {
            return 1;
        }
        return 0;
    }

    if (parser.isSet(metronomeOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        const QString fixture = parser.value(loadOption);
        if (fixture.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --metronome-check needs --load\n");
            return 1;
        }
        const auto waitFor = [](const std::function<bool()> &ready, int budget) {
            QElapsedTimer timer;
            timer.start();
            while (timer.elapsed() < budget) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                if (ready()) {
                    return true;
                }
                QThread::msleep(10);
            }
            return ready();
        };
        bridge->connectToEngine(parser.value(connectOption));
        if (!waitFor([&]() { return bridge->isConnected(); }, 3000)) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        const auto deckA = [&]() {
            const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant &entry : decks) {
                const QVariantMap deck = entry.toMap();
                if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    return deck;
                }
            }
            return QVariantMap();
        };
        const auto flag = [&]() {
            return deckA().value(QStringLiteral("metronome")).toBool();
        };
        const auto clicks = [&]() {
            return deckA().value(QStringLiteral("clicks")).toULongLong();
        };
        const auto lit = [&]() {
            auto *button = findItem(window->contentItem(), QStringLiteral("metronome_A"));
            if (button == nullptr) {
                return false;
            }
            // A two-state control in the deck paints its state through `lit`; `highlighted` is the
            // Controls property, and with the deck's own paint nothing reads it any more.
            const QVariant state = button->property("lit");
            return state.isValid() ? state.toBool() : button->property("highlighted").toBool();
        };
        bridge->load(QStringLiteral("A"), fixture);
        const QString wantName = QFileInfo(fixture).fileName();
        if (!waitFor(
                [&]() {
                    const QString held = deckA().value(QStringLiteral("path")).toString();
                    return !held.isEmpty() && held.endsWith(wantName)
                           && !deckA()
                                   .value(QStringLiteral("track"))
                                   .toMap()
                                   .value(QStringLiteral("grid"))
                                   .toMap()
                                   .value(QStringLiteral("segments"))
                                   .toList()
                                   .isEmpty();
                },
                20000)) {
            std::fprintf(stderr, "dj-ui: the deck published no grid to click against (path '%s')\n",
                         deckA().value(QStringLiteral("path")).toString().toUtf8().constData());
            return 1;
        }
        auto *button = findItem(window->contentItem(), QStringLiteral("metronome_A"));
        if (button == nullptr) {
            std::fprintf(stderr, "dj-ui: the deck has no metronome button\n");
            return 1;
        }
        if (flag() || lit()) {
            std::fprintf(stderr, "dj-ui: the metronome is already on before anyone pressed it\n");
            return 1;
        }
        QMetaObject::invokeMethod(button, "clicked");
        if (!waitFor([&]() { return flag() && lit(); }, 5000)) {
            std::fprintf(stderr,
                         "dj-ui: the metronome button did not reach the engine (state %d, lit %d)\n",
                         int(flag()), int(lit()));
            return 1;
        }
        // A flag is not a click: the deck has to play, and the engine has to count the clicks it played on
        // the beats it crossed. The count is the one number a check can measure instead of hearing.
        // The engine attaches the file at a block boundary, and the measurement that follows re-attaches
        // it: a deck told to play before that lands is put back at the start and stopped, which is exactly
        // what a metronome that never clicked looked like. So the load has to settle first, and the play
        // has to be seen holding before the clicks are counted.
        const auto settled = [&]() {
            const QVariantMap deck = deckA();
            const QString load = deck.value(QStringLiteral("load")).toString();
            return load.isEmpty() || load == QStringLiteral("Ready");
        };
        if (!waitFor(settled, 20000)) {
            std::fprintf(stderr, "dj-ui: the deck never finished loading for the metronome\n");
            return 1;
        }
        waitFor([&]() { return false; }, 300);
        // The engine attaches a file at a block boundary and a measurement that finishes re-attaches it,
        // which puts the deck back at the start, stopped. A hand would press play again; so does this.
        const auto playing = [&]() {
            return deckA().value(QStringLiteral("playing")).toBool();
        };
        bridge->play(QStringLiteral("A"), true);
        if (!waitFor(playing, 5000)) {
            std::fprintf(stderr, "dj-ui: the deck did not start playing for the metronome\n");
            return 1;
        }
        const auto clicksArrive = [&]() {
            for (int tick = 0; tick < 80; ++tick) {
                if (clicks() >= 2) {
                    return true;
                }
                if (!playing()) {
                    // A reload landed: start again from where the engine put the deck.
                    bridge->play(QStringLiteral("A"), true);
                }
                waitFor([&]() { return false; }, 100);
            }
            return clicks() >= 2;
        };
        if (!clicksArrive()) {
            // Everything a silent metronome could be: the deck's own state, the grid it clicks against,
            // and whether the head is moving at all. A count of zero on its own says nothing.
            const QVariantMap deck = deckA();
            const QVariantMap grid = deck.value(QStringLiteral("track"))
                                         .toMap()
                                         .value(QStringLiteral("grid"))
                                         .toMap();
            const QVariantList segments = grid.value(QStringLiteral("segments")).toList();
            const auto segment = segments.isEmpty() ? QVariantMap() : segments.first().toMap();
            std::fprintf(
                stderr,
                "dj-ui: the deck played but the metronome clicked %llu times "
                "(frame %lld playing %d metronome %d bpm %.3f first %lld metre %d segments %lld "
                "segment0 frame %lld beat %lld bpm %.3f)\n",
                static_cast<unsigned long long>(clicks()),
                static_cast<long long>(deck.value(QStringLiteral("source_frame")).toLongLong()),
                int(deck.value(QStringLiteral("playing")).toBool()),
                int(flag()),
                grid.value(QStringLiteral("bpm")).toDouble(),
                static_cast<long long>(grid.value(QStringLiteral("first_beat_frame")).toLongLong()),
                grid.value(QStringLiteral("beats_per_bar")).toInt(),
                static_cast<long long>(segments.size()),
                static_cast<long long>(segment.value(QStringLiteral("frame")).toLongLong()),
                static_cast<long long>(segment.value(QStringLiteral("beat")).toLongLong()),
                segment.value(QStringLiteral("bpm")).toDouble());
            return 1;
        }
        const unsigned long long played = clicks();
        bridge->play(QStringLiteral("A"), false);
        QMetaObject::invokeMethod(button, "clicked");
        if (!waitFor([&]() { return !flag() && !lit(); }, 5000)) {
            std::fprintf(stderr, "dj-ui: the metronome stayed on (state %d, lit %d)\n", int(flag()),
                         int(lit()));
            return 1;
        }
        const unsigned long long after = clicks();
        QThread::msleep(400);
        if (clicks() != after) {
            std::fprintf(stderr, "dj-ui: the metronome kept clicking after it was switched off\n");
            return 1;
        }
        std::fprintf(stdout,
                     "dj-ui: the metronome clicks the grid: the button lit, the deck played %llu clicks "
                     "on its beats, and switching it off stopped them\n",
                     played);
        return 0;
    }

    if (parser.isSet(scratchOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        const QString fixture = parser.value(loadOption);
        if (fixture.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --scratch-check needs --load\n");
            return 1;
        }
        const auto waitFor = [](const std::function<bool()> &ready, int budget) {
            QElapsedTimer timer;
            timer.start();
            while (timer.elapsed() < budget) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                if (ready()) {
                    return true;
                }
                QThread::msleep(10);
            }
            return ready();
        };
        bridge->connectToEngine(parser.value(connectOption));
        if (!waitFor([&]() { return bridge->isConnected(); }, 3000)) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        const auto deckA = [&]() {
            const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant &entry : decks) {
                const QVariantMap deck = entry.toMap();
                if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    return deck;
                }
            }
            return QVariantMap();
        };
        const auto frame = [&]() { return deckA().value(QStringLiteral("source_frame")).toLongLong(); };
        const auto playing = [&]() { return deckA().value(QStringLiteral("playing")).toBool(); };

        bridge->load(QStringLiteral("A"), fixture);
        if (!waitFor(
                [&]() {
                    const QVariantMap deck = deckA();
                    return deck.value(QStringLiteral("load")).toString() == QStringLiteral("Ready")
                           && !deck.value(QStringLiteral("track"))
                                   .toMap()
                                   .value(QStringLiteral("grid"))
                                   .toMap()
                                   .value(QStringLiteral("segments"))
                                   .toList()
                                   .isEmpty();
                },
                20000)) {
            std::fprintf(stderr, "dj-ui: the deck never finished loading for the scratch\n");
            return 1;
        }
        // The measurement re-attaches the material and puts the deck back at the start: play from the
        // frame it settled on, not from the one it was heading to.
        waitFor([&]() { return false; }, 300);

        bridge->play(QStringLiteral("A"), true);
        if (!waitFor(playing, 5000)) {
            std::fprintf(stderr, "dj-ui: the deck did not start playing for the scratch\n");
            return 1;
        }
        (void)waitFor([&]() { return frame() > 0; }, 3000);

        auto *strip = findItem(window->contentItem(), QStringLiteral("waveStrip_A"));
        if (strip == nullptr) {
            std::fprintf(stderr, "dj-ui: the deck has no waveform strip to take hold of\n");
            return 1;
        }
        const auto send = [&](QEvent::Type type, const QPointF &at) {
            // A move carries the button that is *held*, not the one that was pressed: a move with no button
            // held is not a drag, and the hand lets go of the record in the middle of it.
            // A press and a release both name the left button; only a move carries none. `buttons` is what
            // is held *after* the event, so a release has none.
            QMouseEvent event(type, at, window->mapToGlobal(at.toPoint()),
                              type == QEvent::MouseMove ? Qt::NoButton : Qt::LeftButton,
                              type == QEvent::MouseButtonRelease ? Qt::NoButton : Qt::LeftButton,
                              Qt::NoModifier);
            QCoreApplication::sendEvent(window, &event);
            QCoreApplication::processEvents();
        };
        const QPointF at = strip->mapToScene(QPointF(strip->width() * 0.5, strip->height() * 0.5));

        // A hand on the record stops the motor: the record's speed is the hand's from here on. The hand is
        // taken by the first move, not by the press — a click on the wave is only a click.
        send(QEvent::MouseButtonPress, at);
        send(QEvent::MouseMove, at - QPointF(2.0, 0.0));
        if (!waitFor([&]() { return !playing(); }, 3000)) {
            std::fprintf(stderr, "dj-ui: taking the record in hand did not stop the transport\n");
            return 1;
        }
        const qint64 grabbed = frame();

        // Twelve moves to the left: the material goes forward, as the picture says it does.
        for (int step = 1; step <= 12; ++step) {
            send(QEvent::MouseMove, at - QPointF(10.0 * step, 0.0));
            (void)waitFor([&]() { return false; }, 15);
        }
        const qint64 moved = frame();
        if (moved <= grabbed) {
            std::fprintf(stderr,
                         "dj-ui: the hand moved left but the material went nowhere (%lld -> %lld)\n",
                         static_cast<long long>(grabbed),
                         static_cast<long long>(moved));
            return 1;
        }
        // A hand, not a jump: 120 px of a zoomed strip is a fraction of a second of material, and the
        // whole file is eight.
        const qint64 travelled = moved - grabbed;
        if (travelled > 48'000 * 2) {
            std::fprintf(stderr, "dj-ui: the drag jumped %lld frames instead of moving the material\n",
                         static_cast<long long>(travelled));
            return 1;
        }

        // A hand that stops moving comes to rest: the record has the speed it was last given until the
        // hand's silence reaches the deck, and then it stands still. So the check asks that it *stops*, not
        // that it never moved after the last event.
        (void)waitFor([&]() { return false; }, 200);
        const qint64 rested = frame();
        (void)waitFor([&]() { return false; }, 300);
        if (frame() != rested) {
            std::fprintf(stderr, "dj-ui: a still hand kept moving the record (%lld -> %lld)\n",
                         static_cast<long long>(rested), static_cast<long long>(frame()));
            return 1;
        }

        // Letting go gives the record back to the motor, from where the hand left it.
        send(QEvent::MouseButtonRelease, at - QPointF(120.0, 0.0));
        if (!waitFor(playing, 3000)) {
            std::fprintf(stderr, "dj-ui: the motor did not take the record back after the hand let go\n");
            return 1;
        }
        if (frame() < moved) {
            std::fprintf(stderr, "dj-ui: the motor carried the record backwards (%lld -> %lld)\n",
                         static_cast<long long>(moved), static_cast<long long>(frame()));
            return 1;
        }
        // The motor, not the hand: after the hand lets go the deck has to carry the material at the file's
        // own speed, which is the one thing that says the hand is really off the record.
        (void)waitFor([&]() { return false; }, 100);
        const qint64 carriedAt = frame();
        const double rate = deckA().value(QStringLiteral("source_rate")).toDouble();
        (void)waitFor([&]() { return false; }, 400);
        const qint64 carriedTo = frame();
        const double expected = rate * 0.4;
        const double actual = static_cast<double>(carriedTo - carriedAt);
        if (std::abs(actual - expected) > expected * 0.35) {
            std::fprintf(stderr,
                         "dj-ui: after the hand let go the material moved at %.2f of the file's speed\n",
                         actual / expected);
            return 1;
        }
        std::fprintf(stdout,
                     "dj-ui: the waveform is the record: the hand stopped the motor, moved the material "
                     "%lld frames to the left, a still hand moved nothing, and letting go handed it back "
                     "to the motor\n",
                     static_cast<long long>(travelled));
        return 0;
    }

    if (parser.isSet(gridLinesOption)) {
        return runGridLines(engine, bridge, parser, connectOption, loadOption);
    }

    if (parser.isSet(gridCreateOption)) {
        // A track whose tempo could not be measured has no grid — and that is exactly the track whose
        // grid a person has to place. The tools have to be there, the ones that read a grid have to be
        // grey, and the two that make one have to work: a beat placed at the play head starts a grid
        // there, a typed tempo starts one at the first frame (DJP-MEDIA-006).
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        const QString fixture = parser.value(loadOption);
        if (fixture.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --grid-create-check needs --load\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        const auto waitFor = [](const std::function<bool()> &ready, int budget) {
            QElapsedTimer timer;
            timer.start();
            while (timer.elapsed() < budget) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                if (ready()) {
                    return true;
                }
                QThread::msleep(10);
            }
            return ready();
        };
        if (!waitFor([&]() { return bridge->isConnected(); }, 3000)) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        const auto deckA = [&]() {
            return deckById(bridge->snapshot(), QStringLiteral("A"));
        };
        const auto gridOf = [&]() { return deckA().value(QStringLiteral("track")).toMap().value(
                                            QStringLiteral("grid")).toMap(); };
        const auto firstBeat = [&]() -> qint64 {
            const QVariantList segments = gridOf().value(QStringLiteral("segments")).toList();
            if (segments.isEmpty()) {
                return -1;
            }
            return segments.first().toMap().value(QStringLiteral("frame")).toLongLong();
        };
        const auto tempoOf = [&]() {
            const QVariantList segments = gridOf().value(QStringLiteral("segments")).toList();
            return segments.isEmpty()
                       ? 0.0
                       : segments.first().toMap().value(QStringLiteral("bpm")).toDouble();
        };
        const auto itemByName = [&](const QString &name) {
            return findItem(window->contentItem(), name);
        };

        // Its own file, emptied first: a deck that already holds a grid would answer about the track
        // before this one.
        bridge->eject(QStringLiteral("A"));
        waitFor([&]() { return deckA().value(QStringLiteral("track")).toMap().isEmpty(); }, 5000);
        bridge->load(QStringLiteral("A"), fixture);
        const QString wantName = QFileInfo(fixture).fileName();
        const auto loadArrived = [&]() {
            const QString held = deckA().value(QStringLiteral("path")).toString();
            return !held.isEmpty() && held.endsWith(wantName);
        };
        if (!waitFor([&]() { return loadArrived(); }, 30000)) {
            std::fprintf(stderr, "dj-ui: deck A never loaded %s\n", qPrintable(wantName));
            return 1;
        }
        if (!gridOf().isEmpty()) {
            std::fprintf(stderr, "dj-ui: %s measured a grid of its own; this check needs a track "
                                 "with none\n",
                         qPrintable(wantName));
            return 1;
        }

        // The row is there, the readers are grey, the makers are live.
        auto *row = itemByName(QStringLiteral("gridedit_A"));
        if (row == nullptr || !row->isVisible()) {
            std::fprintf(stderr, "dj-ui: the grid tools are hidden on a track with no grid\n");
            return 1;
        }
        struct Expect {
            const char *name;
            bool enabled;
        };
        for (const Expect &expect : {Expect{"gridHere_A", true}, Expect{"gridTempo_A", true},
                                     Expect{"gridTap_A", true}, Expect{"gridRight_A", false},
                                     Expect{"gridHalf_A", false}, Expect{"gridDouble_A", false},
                                     Expect{"metronome_A", false}}) {
            auto *item = itemByName(QString::fromLatin1(expect.name));
            if (item == nullptr) {
                std::fprintf(stderr, "dj-ui: no %s on a track with no grid\n", expect.name);
                return 1;
            }
            if (!item->isVisible()) {
                std::fprintf(stderr, "dj-ui: %s is hidden on a track with no grid\n", expect.name);
                return 1;
            }
            const bool enabled = item->property("enabled").toBool();
            if (enabled != expect.enabled) {
                std::fprintf(stderr, "dj-ui: %s is %s, and on a track with no grid it must be %s\n",
                             expect.name, enabled ? "live" : "grey",
                             expect.enabled ? "live" : "grey");
                return 1;
            }
        }
        std::fprintf(stdout, "dj-ui: on a track with no grid the tools are there, the readers grey\n");

        // A beat placed at the play head: the grid starts there, at the tempo a deck shows when nothing
        // has been measured.
        constexpr qint64 target = 48'000;
        bridge->seek(QStringLiteral("A"), static_cast<qulonglong>(target));
        waitFor(
            [&]() { return deckA().value(QStringLiteral("source_frame")).toLongLong() == target; },
            5000);
        QMetaObject::invokeMethod(itemByName(QStringLiteral("gridHere_A")), "clicked");
        const auto placed = [&]() {
            return !gridOf().isEmpty() && firstBeat() == target;
        };
        if (!waitFor(placed, 5000)) {
            std::fprintf(stderr,
                         "dj-ui: placing a beat made no grid at the play head (first beat %lld)\n",
                         static_cast<long long>(firstBeat()));
            return 1;
        }
        if (std::abs(tempoOf() - 120.0) > 0.01) {
            std::fprintf(stderr, "dj-ui: the grid started at %.2f BPM instead of 120\n", tempoOf());
            return 1;
        }
        if (gridOf().value(QStringLiteral("beats_per_bar")).toInt() != 4) {
            std::fprintf(stderr, "dj-ui: the grid started in %d/4\n",
                         gridOf().value(QStringLiteral("beats_per_bar")).toInt());
            return 1;
        }
        std::fprintf(stdout, "dj-ui: a beat placed at frame %lld started a 120 BPM grid there\n",
                     static_cast<long long>(target));

        // A typed tempo on a track with no grid begins at the first frame: there is no beat to hold on
        // to, and the reading a DJ then shifts onto the first kick is the one that starts at zero. It is
        // measured on deck B, whose file has no grid: deck A has one now, and retiming an existing grid
        // keeps its first beat — a different rule, and the one that would hide this one.
        const QString second = parser.value(loadSecondOption);
        if (second.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --grid-create-check needs --load-b, a second track with no "
                                 "grid\n");
            return 1;
        }
        const auto deckB = [&]() { return deckById(bridge->snapshot(), QStringLiteral("B")); };
        const auto gridOfB = [&]() { return deckB().value(QStringLiteral("track")).toMap().value(
                                             QStringLiteral("grid")).toMap(); };
        const auto firstBeatB = [&]() -> qint64 {
            const QVariantList segments = gridOfB().value(QStringLiteral("segments")).toList();
            return segments.isEmpty()
                       ? -1
                       : segments.first().toMap().value(QStringLiteral("frame")).toLongLong();
        };
        const auto tempoOfB = [&]() {
            const QVariantList segments = gridOfB().value(QStringLiteral("segments")).toList();
            return segments.isEmpty()
                       ? 0.0
                       : segments.first().toMap().value(QStringLiteral("bpm")).toDouble();
        };
        bridge->eject(QStringLiteral("B"));
        waitFor([&]() { return deckB().value(QStringLiteral("track")).toMap().isEmpty(); }, 5000);
        bridge->load(QStringLiteral("B"), second);
        const QString secondName = QFileInfo(second).fileName();
        if (!waitFor(
                [&]() {
                    const QString held = deckB().value(QStringLiteral("path")).toString();
                    return !held.isEmpty() && held.endsWith(secondName);
                },
                30000)) {
            std::fprintf(stderr, "dj-ui: deck B never loaded %s\n", qPrintable(secondName));
            return 1;
        }
        if (!gridOfB().isEmpty()) {
            std::fprintf(stderr, "dj-ui: %s measured a grid of its own; this check needs a track "
                                 "with none\n",
                         qPrintable(secondName));
            return 1;
        }
        bridge->setGridTempo(QStringLiteral("B"), 174.0);
        const auto retimed = [&]() {
            return !gridOfB().isEmpty() && std::abs(tempoOfB() - 174.0) < 0.01 && firstBeatB() == 0;
        };
        if (!waitFor(retimed, 5000)) {
            std::fprintf(stderr,
                         "dj-ui: a typed tempo left the grid at %.2f BPM from frame %lld\n",
                         tempoOfB(), static_cast<long long>(firstBeatB()));
            return 1;
        }
        std::fprintf(stdout, "dj-ui: a typed tempo of 174 BPM started the grid at the file start\n");

        // A tenth of a beat per minute, up and down, from the tempo the grid now has.
        const auto tempoAfter = [&](double want) {
            return waitFor([&]() { return std::abs(tempoOfB() - want) < 0.005; }, 5000);
        };
        for (int press = 0; press < 2; ++press) {
            if (itemByName(QStringLiteral("gridTempoUp_B")) == nullptr) {
                std::fprintf(stderr, "dj-ui: deck B has no tempo-up button\n");
                return 1;
            }
            QMetaObject::invokeMethod(itemByName(QStringLiteral("gridTempoUp_B")), "clicked");
        }
        if (!tempoAfter(174.2)) {
            std::fprintf(stderr, "dj-ui: two presses up left the tempo at %.3f, not 174.2\n",
                         tempoOfB());
            return 1;
        }
        if (itemByName(QStringLiteral("gridTempoDown_B")) == nullptr) {
            std::fprintf(stderr, "dj-ui: deck B has no tempo-down button\n");
            return 1;
        }
        QMetaObject::invokeMethod(itemByName(QStringLiteral("gridTempoDown_B")), "clicked");
        if (!tempoAfter(174.1)) {
            std::fprintf(stderr, "dj-ui: a press down left the tempo at %.3f, not 174.1\n",
                         tempoOfB());
            return 1;
        }
        if (firstBeatB() != 0) {
            std::fprintf(stderr, "dj-ui: trimming the tempo moved the first beat to %lld\n",
                         static_cast<long long>(firstBeatB()));
            return 1;
        }
        std::fprintf(stdout, "dj-ui: the tempo buttons move it by 0.1 BPM and keep the first beat\n");

        // Both separators are accepted by the field the dialog itself fills with a dot: in a comma
        // locale the old validator refused the number it had just written.
        auto *editButton = itemByName(QStringLiteral("gridTempo_A"));
        if (editButton == nullptr) {
            std::fprintf(stderr, "dj-ui: no tempo field button on deck A\n");
            return 1;
        }
        QMetaObject::invokeMethod(editButton, "clicked");
        QCoreApplication::processEvents();
        auto *field = itemByName(QStringLiteral("tempoField_A"));
        if (field == nullptr) {
            std::fprintf(stderr, "dj-ui: the tempo field never appeared\n");
            return 1;
        }
        // Typed, not assigned: a comma that survives `setProperty` may still never arrive from a
        // keyboard, and the keyboard is what the DJ uses. The key events go to the focused item through
        // the window, exactly as a real press does.
        const auto typeInto = [&](const QString &text) {
            field->forceActiveFocus();
            field->setProperty("text", QString());
            QCoreApplication::processEvents();
            for (const QChar &character : text) {
                QKeyEvent press(QEvent::KeyPress, character.unicode(), Qt::NoModifier,
                                QString(character));
                QCoreApplication::sendEvent(window, &press);
                QKeyEvent release(QEvent::KeyRelease, character.unicode(), Qt::NoModifier,
                                  QString(character));
                QCoreApplication::sendEvent(window, &release);
            }
            QCoreApplication::processEvents();
            return field->property("text").toString();
        };
        for (const QString &typed : {QStringLiteral("174.5"), QStringLiteral("174,5")}) {
            const QString landed = typeInto(typed);
            if (landed != typed) {
                std::fprintf(stderr, "dj-ui: typing \"%s\" put \"%s\" in the tempo field\n",
                             qPrintable(typed), qPrintable(landed));
                return 1;
            }
            if (!field->property("acceptableInput").toBool()) {
                std::fprintf(stderr, "dj-ui: the tempo field refuses \"%s\"\n", qPrintable(typed));
                return 1;
            }
        }
        for (const QString &typed : {QStringLiteral("174.5.5"), QStringLiteral("174,5,5")}) {
            field->setProperty("text", typed);
            QCoreApplication::processEvents();
            if (field->property("acceptableInput").toBool()) {
                std::fprintf(stderr, "dj-ui: the tempo field accepts \"%s\"\n", qPrintable(typed));
                return 1;
            }
        }
        std::fprintf(stdout, "dj-ui: the tempo field takes a dot and a comma from the keyboard\n");

        // And it stays inside its own window: the text and the field used to be laid out at their own
        // width and ran out of the popup on the right.
        auto *label = itemByName(QStringLiteral("tempoEditLabel_A"));
        if (label == nullptr) {
            std::fprintf(stderr, "dj-ui: the tempo dialog has no label\n");
            return 1;
        }
        const auto rightEdge = [](QQuickItem *item) {
            return item->mapToScene(QPointF(item->width(), 0.0)).x();
        };
        const qreal windowWidth = static_cast<qreal>(window->width());
        std::fprintf(stdout, "dj-ui: the tempo dialog: label %.0f px, field %.0f px, window %.0f px\n",
                     label->width(), field->width(), windowWidth);
        if (rightEdge(label) > windowWidth || rightEdge(field) > windowWidth) {
            std::fprintf(stderr,
                         "dj-ui: the tempo dialog runs out of the window (label to %.0f, field to "
                         "%.0f, window %.0f)\n",
                         rightEdge(label), rightEdge(field), windowWidth);
            return 1;
        }
        if (label->width() > 280.0) {
            std::fprintf(stderr, "dj-ui: the tempo dialog label is %.0f px wide, wider than the "
                                 "popup\n",
                         label->width());
            return 1;
        }
        std::fprintf(stdout, "dj-ui: the tempo dialog keeps its content inside itself\n");
        std::fprintf(stdout, "dj-ui: a track with no grid can be given one by hand\n");
        return 0;
    }

    if (parser.isSet(gridCheckOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        const QString fixture = parser.value(loadOption);
        if (fixture.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --grid-check needs --load\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        const auto waitFor = [](const std::function<bool()> &ready, int budget) {
            QElapsedTimer timer;
            timer.start();
            while (timer.elapsed() < budget) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                if (ready()) {
                    return true;
                }
                QThread::msleep(10);
            }
            return ready();
        };
        if (!waitFor([&]() { return bridge->isConnected(); }, 3000)) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        const auto deckA = [&]() {
            const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant &entry : decks) {
                const QVariantMap deck = entry.toMap();
                if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    return deck;
                }
            }
            return QVariantMap();
        };
        // The grid as the deck publishes it: the same numbers the beat lines are drawn from.
        const auto gridOf = [&]() {
            return deckA()
                .value(QStringLiteral("track"))
                .toMap()
                .value(QStringLiteral("grid"))
                .toMap()
                .value(QStringLiteral("segments"))
                .toList();
        };
        const auto firstBeat = [&]() -> qint64 {
            const QVariantList segments = gridOf();
            if (segments.isEmpty()) {
                return -1;
            }
            return segments.first().toMap().value(QStringLiteral("frame")).toLongLong();
        };
        const auto tempoNow = [&]() {
            return deckA().value(QStringLiteral("local_bpm")).toDouble();
        };
        const auto click = [&](const QString &name) {
            auto *item = findItem(window->contentItem(), name);
            if (item == nullptr) {
                std::fprintf(stderr, "dj-ui: no %s to press\n", qPrintable(name));
                return false;
            }
            QMetaObject::invokeMethod(item, "clicked");
            QCoreApplication::processEvents();
            return true;
        };

        bridge->load(QStringLiteral("A"), fixture);
        // The deck keeps the previous track until the new one is ready, grid and all: waiting for "a
        // grid" means editing the file the previous run left behind.
        const QString wantName = QFileInfo(fixture).fileName();
        const auto loadArrived = [&]() {
            const QString held = deckA().value(QStringLiteral("path")).toString();
            return !held.isEmpty() && held.endsWith(wantName);
        };
        if (!waitFor([&]() { return loadArrived() && firstBeat() >= 0 && tempoNow() > 0.0; }, 20000)) {
            std::fprintf(stderr, "dj-ui: the deck published no grid to edit\n");
            return 1;
        }
        const qint64 start = firstBeat();
        const double tempo = tempoNow();
        // A step of ten milliseconds is the rate over a hundred, whatever the file's own rate is.
        const qint64 rate = deckA().value(QStringLiteral("source_rate")).toLongLong();
        if (rate <= 0) {
            std::fprintf(stderr, "dj-ui: the deck published no source rate\n");
            return 1;
        }

        const qint64 step = std::clamp<qint64>(rate / 100, 1, 100000);
        if (!click(QStringLiteral("gridRight_A"))) {
            return 1;
        }
        if (!waitFor([&]() { return firstBeat() == start + step; }, 5000)) {
            std::fprintf(stderr, "dj-ui: the shift did not move the grid by ten milliseconds (%lld -> %lld)\n",
                         static_cast<long long>(start), static_cast<long long>(firstBeat()));
            return 1;
        }
        if (!click(QStringLiteral("gridLeft_A"))) {
            return 1;
        }
        if (!waitFor([&]() { return firstBeat() == start; }, 5000)) {
            std::fprintf(stderr, "dj-ui: the grid did not come back\n");
            return 1;
        }
        // A beat a press: one period of the tempo under the play head, and back again. A grid that starts
        // on the wrong beat needs a beat, not the ten milliseconds of the arrows.
        const qint64 beat = std::llround(double(rate) * 60.0 / tempo);
        if (!click(QStringLiteral("gridBeatRight_A"))) {
            return 1;
        }
        if (!waitFor([&]() { return firstBeat() == start + beat; }, 5000)) {
            std::fprintf(stderr, "dj-ui: the beat shift moved the grid to %lld instead of %lld\n",
                         static_cast<long long>(firstBeat()),
                         static_cast<long long>(start + beat));
            return 1;
        }
        // Размер такта: замер отдаёт четыре, DJ ставит три и возвращает обратно. Счёт такта на деке
        // читает то же поле снапшота, так что проверка смотрит ровно то, что видит глаз.
        const auto metreNow = [&]() {
            return deckA()
                .value(QStringLiteral("track"))
                .toMap()
                .value(QStringLiteral("grid"))
                .toMap()
                .value(QStringLiteral("beats_per_bar"))
                .toInt();
        };
        const int metreBefore = metreNow();
        // Меню размера встаёт у кнопки: владелец находил его в левом верхнем углу окна, потому что
        // всплывающее окно без места открывается в начало координат. Само окно стенду недоступно, но
        // панель отдаёт его координаты — они и сравниваются с положением кнопки.
        if (QQuickItem *metreButton = findItem(window->contentItem(), QStringLiteral("gridMetre_A"))) {
            // A button the DJ cannot see cannot be pressed: the menu is measured against it, so its own
            // visibility is part of the premise.
            if (!metreButton->isVisible()) {
                std::fprintf(stderr, "dj-ui: the metre button is in the scene but not visible\n");
                return 1;
            }
            if (!click(QStringLiteral("gridMetre_A"))) {
                return 1;
            }
            // Дать меню раскрыться: waitFor ждёт всё окно, когда условие не выполняется.
            (void)waitFor([&]() { return false; }, 120);
            QQuickItem *deckPanel = findItem(window->contentItem(), QStringLiteral("deckPanel_A"));
            if (deckPanel == nullptr) {
                std::fprintf(stderr, "dj-ui: the deck panel is not there to ask about the menu\n");
                return 1;
            }
            QVariant at;
            if (!QMetaObject::invokeMethod(deckPanel, "metreMenuAt", Q_RETURN_ARG(QVariant, at))) {
                std::fprintf(stderr, "dj-ui: the panel does not say where the metre menu is\n");
                return 1;
            }
            const QVariantMap atMap = at.toMap();
            const double menuX = atMap.value(QStringLiteral("x")).toDouble();
            const double menuY = atMap.value(QStringLiteral("y")).toDouble();
            // Место считается от самой кнопки: левый верх меню — на её левом краю, верх — на её низу.
            // Высоту кнопки отдаёт QML: там же, где живёт само меню, — иначе сравниваются два разных
            // представления об одном предмете.
            const double wantX = 0.0;
            const double wantY = atMap.value(QStringLiteral("buttonHeight")).toDouble();
            // Вплотную: меню — сосед кнопки, а не что-то «примерно там же». Два пикселя на рамку
            // всплывающего окна, не больше.
            if (std::abs(menuX - wantX) > 2.0 || std::abs(menuY - wantY) > 2.0) {
                std::fprintf(stderr,
                             "dj-ui: the metre menu opened at %.0f,%.0f instead of the button at %.0f,%.0f\n",
                             menuX,
                             menuY,
                             wantX,
                             wantY);
                return 1;
            }
            std::fprintf(stderr, "dj-ui: the metre menu sits at %.0f,%.0f under the button at %.0f,%.0f\n",
                         menuX, menuY, wantX, wantY);
        }

        if (bridge->setGridMetre(QStringLiteral("A"), 3) < 0) {
            std::fprintf(stderr, "dj-ui: the metre change was refused\n");
            return 1;
        }
        if (!waitFor([&]() { return metreNow() == 3; }, 5000)) {
            std::fprintf(stderr, "dj-ui: the bar reads %d instead of 3\n", metreNow());
            return 1;
        }
        if (bridge->setGridMetre(QStringLiteral("A"), metreBefore) < 0
            || !waitFor([&]() { return metreNow() == metreBefore; }, 5000)) {
            std::fprintf(stderr, "dj-ui: the bar did not come back to %d\n", metreBefore);
            return 1;
        }

        // Инструменты переменной сетки: разбить участок, задать ему темп, убрать его. Команды те же,
        // что шлют кнопки ряда и диалог BPM; кадр берётся из сетки, а не из головы, — этой проверке
        // нужен участок, а не позиция транспорта.
        const int segmentsBefore = int(gridOf().size());
        const qint64 deeper = start + beat * 64;
        if (bridge->splitGrid(QStringLiteral("A"), deeper) < 0) {
            std::fprintf(stderr, "dj-ui: the split was refused\n");
            return 1;
        }
        if (!waitFor([&]() { return int(gridOf().size()) == segmentsBefore + 1; }, 5000)) {
            std::fprintf(stderr, "dj-ui: the split left %d stretches instead of %d\n",
                         int(gridOf().size()), segmentsBefore + 1);
            return 1;
        }
        // Темп участка: та же функция, которую зовёт диалог BPM и стрелки по 0,1.
        if (bridge->setGridStretchTempo(QStringLiteral("A"), deeper, 101.0) < 0) {
            std::fprintf(stderr, "dj-ui: the stretch tempo was refused\n");
            return 1;
        }
        const auto stretchTempo = [&](int index) {
            const QVariantList segments = gridOf();
            if (index >= segments.size()) {
                return 0.0;
            }
            return segments.at(index).toMap().value(QStringLiteral("bpm")).toDouble();
        };
        if (!waitFor([&]() { return std::fabs(stretchTempo(1) - 101.0) < 0.6; }, 5000)) {
            std::fprintf(stderr, "dj-ui: the second stretch reads %.2f BPM instead of 101\n",
                         stretchTempo(1));
            return 1;
        }
        if (!waitFor([&]() { return std::fabs(stretchTempo(0) - tempo) < 0.6; }, 5000)) {
            std::fprintf(stderr, "dj-ui: the first stretch moved to %.2f BPM\n", stretchTempo(0));
            return 1;
        }
        if (bridge->dropGridStretch(QStringLiteral("A"), deeper) < 0) {
            std::fprintf(stderr, "dj-ui: the drop was refused\n");
            return 1;
        }
        if (!waitFor([&]() { return int(gridOf().size()) == segmentsBefore; }, 5000)) {
            std::fprintf(stderr, "dj-ui: dropping the stretch left %d stretches instead of %d\n",
                         int(gridOf().size()), segmentsBefore);
            return 1;
        }

        if (!click(QStringLiteral("gridBeatLeft_A"))) {
            return 1;
        }
        if (!waitFor([&]() { return firstBeat() == start; }, 5000)) {
            std::fprintf(stderr, "dj-ui: the beat shift back did not return the grid\n");
            return 1;
        }

        // Half, then double: the same grid read twice as slow and then back.
        if (!click(QStringLiteral("gridHalf_A"))) {
            return 1;
        }
        if (!waitFor([&]() { return std::abs(tempoNow() - tempo / 2.0) < 0.05; }, 5000)) {
            std::fprintf(stderr, "dj-ui: half did not halve the tempo (%.2f)\n", tempoNow());
            return 1;
        }
        if (!click(QStringLiteral("gridDouble_A"))) {
            return 1;
        }
        if (!waitFor([&]() { return std::abs(tempoNow() - tempo) < 0.05; }, 5000)) {
            std::fprintf(stderr, "dj-ui: double did not bring the tempo back (%.2f)\n", tempoNow());
            return 1;
        }

        // A tempo typed by hand: the number the DJ knows. The question is built when it is first
        // opened, so the field only exists after the button has been pressed.
        if (!click(QStringLiteral("gridTempo_A"))) {
            return 1;
        }
        auto *field = findItem(window->contentItem(), QStringLiteral("tempoField_A"));
        if (field == nullptr) {
            std::fprintf(stderr, "dj-ui: the tempo field was never created\n");
            return 1;
        }
        field->setProperty("text", QStringLiteral("128.0"));
        if (!click(QStringLiteral("tempoApply_A"))) {
            return 1;
        }
        if (!waitFor([&]() { return std::abs(tempoNow() - 128.0) < 0.05; }, 5000)) {
            std::fprintf(stderr, "dj-ui: the typed tempo did not reach the deck (%.2f)\n", tempoNow());
            return 1;
        }

        // Every edit is a decision about the file: it is written beside the music. The lock is the DJ's
        // switch and an edit does not touch it — the saved grid is what keeps the next automatic pass
        // away, and a flag set here would grey the editor after the first edit.
        const QString sidecar =
            QFileInfo(fixture).absolutePath() + QStringLiteral("/.djmeta/") +
            QFileInfo(fixture).fileName() + QStringLiteral(".json");
        const auto stored = [&]() {
            QFile file(sidecar);
            if (!file.open(QIODevice::ReadOnly)) {
                return QJsonObject();
            }
            return QJsonDocument::fromJson(file.readAll()).object();
        };
        if (!waitFor(
                [&]() {
                    const QJsonObject grid = stored().value(QStringLiteral("beatgrid")).toObject();
                    return std::abs(grid.value(QStringLiteral("bpm")).toDouble() - 128.0) < 0.05 &&
                           !stored().value(QStringLiteral("analysis_lock")).toBool();
                },
                5000)) {
            std::fprintf(stderr, "dj-ui: the edit never reached %s\n", qPrintable(sidecar));
            return 1;
        }
        const QJsonObject grid = stored().value(QStringLiteral("beatgrid")).toObject();
        std::fprintf(stdout,
                     "dj-ui: the grid moved a beat and back, halved, doubled and took a typed tempo: "
                     "%.2f -> 128.00 BPM, "
                     "first beat %lld, file revision %d, locked\n",
                     tempo, static_cast<long long>(grid.value(QStringLiteral("first_beat_frame"))
                                                       .toVariant()
                                                       .toLongLong()),
                     stored().value(QStringLiteral("revision")).toInt());
        // The rows under the wave are part of what this check promises: the strip folds, and folding it
        // gives the wave its height back.
        if (const int rows = measureDeckRows(window, QStringLiteral("A")); rows != 0) {
            return rows;
        }
        return 0;
    }

    if (parser.isSet(eqCheckOption)) {
        return runEqCheck(engine, bridge, parser, connectOption);
    }






    if (parser.isSet(loopOption)) {
        return runLoop(engine, bridge, parser, connectOption, loadOption);
    }

    if (parser.isSet(scenarioOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        QElapsedTimer linking;
        linking.start();
        while (!bridge->isConnected() && linking.elapsed() < 3000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(10);
        }
        if (!bridge->isConnected()) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        window->resize(1366, 768);
        window->show();
        const QString path = parser.value(loadOption);
        if (path.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --scenario-check needs --load\n");
            return 1;
        }
        bridge->load(QStringLiteral("A"), path);
        return djui::runScenarios(*bridge, *window, QStringLiteral("A"), path);
    }

    if (parser.isSet(padsOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        QElapsedTimer linking;
        linking.start();
        while (!bridge->isConnected() && linking.elapsed() < 3000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(10);
        }
        if (!bridge->isConnected()) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        window->resize(1366, 768);
        window->show();
        const auto pump = [&](int rounds) {
            for (int step = 0; step < rounds; ++step) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 10);
                QThread::msleep(5);
            }
        };
        pump(60);
        const QString path = parser.value(loadOption);
        if (path.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --pads-check needs --load\n");
            return 1;
        }
        bridge->load(QStringLiteral("A"), path);
        // The loader decodes the file and measures its loudness before the graph attaches it, and an
        // attach puts the playhead at the start: a seek sent into that window is undone a moment later.
        pump(120);
        QElapsedTimer ready;
        ready.start();
        while (ready.elapsed() < 15000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(20);
            bool loaded = false;
            for (const QVariant &entry :
                 bridge->snapshot().value(QStringLiteral("decks")).toList()) {
                const QVariantMap map = entry.toMap();
                if (map.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    loaded = map.value(QStringLiteral("remaining_frames")).toDouble() > 0.0;
                }
            }
            if (loaded) {
                break;
            }
        }

        const auto deckOf = [&]() {
            for (const QVariant &entry : bridge->snapshot().value(QStringLiteral("decks")).toList()) {
                const QVariantMap map = entry.toMap();
                if (map.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    return map;
                }
            }
            return QVariantMap();
        };
        const auto click = [&](QQuickItem *item, Qt::KeyboardModifiers modifiers) {
            if (item == nullptr) {
                return false;
            }
            const QPointF at = item->mapToScene(QPointF(item->width() / 2.0, item->height() / 2.0));
            for (const QEvent::Type type : {QEvent::MouseButtonPress, QEvent::MouseButtonRelease}) {
                QMouseEvent event(type, at, window->mapToGlobal(at.toPoint()), Qt::LeftButton,
                                  Qt::LeftButton, modifiers);
                QCoreApplication::sendEvent(window, &event);
                QCoreApplication::processEvents();
            }
            pump(30);
            return true;
        };

        // Park the playhead somewhere recognisable, then set pad 3 on it. The seek is repeated until it
        // sticks: a load still landing resets the playhead to the start of the track, and a check that
        // raced it would store the cue at zero and blame the pad.
        double parked = 0.0;
        for (int attempt = 0; attempt < 20; ++attempt) {
            bridge->seek(QStringLiteral("A"), 96000);
            pump(20);
            parked = deckOf().value(QStringLiteral("source_frame")).toDouble();
            if (std::abs(parked - 96000.0) < 512.0) {
                break;
            }
        }
        std::fprintf(stdout, "dj-ui: the playhead stands at %.0f before the pad is clicked\n", parked);
        if (std::abs(parked - 96000.0) > 512.0) {
            std::fprintf(stderr, "dj-ui: the playhead could not be parked for the check\n");
            return 1;
        }
        auto *pad = findItem(window->contentItem(), QStringLiteral("cuepad_A_3"));
        if (pad == nullptr) {
            std::fprintf(stderr, "dj-ui: deck A has no pad 3\n");
            return 1;
        }
        {
            // Control hand: the same command without the mouse. If this one also lands at zero, the
            // pad is not the suspect.
            bridge->cue(QStringLiteral("A"), QStringLiteral("Set"), 5);
            pump(30);
            const QVariantMap controlled = deckOf();
            const QVariantList cuesNow = controlled.value(QStringLiteral("hotcues")).toList();
            std::fprintf(stdout,
                         "dj-ui: the same command without the mouse put pad 5 at %.0f with the "
                         "playhead at %.0f\n",
                         cuesNow.size() > 5 && !cuesNow[5].isNull() ? cuesNow[5].toDouble() : -1.0,
                         controlled.value(QStringLiteral("source_frame")).toDouble());
        }
        std::fprintf(stdout, "dj-ui: pad 3 sits at %.0f,%.0f %.0fx%.0f in a %dx%d window\n",
                     pad->mapToScene(QPointF(0, 0)).x(), pad->mapToScene(QPointF(0, 0)).y(),
                     pad->width(), pad->height(), window->width(), window->height());
        click(pad, Qt::NoModifier);
        {
            const QVariantMap after = deckOf();
            const QVariantList cuesNow = after.value(QStringLiteral("hotcues")).toList();
            QStringList parts;
            for (const QVariant &cue : cuesNow) {
                parts << (cue.isNull() ? QStringLiteral("—") : QString::number(cue.toDouble()));
            }
            std::fprintf(stdout, "dj-ui: after the click the deck's load state is %s\n",
                         qPrintable(QJsonDocument::fromVariant(
                                        after.value(QStringLiteral("load")))
                                        .toJson(QJsonDocument::Compact)));
            std::fprintf(stdout, "dj-ui: after the click the pads read [%s] at playhead %.0f\n",
                         qPrintable(parts.join(QStringLiteral(", "))),
                         after.value(QStringLiteral("source_frame")).toDouble());
        }
        const QVariantMap set = deckOf();
        const QVariantList cues = set.value(QStringLiteral("hotcues")).toList();
        const double stored = cues.size() > 3 && cues[3].isValid() && !cues[3].isNull()
                                  ? cues[3].toDouble() : -1.0;
        std::fprintf(stdout, "dj-ui: clicking an empty pad 3 stored a cue at %.0f\n", stored);
        if (std::abs(stored - 96000.0) > 2000.0) {
            std::fprintf(stderr, "dj-ui: the pad did not store the playhead\n");
            return 1;
        }

        // A name and a colour travel through the engine and come back in the snapshot.
        bridge->setCueMeta(QStringLiteral("A"), 3, QStringLiteral("Drop"),
                           QStringLiteral("#5fb85f"));
        pump(40);
        const QVariantMap named = deckOf();
        const QVariantList labels = named.value(QStringLiteral("cue_labels")).toList();
        const QVariantList colors = named.value(QStringLiteral("cue_colors")).toList();
        const QString label = labels.size() > 3 ? labels[3].toString() : QString();
        const QString color = colors.size() > 3 ? colors[3].toString() : QString();
        std::fprintf(stdout, "dj-ui: the pad is called %s and coloured %s\n", qPrintable(label),
                     qPrintable(color));
        if (label != QStringLiteral("Drop") || color != QStringLiteral("#5fb85f")) {
            std::fprintf(stderr, "dj-ui: the pad's name or colour did not survive the round trip\n");
            return 1;
        }

        // The strip has to be told about the pad: drawing is the point of storing it.
        {
            auto *strip = findItem(window->contentItem(), QStringLiteral("waveStrip_A"));
            const QVariantList marks = strip != nullptr ? strip->property("cues").toList()
                                                        : QVariantList();
            std::fprintf(stdout, "dj-ui: the waveform strip carries %d pad mark(s)\n",
                         int(marks.size()));
            if (marks.isEmpty()) {
                std::fprintf(stderr, "dj-ui: the pad did not reach the waveform strip\n");
                return 1;
            }
            const QVariantMap first = marks.first().toMap();
            const QString colour = first.value(QStringLiteral("color")).toString();
            const QString label = first.value(QStringLiteral("label")).toString();
            std::fprintf(stdout, "dj-ui: the mark wears %s and is called %s at %.3f of the track\n",
                         qPrintable(colour), qPrintable(label),
                         first.value(QStringLiteral("ratio")).toDouble());
            if (colour != QStringLiteral("#5fb85f") || label != QStringLiteral("Drop")) {
                std::fprintf(stderr, "dj-ui: the mark lost its colour or its name on the way\n");
                return 1;
            }
        }

        // Clicking a set pad jumps to it.
        bridge->seek(QStringLiteral("A"), 10000);
        pump(30);
        click(pad, Qt::NoModifier);
        const QVariantMap jumped = deckOf();
        const double landed = jumped.value(QStringLiteral("source_frame")).toDouble();
        std::fprintf(stdout, "dj-ui: clicking the pad again moved the playhead to %.0f\n", landed);
        if (std::abs(landed - stored) > 2000.0) {
            std::fprintf(stderr, "dj-ui: the pad did not jump to its cue\n");
            return 1;
        }

        // Shift-click clears it.
        click(pad, Qt::ShiftModifier);
        const QVariantMap cleared = deckOf();
        const QVariantList after = cleared.value(QStringLiteral("hotcues")).toList();
        const bool stillThere = after.size() > 3 && after[3].isValid() && !after[3].isNull();
        std::fprintf(stdout, "dj-ui: shift-click left pad 3 %s\n", stillThere ? "set" : "empty");
        if (stillThere) {
            std::fprintf(stderr, "dj-ui: shift-click did not clear the pad\n");
            return 1;
        }

        // A pad that holds a loop is deleted the same way: the loop goes with the rest of the pad.
        bridge->setLoop(QStringLiteral("A"), true, 96000, 144000);
        pump(30);
        bridge->setPadLoop(QStringLiteral("A"), 4);
        pump(40);
        const QVariantList loopsSet = deckOf().value(QStringLiteral("pad_loops")).toList();
        const bool heldLoop = loopsSet.size() > 4 && loopsSet[4].isValid() && !loopsSet[4].isNull();
        std::fprintf(stdout, "dj-ui: pad 4 %s a loop\n", heldLoop ? "holds" : "does not hold");
        if (!heldLoop) {
            std::fprintf(stderr, "dj-ui: the loop did not reach pad 4\n");
            return 1;
        }
        auto *padFour = findItem(window->contentItem(), QStringLiteral("cuepad_A_4"));
        if (padFour == nullptr) {
            std::fprintf(stderr, "dj-ui: deck A has no pad 4\n");
            return 1;
        }
        click(padFour, Qt::ShiftModifier);
        pump(30);
        const QVariantList loopsAfter = deckOf().value(QStringLiteral("pad_loops")).toList();
        const bool loopLeft = loopsAfter.size() > 4 && loopsAfter[4].isValid() && !loopsAfter[4].isNull();
        const int armedLeft = int(deckOf().value(QStringLiteral("active_loops")).toList().size());
        std::fprintf(stdout, "dj-ui: shift-click left pad 4 %s loop, %d loop(s) armed\n",
                     loopLeft ? "holding a" : "without a", armedLeft);
        if (loopLeft) {
            std::fprintf(stderr, "dj-ui: shift-click did not clear the loop on the pad\n");
            return 1;
        }
        return 0;
    }

    if (parser.isSet(transportOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        QElapsedTimer linking;
        linking.start();
        while (!bridge->isConnected() && linking.elapsed() < 3000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(10);
        }
        if (!bridge->isConnected()) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        window->resize(1366, 768);
        window->show();
        const auto pump = [&](int rounds) {
            for (int step = 0; step < rounds; ++step) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 10);
                QThread::msleep(5);
            }
        };
        pump(60);

        const auto fieldOf = [&](const char *field) {
            for (const QVariant &entry : bridge->snapshot().value(QStringLiteral("decks")).toList()) {
                const QVariantMap map = entry.toMap();
                if (map.value(QStringLiteral("deck")).toString() != QStringLiteral("A")) {
                    continue;
                }
                return map.value(QString::fromLatin1(field)).toDouble();
            }
            return -1.0;
        };
        const auto playing = [&]() {
            for (const QVariant &entry : bridge->snapshot().value(QStringLiteral("decks")).toList()) {
                const QVariantMap map = entry.toMap();
                if (map.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    return map.value(QStringLiteral("playing")).toBool();
                }
            }
            return false;
        };
        // The point Stop returns to is hotcue 0 when the track carries one, the start of the file
        // otherwise: the check reads the deck's own answer instead of assuming zero.
        const auto cuePoint = [&]() {
            for (const QVariant &entry : bridge->snapshot().value(QStringLiteral("decks")).toList()) {
                const QVariantMap map = entry.toMap();
                if (map.value(QStringLiteral("deck")).toString() != QStringLiteral("A")) {
                    continue;
                }
                // `slots` is a Qt keyword: the list is named for what it holds.
                const QVariantList cues = map.value(QStringLiteral("hotcues")).toList();
                if (!cues.isEmpty() && cues[0].isValid() && !cues[0].isNull()) {
                    return cues[0].toDouble();
                }
                return 0.0;
            }
            return 0.0;
        };
        const auto press = [&](QQuickItem *item) {
            if (item == nullptr) {
                return false;
            }
            const QPointF at = item->mapToScene(QPointF(item->width() / 2.0, item->height() / 2.0));
            QMouseEvent down(QEvent::MouseButtonPress, at, window->mapToGlobal(at.toPoint()),
                             Qt::LeftButton, Qt::LeftButton, Qt::NoModifier);
            QCoreApplication::sendEvent(window, &down);
            QCoreApplication::processEvents();
            return true;
        };
        const auto release = [&](QQuickItem *item) {
            if (item == nullptr) {
                return false;
            }
            const QPointF at = item->mapToScene(QPointF(item->width() / 2.0, item->height() / 2.0));
            QMouseEvent up(QEvent::MouseButtonRelease, at, window->mapToGlobal(at.toPoint()),
                           Qt::LeftButton, Qt::LeftButton, Qt::NoModifier);
            QCoreApplication::sendEvent(window, &up);
            QCoreApplication::processEvents();
            return true;
        };

        // Loaded already by --load; wait for the deck to carry a track.
        const QString path = parser.value(loadOption);
        if (path.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --transport-check needs --load\n");
            return 1;
        }
        // A check of the transport needs a track: the load is part of the setup, not something the
        // production code does on its own.
        bridge->load(QStringLiteral("A"), path);
        QElapsedTimer ready;
        ready.start();
        while (ready.elapsed() < 15000 && fieldOf("remaining_frames") <= 0.0) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(20);
        }
        // A fresh engine is not guaranteed: the checks of a gate run share one, so the deck is put in a
        // known state before the transport is measured — silent, at its cue point.
        bridge->play(QStringLiteral("A"), false);
        pump(30);
        bridge->seek(QStringLiteral("A"), 0);
        pump(30);
        // The snapshot carries the position and what is left of it; their sum is the track.
        const double totalFrames = fieldOf("source_frame")
                                   + fieldOf("remaining_frames");
        auto *playButton = findItem(window->contentItem(), QStringLiteral("play_A"));
        auto *cueButton = findItem(window->contentItem(), QStringLiteral("cue_A"));
        auto *overview = findItem(window->contentItem(), QStringLiteral("deckOverview_A"));
        auto *clock = findItem(window->contentItem(), QStringLiteral("clock_A"));
        if (playButton == nullptr || cueButton == nullptr || overview == nullptr
            || clock == nullptr) {
            std::fprintf(stderr, "dj-ui: the deck is missing its transport, its clock or its overview\n");
            return 1;
        }

        // Play, let the playhead move, then a tap of Cue: the head must be back on the cue point. The
        // Stop button is gone (owner's decision): returning the head is the cue button's job, and the
        // pause is the Play button's.
        press(playButton);
        pump(5);
        release(playButton);
        pump(60);
        if (!playing()) {
            std::fprintf(stderr, "dj-ui: the play button did not start the deck\n");
            return 1;
        }
        const double moved = fieldOf("source_frame");
        const double cue = cuePoint();
        press(cueButton);
        pump(10);
        release(cueButton);
        pump(20);
        // Freeze the deck before reading the position: a tap of Cue brings the head back and the deck
        // rolls on from the point, and a rolling deck is no position to compare. Pausing is the Play
        // button's job now that Stop is gone.
        press(playButton);
        pump(5);
        release(playButton);
        pump(40);
        const double backAt = fieldOf("source_frame");
        std::fprintf(stdout,
                     "dj-ui: play moved the playhead to %.0f, a tap of Cue brought it back to %.0f and "
                     "left the deck %s (the cue point is %.0f)\n",
                     moved, backAt, playing() ? "playing" : "paused", cue);
        if (playing()) {
            std::fprintf(stderr, "dj-ui: the deck kept playing after Play was pressed\n");
            return 1;
        }
        if (backAt < cue - 2.0 || backAt - cue > 24000.0) {
            std::fprintf(stderr,
                         "dj-ui: the tap of Cue left the playhead at %.0f instead of the cue point "
                         "%.0f\n",
                         backAt, cue);
            return 1;
        }
        if (backAt >= moved - 1000.0) {
            std::fprintf(stderr, "dj-ui: the tap of Cue did not bring the playhead back\n");
            return 1;
        }
        // The hold of the same button sets the point where the hand landed and leaves the deck there.
        press(playButton);
        pump(5);
        release(playButton);
        pump(300);
        const double handLanded = fieldOf("source_frame");
        press(cueButton);
        pump(600);
        release(cueButton);
        pump(80);
        const double leftAt = fieldOf("source_frame");
        const double pointNow = cuePoint();
        std::fprintf(stdout,
                     "dj-ui: holding Cue set the point at %.0f (the hand landed at %.0f), the head is "
                     "at %.0f and the deck is %s\n",
                     pointNow, handLanded, leftAt, playing() ? "playing" : "silent");
        if (pointNow <= 0.0 || std::abs(pointNow - handLanded) > 10000.0) {
            std::fprintf(stderr, "dj-ui: the hold did not set the cue point where the hand landed\n");
            return 1;
        }
        if (playing()) {
            std::fprintf(stderr, "dj-ui: the deck kept playing after the cue point was set\n");
            return 1;
        }
        if (std::abs(leftAt - pointNow) > 2.0) {
            std::fprintf(stderr, "dj-ui: the deck did not stand on the point it had just set\n");
            return 1;
        }

        // Scrub: drag the hand across the overview and require the engine's playhead to follow.
        const QPointF from = overview->mapToScene(QPointF(overview->width() * 0.2, overview->height() / 2));
        const QPointF to = overview->mapToScene(QPointF(overview->width() * 0.6, overview->height() / 2));
        QMouseEvent down(QEvent::MouseButtonPress, from, window->mapToGlobal(from.toPoint()),
                         Qt::LeftButton, Qt::LeftButton, Qt::NoModifier);
        QCoreApplication::sendEvent(window, &down);
        QMouseEvent along(QEvent::MouseMove, to, window->mapToGlobal(to.toPoint()), Qt::LeftButton,
                          Qt::LeftButton, Qt::NoModifier);
        QCoreApplication::sendEvent(window, &along);
        QMouseEvent up(QEvent::MouseButtonRelease, to, window->mapToGlobal(to.toPoint()),
                       Qt::LeftButton, Qt::LeftButton, Qt::NoModifier);
        QCoreApplication::sendEvent(window, &up);
        pump(40);
        const double scrubbed = fieldOf("source_frame");
        const double total = totalFrames;
        std::fprintf(stdout, "dj-ui: scrubbing to sixty per cent of the overview put the playhead at "
                             "%.0f of %.0f frames\n", scrubbed, total);
        if (total <= 0.0 || scrubbed < total * 0.45 || scrubbed > total * 0.75) {
            std::fprintf(stderr, "dj-ui: the scrub did not move the playhead where the hand went\n");
            return 1;
        }

        // Scrubbing by hand on the zoomed strip: dragging to the right takes the material back, and in
        // the default silent mode the transport is left alone.
        {
            auto *strip = findItem(window->contentItem(), QStringLiteral("waveStrip_A"));
            if (strip == nullptr) {
                std::fprintf(stderr, "dj-ui: deck A has no waveform strip\n");
                return 1;
            }
            const bool wasPlaying = playing();
            const double before = fieldOf("source_frame");
            const QPointF grab =
                strip->mapToScene(QPointF(strip->width() * 0.5, strip->height() * 0.5));
            const QPointF moved = grab + QPointF(60, 0);
            for (const auto &[type, point] :
                 {std::pair{QEvent::MouseButtonPress, grab}, std::pair{QEvent::MouseMove, moved},
                  std::pair{QEvent::MouseButtonRelease, moved}}) {
                QMouseEvent event(type, point, window->mapToGlobal(point.toPoint()), Qt::LeftButton,
                                  Qt::LeftButton, Qt::NoModifier);
                QCoreApplication::sendEvent(window, &event);
                QCoreApplication::processEvents();
            }
            pump(40);
            const double after = fieldOf("source_frame");
            std::fprintf(stdout,
                         "dj-ui: dragging the strip 60 px right moved the playhead from %.0f to "
                         "%.0f, transport %s\n",
                         before, after, playing() == wasPlaying ? "unchanged" : "changed");
            if (after >= before - 500.0) {
                std::fprintf(stderr, "dj-ui: the hand on the strip did not move the material back\n");
                return 1;
            }
            if (playing() != wasPlaying) {
                std::fprintf(stderr, "dj-ui: a silent scrub changed the transport\n");
                return 1;
            }
        }

        // A silent deck with the sound switched on: the noise must come from the hand, not from a deck
        // that quietly started playing. Moving makes sound; stopping must be silent again.
        {
            auto *strip = findItem(window->contentItem(), QStringLiteral("waveStrip_A"));
            if (strip == nullptr) {
                std::fprintf(stderr, "dj-ui: deck A has no strip\n");
                return 1;
            }
            // The mode is a setting now, and the check asks the settings for it — which is the way the
            // DJ gets it too (owner's decision, 14.11: no button on the deck).
            settings.setScrubAudible(true);
            if (!settings.scrubAudible()) {
                std::fprintf(stderr, "dj-ui: the scrub mode will not switch to audible\n");
                return 1;
            }
            pump(20);
            if (playing()) {
                std::fprintf(stderr, "dj-ui: the deck was playing before the audio scrub began\n");
                return 1;
            }

            const QPointF grab =
                strip->mapToScene(QPointF(strip->width() * 0.5, strip->height() * 0.5));
            QMouseEvent down(QEvent::MouseButtonPress, grab, window->mapToGlobal(grab.toPoint()),
                             Qt::LeftButton, Qt::LeftButton, Qt::NoModifier);
            QCoreApplication::sendEvent(window, &down);
            QCoreApplication::processEvents();
            pump(4);
            const double beforeMove = fieldOf("source_frame");
            // A hand that has not moved yet makes no sound.
            pump(20);
            if (playing()) {
                std::fprintf(stderr, "dj-ui: holding the strip started the deck\n");
                return 1;
            }
            const QPointF along = grab + QPointF(40, 0);
            QMouseEvent move(QEvent::MouseMove, along, window->mapToGlobal(along.toPoint()),
                             Qt::LeftButton, Qt::LeftButton, Qt::NoModifier);
            QCoreApplication::sendEvent(window, &move);
            QCoreApplication::processEvents();
            const double afterMove = fieldOf("source_frame");
            // The hand has moved and stops; the button is still down. Nothing may run on: whatever the
            // scratch sounded like, the playhead has to stay where the hand left it.
            QElapsedTimer held;
            held.start();
            while (held.elapsed() < 400) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 10);
                QThread::msleep(10);
            }
            const double afterHold = fieldOf("source_frame");
            QMouseEvent up(QEvent::MouseButtonRelease, along, window->mapToGlobal(along.toPoint()),
                           Qt::LeftButton, Qt::LeftButton, Qt::NoModifier);
            QCoreApplication::sendEvent(window, &up);
            pump(20);
            const double afterRelease = fieldOf("source_frame");
            const bool quietAfter = !playing();
            std::fprintf(stdout,
                         "dj-ui: audio scrub — the hand moved the playhead to %.0f, 400 ms of a held "
                         "button left it at %.0f, releasing left it at %.0f, deck %s\n",
                         afterMove, afterHold, afterRelease, quietAfter ? "silent" : "playing");
            // Whether the hand moves the material is proved by the silent drag above; what matters here
            // is that nothing keeps running after the hand stops.
            (void)beforeMove;
            if (std::abs(afterHold - afterMove) > 4800.0) {
                std::fprintf(stderr, "dj-ui: the deck ran on after the hand stopped\n");
                return 1;
            }
            if (!quietAfter || std::abs(afterRelease - afterHold) > 4800.0) {
                std::fprintf(stderr, "dj-ui: releasing the hand did not leave the deck where it stood\n");
                return 1;
            }
        }

        // The clock counts down, so it must show the time left, not the length.
        const QString shown = clock->property("text").toString();
        std::fprintf(stdout, "dj-ui: the deck's clock reads %s\n", qPrintable(shown));
        if (!shown.contains(QStringLiteral("−")) || shown.contains(QStringLiteral("—"))) {
            std::fprintf(stderr, "dj-ui: the clock does not show the time left\n");
            return 1;
        }
        return 0;
    }

    if (parser.isSet(fourDeckOption)) {
        // Four decks are a claim about the whole chain: four deck objects, four mixer channels, four
        // strips on screen, and a crossfader that knows which side each one is on. This check walks the
        // chain once, from the file to the master.
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        const auto waitFor = [](const std::function<bool()> &ready, int budget) {
            QElapsedTimer timer;
            timer.start();
            while (timer.elapsed() < budget) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                if (ready()) {
                    return true;
                }
                QThread::msleep(10);
            }
            return ready();
        };
        if (!waitFor([&]() { return bridge->isConnected(); }, 3000)) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        const auto deckOf = [&](const QString &id) {
            const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant &entry : decks) {
                const QVariantMap deck = entry.toMap();
                if (deck.value(QStringLiteral("deck")).toString() == id) {
                    return deck;
                }
            }
            return QVariantMap();
        };
        const auto frameOf = [&](const QString &id) {
            return deckOf(id).value(QStringLiteral("source_frame")).toLongLong();
        };
        const auto peakOf = [&](const QString &id) {
            // A deck's meter is a peak and a hold. The peak lasts one block and a sample of it can land in
            // the gap between two beats; the hold is what the strip says a moment later, and it is the
            // honest answer to "did this deck make a sound in this window".
            const QVariantMap levels = deckOf(id).value(QStringLiteral("levels")).toMap();
            return std::max({levels.value(QStringLiteral("peak_left")).toDouble(),
                             levels.value(QStringLiteral("peak_right")).toDouble(),
                             levels.value(QStringLiteral("held_left")).toDouble(),
                             levels.value(QStringLiteral("held_right")).toDouble()});
        };
        const auto masterPeak = [&]() {
            const QVariantMap levels = bridge->snapshot().value(QStringLiteral("master")).toMap();
            return std::max(levels.value(QStringLiteral("peak_left")).toDouble(),
                            levels.value(QStringLiteral("peak_right")).toDouble());
        };
        const auto loudestOver = [&](const std::function<double()> &read, int budget_ms) {
            double loudest = 0.0;
            QElapsedTimer timer;
            timer.start();
            while (timer.elapsed() < budget_ms) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                loudest = std::max(loudest, read());
                QThread::msleep(10);
            }
            return loudest;
        };

        const QString path = parser.value(loadOption);
        if (path.isEmpty()) {
            std::fprintf(stderr, "dj-ui: the four-deck check needs --load\n");
            return 1;
        }
        const QStringList ids{QStringLiteral("A"), QStringLiteral("B"), QStringLiteral("C"),
                              QStringLiteral("D")};
        for (const QString &id : ids) {
            bridge->load(id, path);
        }
        const bool loaded = waitFor(
            [&]() {
                for (const QString &id : ids) {
                    if (!deckOf(id).value(QStringLiteral("path")).isValid()) {
                        return false;
                    }
                }
                return true;
            },
            30000);
        if (!loaded) {
            std::fprintf(stderr, "dj-ui: not all four decks took the file\n");
            return 1;
        }
        // Four decks reading the same bytes from the same place would prove nothing about four decks, and
        // an eight-second fixture runs out from under four windows of measurement: each deck gets a loop
        // of its own half of the track, a quarter of it further in than the one before. The material never
        // ends, and no two decks are reading the same place.
        const qint64 length =
            deckOf(QStringLiteral("A")).value(QStringLiteral("track")).toMap()
                .value(QStringLiteral("duration_frames")).toLongLong();
        if (length <= 0) {
            std::fprintf(stderr, "dj-ui: the four-deck check has no length to loop over\n");
            return 1;
        }
        for (int index = 0; index < ids.size(); ++index) {
            // Every window ends inside the file, never on its last frame: a loop that ends exactly on the
            // end of its file goes silent on the wrap (evidence in docs/qa/evidence/A07/four-decks.md), and
            // this check is about the mixer carrying four decks, not about that defect. It is recorded
            // there and waits for its own test.
            const qint64 start = length / 8 + length / 8 * index;
            const qint64 end = std::min(start + length / 5, length - length / 8);
            bridge->setLoop(ids[index], true, static_cast<qulonglong>(start),
                            static_cast<qulonglong>(end));
        }
        for (const QString &id : ids) {
            bridge->play(id, true);
        }
        if (!waitFor(
                [&]() {
                    for (const QString &id : ids) {
                        if (!deckOf(id).value(QStringLiteral("playing")).toBool()) {
                            return false;
                        }
                    }
                    return true;
                },
                5000)) {
            std::fprintf(stderr, "dj-ui: not all four decks started\n");
        }

        // 1. Every deck moves.
        QVector<qint64> before;
        for (const QString &id : ids) {
            before.append(frameOf(id));
        }
        QThread::msleep(1500);
        QCoreApplication::processEvents(QEventLoop::AllEvents, 100);
        bool all_moved = true;
        for (int index = 0; index < ids.size(); ++index) {
            const qint64 moved = frameOf(ids[index]) - before[index];
            std::fprintf(stdout, "dj-ui: deck %s moved %lld frames in 1,5 s\n", qPrintable(ids[index]),
                         static_cast<long long>(moved));
            if (moved < 24000) {  // half a second of material: a deck that plays is not one that crawls
                all_moved = false;
            }
        }
        if (!all_moved) {
            std::fprintf(stderr, "dj-ui: a deck of four is not moving\n");
            return 1;
        }

        // 2. Every deck makes its own sound, and all of it reaches the master.
        QVector<double> peaks;
        for (const QString &id : ids) {
            peaks.append(loudestOver([&]() { return peakOf(id); }, 2000));
            std::fprintf(stdout, "dj-ui: deck %s peaks at %.4f\n", qPrintable(id), peaks.last());
        }
        for (int index = 0; index < ids.size(); ++index) {
            if (peaks[index] <= 0.01) {
                std::fprintf(stderr, "dj-ui: deck %s says nothing (peak %.4f)\n", qPrintable(ids[index]),
                             peaks[index]);
                return 1;
            }
        }

        // 3. The decks are four voices of one mix, and the crossfader keeps each pair on its own side.
        // Measuring one deck alone turned out to be a measure of the instrument rather than of the mix —
        // a deck soloed through three muted channels read zero while the same two decks together read
        // half scale — so the check asks its question in pairs: shut two, and the other two must still
        // reach the master. Which side each deck is on is a unit test of its own
        // (`the_crossfader_holds_two_sides_of_four_decks`).
        // A command is a message, not a fact: the mix takes the fader on its own thread, and a measurement
        // started one block too early measures the setting that was there before it. Under a loaded
        // machine that turned the first pair of the run into a zero — the crossfader had not arrived yet.
        const auto settled = [&](const std::function<bool()> &reached) {
            QElapsedTimer timer;
            timer.start();
            while (timer.elapsed() < 2000) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                if (reached()) {
                    return true;
                }
                QThread::msleep(5);
            }
            return false;
        };
        // A peak lives one block: a window that saw ten snapshots can miss it, and a window that saw none
        // has measured nothing at all. The count is what tells the two apart.
        const auto loudestWithCount = [&](const std::function<double()> &read, int budget_ms, int *seen) {
            double loudest = 0.0;
            *seen = 0;
            QElapsedTimer timer;
            timer.start();
            while (timer.elapsed() < budget_ms) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                loudest = std::max(loudest, read());
                ++*seen;
                QThread::msleep(10);
            }
            return loudest;
        };
        const auto masterWithOnly = [&](const QString &first, const QString &second) {
            for (const QString &id : ids) {
                bridge->setChannelFader(id, id == first || id == second ? 1.0 : 0.0);
            }
            const auto open = [&]() {
                for (const QString &id : ids) {
                    const double want = id == first || id == second ? 1.0 : 0.0;
                    if (std::abs(deckOf(id).value(QStringLiteral("fader")).toDouble() - want) > 0.01) {
                        return false;
                    }
                }
                return true;
            };
            if (!settled(open)) {
                std::fprintf(stderr, "dj-ui: the mix never took the faders for %s and %s\n",
                             qPrintable(first), qPrintable(second));
                return -1.0;
            }
            double heard = 0.0;
            for (int attempt = 0; attempt < 3; ++attempt) {
                int seen = 0;
                heard = loudestWithCount([&]() { return masterPeak(); }, 1200, &seen);
                if (seen >= 20) {
                    break;
                }
                if (attempt == 2) {
                    std::fprintf(stderr,
                                 "dj-ui: the machine gave %d snapshots in 1,2 s: the pair %s and %s was "
                                 "not measured\n",
                                 seen, qPrintable(first), qPrintable(second));
                    return -1.0;
                }
            }
            for (const QString &id : ids) {
                bridge->setChannelFader(id, 1.0);
            }
            return heard;
        };
        bridge->setCrossfader(0.5);
        if (!settled([&]() {
                return std::abs(bridge->snapshot().value(QStringLiteral("crossfader")).toDouble() - 0.5)
                       < 0.01;
            })) {
            std::fprintf(stderr, "dj-ui: the mix never took the crossfader to the centre\n");
            return 1;
        }
        const double only_cd = masterWithOnly(QStringLiteral("C"), QStringLiteral("D"));
        const double only_ab = masterWithOnly(QStringLiteral("A"), QStringLiteral("B"));
        const double only_ac = masterWithOnly(QStringLiteral("A"), QStringLiteral("C"));
        std::fprintf(stdout,
                     "dj-ui: pairs through the master — A and B at %.4f, C and D at %.4f, A and C at %.4f\n",
                     only_ab, only_cd, only_ac);
        if (only_cd < 0.0 || only_ab < 0.0 || only_ac < 0.0) {
            // The faders never arrived: the measurement was not taken, and saying "no pair reaches the
            // master" would be saying something about the mixer that was never measured.
            return 1;
        }
        if (only_cd <= 0.01 || only_ab <= 0.01 || only_ac <= 0.01) {
            std::fprintf(stderr, "dj-ui: a pair of decks never reaches the master\n");
            return 1;
        }
        const double mixed = loudestOver([&]() { return masterPeak(); }, 1200);
        std::fprintf(stdout, "dj-ui: with all four back in, the master reads %.4f\n", mixed);
        if (mixed <= 0.01) {
            std::fprintf(stderr, "dj-ui: the four decks are silent together\n");
            return 1;
        }

        // 4. The interface carries four decks, not two.
        QStringList missing;
        for (const QString &id : ids) {
            if (findItem(window->contentItem(), QStringLiteral("deckPanel_") + id) == nullptr) {
                missing.append(id);
            }
        }
        if (findItem(window->contentItem(), QStringLiteral("mixer")) == nullptr) {
            missing.append(QStringLiteral("mixer"));
        }
        if (!missing.isEmpty()) {
            std::fprintf(stderr, "dj-ui: the scene has no panel for %s\n", qPrintable(missing.join(',')));
            return 1;
        }

        for (const QString &id : ids) {
            bridge->play(id, false);
        }
        std::fprintf(stdout,
                     "dj-ui: four decks play and mix — each one moves, each one is heard on its own, every "
                     "pair of them reaches the master, and the scene has a panel for every one of them\n");
        return 0;
    }

    if (parser.isSet(syncSoakOption)) {
        return runSyncSoak(engine, bridge, parser, connectOption, soakSecondsOption);
    }

    if (parser.isSet(syncCheckOption)) {
        return runSyncCheck(engine, bridge, parser, connectOption, loadOption, loadSecondOption);
    }

    if (parser.isSet(pitchOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        QElapsedTimer linking;
        linking.start();
        while (!bridge->isConnected() && linking.elapsed() < 3000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(10);
        }
        if (!bridge->isConnected()) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        const auto pump = [&](int rounds) {
            for (int step = 0; step < rounds; ++step) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 10);
                QThread::msleep(5);
            }
        };
        const auto tempoOf = [&](const char *deck) {
            for (const QVariant &entry : bridge->snapshot().value(QStringLiteral("decks")).toList()) {
                const QVariantMap map = entry.toMap();
                if (map.value(QStringLiteral("deck")).toString() == QString::fromLatin1(deck)) {
                    return map.value(QStringLiteral("tempo_ratio")).toDouble();
                }
            }
            return -1.0;
        };
        auto *pitch = findItem(window->contentItem(), QStringLiteral("pitch_A"));
        if (pitch == nullptr) {
            std::fprintf(stderr, "dj-ui: deck A has no pitch fader\n");
            return 1;
        }
        // The material measurement below needs something on the deck: load what --load names and wait
        // for the attach, the same way the other checks do.
        {
            const QString track = parser.value(loadOption);
            if (!track.isEmpty()) {
                bridge->load(QStringLiteral("A"), track);
                pump(120);
                QElapsedTimer ready;
                ready.start();
                while (ready.elapsed() < 15000) {
                    bool attached = false;
                    for (const QVariant &entry :
                         bridge->snapshot().value(QStringLiteral("decks")).toList()) {
                        const QVariantMap map = entry.toMap();
                        if (map.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                            attached = map.value(QStringLiteral("remaining_frames")).toDouble() > 0.0;
                        }
                    }
                    if (attached) {
                        break;
                    }
                    QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                    QThread::msleep(20);
                }
            }
        }
        pump(20);
        const double perPixel = pitch->property("ratioPerPixel").toDouble();
        if (!(perPixel > 0.0)) {
            std::fprintf(stderr, "dj-ui: the pitch fader has no throw (%g per pixel)\n", perPixel);
            return 1;
        }

        // The point is recomputed for every gesture: loading a track changes the deck's layout (the
        // level correction appears beside the clock), and a point captured before that lands somewhere
        // else on the fader.
        const auto grabPoint = [&] {
            return pitch->mapToScene(
                QPointF(pitch->width() / 2.0, pitch->height() / 2.0));
        };
        const QPointF grab = grabPoint();
        const auto send = [&](QEvent::Type type, const QPointF &at) {
            // A release carries no button: with `LeftButton` in the buttons mask the mouse area never
            // sees the drag end, and the next press is measured from a stale position.
            const Qt::MouseButtons buttons =
                type == QEvent::MouseButtonRelease ? Qt::NoButton : Qt::LeftButton;
            QMouseEvent event(type, at, window->mapToGlobal(at.toPoint()), Qt::LeftButton, buttons,
                              Qt::NoModifier);
            QCoreApplication::sendEvent(window, &event);
            QCoreApplication::processEvents();
        };
        // The throw is a setting: ±16 % by default, and the whole travel of the fader is that range.
        // Checked both ways — the fader's own numbers, and a real drag whose expected rate is computed
        // from them — because a fader can show a range it cannot reach.
        {
            const double rangePercent = pitch->property("rangePercent").toDouble();
            const double minimum = pitch->property("minimum").toDouble();
            const double maximum = pitch->property("maximum").toDouble();
            const double perPixelNow = pitch->property("ratioPerPixel").toDouble();
            const double inset = pitch->property("inset").toDouble();
            std::fprintf(stdout, "dj-ui: the pitch throw is ±%.1f %% (%.3f … %.3f)\n",
                         rangePercent, minimum, maximum);
            if (std::abs(rangePercent - 16.0) > 1e-6 || std::abs(minimum - 0.84) > 1e-6
                || std::abs(maximum - 1.16) > 1e-6) {
                std::fprintf(stderr, "dj-ui: the throw is %.1f %% (%.3f … %.3f), not ±16 %%\n",
                             rangePercent, minimum, maximum);
                return 1;
            }
            // The travel is the range: when the fader has been laid out, its pixels must cover the
            // whole throw. A collapsed layout (height 0) says nothing either way, so it is not asked.
            if (pitch->height() > 100.0) {
                const double travel = (pitch->height() - 2.0 * inset) * perPixelNow;
                if (std::abs(travel - (maximum - minimum)) > 0.002) {
                    std::fprintf(stderr,
                                 "dj-ui: the fader's travel covers %.4f, but the range is %.4f\n",
                                 travel, maximum - minimum);
                    return 1;
                }
            }
        }

        // Let the release's own events drain before resetting: a move still queued would otherwise
        // re-send the old rate on top of the reset.
        pump(40);
        // Back to the file's own speed, so the next gesture starts from a known place. The fader reads
        // its position from the engine's snapshot, so the reset has to make the round trip.
        QMetaObject::invokeMethod(pitch, "ask", Q_ARG(QVariant, QVariant(1.0)));
        {
            QElapsedTimer settle;
            settle.start();
            while (settle.elapsed() < 2000 && std::abs(tempoOf("A") - 1.0) > 1e-6) {
                pump(2);
            }
            std::fprintf(stdout, "dj-ui: after the reset the fader reads %.4f (engine %.4f)\n",
                         pitch->property("ratio").toDouble(), tempoOf("A"));
        }

        // Ten pixels up from unity, on the same fader: the middle of the throw still moves by the hand.
        const QPointF centre = grabPoint();
        send(QEvent::MouseButtonPress, centre);
        send(QEvent::MouseMove, centre - QPointF(0, 10));
        pump(30);
        const double asked = pitch->property("lastRatio").toDouble();
        const double playing = tempoOf("A");
        const double other = tempoOf("B");
        const double expected = 1.0 + 10.0 * perPixel;
        std::fprintf(stdout,
                     "dj-ui: ten pixels up the pitch fader asks for %.4f and the engine plays at "
                     "%.4f (deck B at %.3f)\n", asked, playing, other);
        if (std::abs(asked - expected) > 0.001 || std::abs(playing - expected) > 0.001) {
            std::fprintf(stderr, "dj-ui: the pitch fader and the engine disagree about the rate\n");
            return 1;
        }
        if (playing <= 1.0 || std::abs(other - 1.0) > 1e-9) {
            std::fprintf(stderr, "dj-ui: the pitch moved the wrong deck or the wrong way\n");
            return 1;
        }

        // The number in the snapshot is a promise; the material has to move at that rate. Play the deck
        // and compare how far the source went with how much time passed — the check that would have
        // caught a fader whose value never reached the sound.
        {
            const auto frameOf = [&] {
                for (const QVariant &entry : bridge->snapshot().value(QStringLiteral("decks")).toList()) {
                    const QVariantMap map = entry.toMap();
                    if (map.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                        return map.value(QStringLiteral("source_frame")).toDouble();
                    }
                }
                return -1.0;
            };
            bridge->play(QStringLiteral("A"), true);
            // A deck that is still loading accepts Play and does nothing; wait for material to move
            // rather than measuring the load.
            // From a known place, far enough from the end: a deck that runs out of clip half way through
            // the window stops dead, and the average then reads exactly like a deck running slow. Left
            // over from an earlier check, a short clip can be a second from its end.
            double remaining = 0.0;
            for (const QVariant &entry : bridge->snapshot().value(QStringLiteral("decks")).toList()) {
                const QVariantMap map = entry.toMap();
                if (map.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    remaining = map.value(QStringLiteral("remaining_frames")).toDouble();
                }
            }
            if (remaining < 48000.0 * 4) {
                bridge->seek(QStringLiteral("A"), 0);
                pump(40);
            }
            const double before = frameOf();
            QElapsedTimer start;
            start.start();
            while (start.elapsed() < 4000 && !(frameOf() > before + 100.0)) {
                pump(2);
            }
            if (!(frameOf() > before + 100.0)) {
                QString state;
                for (const QVariant &entry : bridge->snapshot().value(QStringLiteral("decks")).toList()) {
                    const QVariantMap map = entry.toMap();
                    if (map.value(QStringLiteral("deck")).toString() != QStringLiteral("A")) {
                        continue;
                    }
                    state = QStringLiteral("load %1, playing %2, frame %3, remaining %4")
                                .arg(QJsonDocument::fromVariant(map.value(QStringLiteral("load")))
                                         .toJson(QJsonDocument::Compact),
                                     map.value(QStringLiteral("playing")).toBool() ? "yes" : "no")
                                .arg(map.value(QStringLiteral("source_frame")).toDouble(), 0, 'f', 0)
                                .arg(map.value(QStringLiteral("remaining_frames")).toDouble(), 0, 'f', 0);
                }
                std::fprintf(stderr, "dj-ui: the deck did not start playing at all (%s)\n",
                             qPrintable(state));
                return 1;
            }
            QElapsedTimer window;
            window.start();
            const double from = frameOf();
            // Long enough that the snapshot's 20 Hz publishing is a small part of the window. Sampled in
            // slices: a deck that stalls for part of the window and a deck that runs slow show the same
            // average, and only the series tells them apart.
            QVector<double> series;
            for (int slice = 0; slice < 12; ++slice) {
                pump(20);
                series.append(frameOf());
            }
            const double elapsed = window.elapsed() / 1000.0;
            const double to = frameOf();
            bridge->play(QStringLiteral("A"), false);
            // The clip that is on the deck, not the one this check assumed: a file at another rate moves a
            // different number of source frames for the same second of sound.
            qint64 meterRate = 0;
            for (const QVariant &entry : bridge->snapshot().value(QStringLiteral("decks")).toList()) {
                const QVariantMap map = entry.toMap();
                if (map.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    meterRate = map.value(QStringLiteral("source_rate")).toLongLong();
                }
            }
            // Against the rate the deck actually holds: the window is wall clock, the material is frames of
            // that file, and a 44,1 kHz clip would read a tenth fast at a hard-coded 48 000.
            const double meter = meterRate > 0 ? static_cast<double>(meterRate) : 48000.0;
            const double heard = (to - from) / (elapsed * meter);
            std::fprintf(stdout,
                         "dj-ui: the deck consumed %.3f seconds of the file per second at %.4f "
                         "(elapsed %.3f s, source rate %lld)\n",
                         heard,
                         playing,
                         elapsed,
                         static_cast<long long>(meterRate));
            if (elapsed < 0.5 || std::abs(heard - playing) > 0.06) {
                QString walked;
                for (const double sample : series) {
                    walked += QStringLiteral("%1 ").arg(sample - from, 0, 'f', 0);
                }
                std::fprintf(stderr, "dj-ui: the window walked %s\n", qPrintable(walked));
                std::fprintf(stderr,
                             "dj-ui: the fader says %.3f but the material moved at %.3f "
                             "(elapsed %.3f s, source rate %lld, %lld frames)\n",
                             playing,
                             heard,
                             elapsed,
                             static_cast<long long>(meterRate),
                             static_cast<long long>(to - from));
                return 1;
            }
        }

        // The same gesture back down, and a double click to the file's own speed.
        send(QEvent::MouseMove, grab);
        pump(20);
        if (std::abs(tempoOf("A") - 1.0) > 0.002) {
            std::fprintf(stderr, "dj-ui: the fader did not come back to the file's speed\n");
            return 1;
        }
        send(QEvent::MouseMove, grab - QPointF(0, 10));
        pump(20);
        send(QEvent::MouseButtonDblClick, grab);
        send(QEvent::MouseButtonRelease, grab);
        pump(30);
        const double reset = tempoOf("A");
        std::fprintf(stdout, "dj-ui: a double click put the pitch back at %.3f\n", reset);
        if (std::abs(reset - 1.0) > 1e-6) {
            std::fprintf(stderr, "dj-ui: a double click did not return the deck to its own speed\n");
            return 1;
        }
        // A rate the fader cannot reach. The sync matches a tempo beyond the fader's travel
        // (`PitchLimits::beyond_the_fader`): the deck then runs at a rate the hand could never ask for,
        // and the handle must still be a handle — pinned at the end of its travel, not drawn past the
        // strip it belongs to.
        {
            QQuickItem *handle = findItem(window->contentItem(), QStringLiteral("pitchHandle_A"));
            if (handle == nullptr) {
                std::fprintf(stderr, "dj-ui: the pitch fader has no handle\n");
                return 1;
            }
            const double top_of_throw = pitch->property("maximum").toDouble();
            const double bottom_of_throw = pitch->property("minimum").toDouble();
            const double beyond = top_of_throw + (top_of_throw - bottom_of_throw) * 0.4;
            bridge->setTempo(QStringLiteral("A"), beyond);
            QElapsedTimer waiting;
            waiting.start();
            while (waiting.elapsed() < 2000 && std::abs(tempoOf("A") - beyond) > 0.005) {
                pump(1);
            }
            if (std::abs(tempoOf("A") - beyond) > 0.005) {
                std::fprintf(stderr,
                             "dj-ui: the deck did not take a rate beyond the fader (%.4f wanted, "
                             "%.4f playing)\n",
                             beyond, tempoOf("A"));
                return 1;
            }
            const double top = handle->y();
            const double bottom = top + handle->height();
            std::fprintf(stdout,
                         "dj-ui: at %.4f, past the fader's %.3f, the handle sits at %.1f…%.1f px of "
                         "%.1f\n",
                         beyond, top_of_throw, top, bottom, pitch->height());
            if (top < 0.0 || bottom > pitch->height()) {
                std::fprintf(stderr,
                             "dj-ui: the handle leaves its fader at %.4f: %.1f…%.1f of %.1f px\n",
                             beyond, top, bottom, pitch->height());
                return 1;
            }
            bridge->setTempo(QStringLiteral("A"), 1.0);
            pump(20);
        }
        return 0;
    }

    if (parser.isSet(neutralOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        QElapsedTimer linking;
        linking.start();
        while (!bridge->isConnected() && linking.elapsed() < 3000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(10);
        }
        if (!bridge->isConnected()) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }

        const auto pump = [&](int rounds) {
            for (int step = 0; step < rounds; ++step) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 10);
                QThread::msleep(5);
            }
        };
        // A real double click arrives as press, release, double click, release: Qt synthesises the
        // double click in the platform layer, so a check that only presses twice never sees one.
        const auto doubleClick = [&](QQuickItem *target, const char *name) {
            if (target == nullptr) {
                std::fprintf(stderr, "dj-ui: nothing to double click for %s\n", name);
                return false;
            }
            const QPointF at =
                target->mapToScene(QPointF(target->width() / 2.0, target->height() / 2.0));
            const auto send = [&](QEvent::Type type) {
                QMouseEvent event(type, at, window->mapToGlobal(at.toPoint()), Qt::LeftButton,
                                  Qt::LeftButton, Qt::NoModifier);
                QCoreApplication::sendEvent(window, &event);
                QCoreApplication::processEvents();
            };
            send(QEvent::MouseButtonPress);
            send(QEvent::MouseButtonRelease);
            send(QEvent::MouseButtonDblClick);
            send(QEvent::MouseButtonRelease);
            pump(30);
            return true;
        };
        const auto fieldOf = [&](const char *deck, const char *field) {
            for (const QVariant &entry : bridge->snapshot().value(QStringLiteral("decks")).toList()) {
                const QVariantMap map = entry.toMap();
                if (map.value(QStringLiteral("deck")).toString() != QString::fromLatin1(deck)) {
                    continue;
                }
                return map.value(QString::fromLatin1(field)).toDouble();
            }
            return -1.0;
        };
        const auto bandOf = [&](const char *deck, const char *band) {
            for (const QVariant &entry : bridge->snapshot().value(QStringLiteral("decks")).toList()) {
                const QVariantMap map = entry.toMap();
                if (map.value(QStringLiteral("deck")).toString() != QString::fromLatin1(deck)) {
                    continue;
                }
                const QVariantMap eq = map.value(QStringLiteral("eq")).toMap();
                return eq.value(QString::fromLatin1(band) + QStringLiteral("_db")).toDouble();
            }
            return 999.0;
        };

        // Every control away from where it should land: a double click that is not wired would then leave
        // the old number in place and the check would say which one.
        bridge->setChannelGain(QStringLiteral("A"), 0.5);
        bridge->setChannelGain(QStringLiteral("B"), 2.0);
        bridge->setChannelEq(QStringLiteral("A"), QStringLiteral("low"), 9.0);
        bridge->setChannelEq(QStringLiteral("B"), QStringLiteral("high"), -12.0);
        bridge->setMasterGain(0.25);
        bridge->setChannelFader(QStringLiteral("A"), 0.4);
        bridge->setChannelFader(QStringLiteral("B"), 0.7);
        bridge->setCrossfader(0.15);
        pump(60);

        if (!doubleClick(findItem(window->contentItem(), QStringLiteral("trim_A")), "the gain of A")
            || !doubleClick(findItem(window->contentItem(), QStringLiteral("trim_B")), "the gain of B")
            || !doubleClick(findItem(window->contentItem(), QStringLiteral("eq_low_A")), "the low band")
            || !doubleClick(findItem(window->contentItem(), QStringLiteral("eq_high_B")), "the high band")
            || !doubleClick(findItem(window->contentItem(), QStringLiteral("masterKnob")), "the master")
            || !doubleClick(findItem(window->contentItem(), QStringLiteral("fader_A")), "the fader of A")
            || !doubleClick(findItem(window->contentItem(), QStringLiteral("fader_B")), "the fader of B")
            || !doubleClick(findItem(window->contentItem(), QStringLiteral("crossfaderHandle")),
                            "the crossfader")) {
            return 1;
        }

        const double gainA = fieldOf("A", "linear_gain");
        const double gainB = fieldOf("B", "linear_gain");
        const double lowA = bandOf("A", "low");
        const double highB = bandOf("B", "high");
        const double master = bridge->snapshot().value(QStringLiteral("master_gain")).toDouble();
        const double faderA = fieldOf("A", "fader");
        const double faderB = fieldOf("B", "fader");
        const double crossfader = bridge->snapshot().value(QStringLiteral("crossfader")).toDouble();
        // Three quarters of the level: 0.75 of full gain, which is -2.5 dB.
        const double threeQuarters = 0.75;
        std::fprintf(stdout,
                     "dj-ui: double clicks put the gains at %.3f / %.3f, the bands at %+.1f / %+.1f dB, "
                     "the master at %.3f (%.1f dB), the faders at %.3f / %.3f and the crossfader at "
                     "%.2f\n",
                     gainA, gainB, lowA, highB, master, 20.0 * std::log10(master), faderA, faderB,
                     crossfader);
        if (std::abs(gainA - 1.0) > 0.01 || std::abs(gainB - 1.0) > 0.01) {
            std::fprintf(stderr, "dj-ui: a double click did not trim the gain back to unity\n");
            return 1;
        }
        if (std::abs(lowA) > 0.01 || std::abs(highB) > 0.01) {
            std::fprintf(stderr, "dj-ui: a double click did not flatten the band\n");
            return 1;
        }
        if (std::abs(master - threeQuarters) > 0.01) {
            std::fprintf(stderr, "dj-ui: a double click did not stand the master at three quarters of "
                                 "its level\n");
            return 1;
        }
        if (std::abs(faderA - 1.0) > 0.01 || std::abs(faderB - 1.0) > 0.01) {
            std::fprintf(stderr, "dj-ui: a double click did not open a fader\n");
            return 1;
        }
        if (std::abs(crossfader - 0.5) > 0.01) {
            std::fprintf(stderr, "dj-ui: a double click did not centre the crossfader\n");
            return 1;
        }
        return 0;
    }

    if (parser.isSet(spaceCheckOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        const QString track = parser.value(loadOption);
        bridge->connectToEngine(parser.value(connectOption));
        QElapsedTimer linking;
        linking.start();
        while (!bridge->isConnected() && linking.elapsed() < 3000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(10);
        }
        if (!track.isEmpty()) {
            // A check that needs a load unloads the deck first: the check before this one can leave a
            // track playing in deck A, and a load onto a playing deck is refused as `Busy` — the colour
            // under test would then belong to somebody else's track, which ends mid-blink.
            const auto emptied = [&]() {
                QElapsedTimer attempt;
                attempt.start();
                while (attempt.elapsed() < 5000) {
                    QString title;
                    bool busy = false;
                    for (const QVariant &entry :
                         bridge->snapshot().value(QStringLiteral("decks")).toList()) {
                        const QVariantMap deck = entry.toMap();
                        if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                            title = deck.value(QStringLiteral("title")).toString();
                            busy = deck.value(QStringLiteral("playing")).toBool();
                        }
                    }
                    if (title.isEmpty() && !busy) {
                        return true;
                    }
                    bridge->play(QStringLiteral("A"), false);
                    bridge->eject(QStringLiteral("A"));
                    QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                    QThread::msleep(20);
                }
                return false;
            };
            if (!emptied()) {
                std::fprintf(stderr, "dj-ui: deck A never emptied before the space check loaded\n");
                return 1;
            }
            bridge->load(QStringLiteral("A"), track);
        }

        const auto deckPlaying = [&]() {
            const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant &entry : decks) {
                const QVariantMap deck = entry.toMap();
                if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    return deck.value(QStringLiteral("playing")).toBool();
                }
            }
            return false;
        };
        QElapsedTimer waiting;
        waiting.start();
        bool playAsked = false;
        while (waiting.elapsed() < 10000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            if (!deckPlaying() && !playAsked && waiting.elapsed() > 800) {
                bridge->play(QStringLiteral("A"), true);
                playAsked = true;
            }
            if (deckPlaying()) {
                break;
            }
            QThread::msleep(20);
        }
        if (!deckPlaying()) {
            std::fprintf(stderr, "dj-ui: deck A never started playing\n");
            return 1;
        }

        // A Shortcut is not an item, so it is looked up as an object: the visual tree has no place
        // for something that draws nothing.
        auto *toggle = window->findChild<QObject *>(QStringLiteral("browserToggle"));
        auto *browserPanel = findItem(window->contentItem(), QStringLiteral("browserPanel"));
        auto *shell = findItem(window->contentItem(), QStringLiteral("globalBar"));
        if (toggle == nullptr || browserPanel == nullptr || shell == nullptr) {
            std::fprintf(stderr, "dj-ui: the shell has no browser toggle\n");
            return 1;
        }
        shell = shell->parentItem();
        std::fprintf(stdout, "dj-ui: the browser key is %s\n",
                     qPrintable(toggle->property("sequence").toString()));
        if (toggle->property("sequence").toString() != QStringLiteral("Space")) {
            std::fprintf(stderr, "dj-ui: the browser is not on Space\n");
            return 1;
        }

        // The gesture itself, delivered as the window delivers keys: Space must fold the browser and
        // leave the deck playing exactly as it was.
        const auto pressSpace = [&]() {
            for (const QEvent::Type type : {QEvent::KeyPress, QEvent::KeyRelease}) {
                QKeyEvent event(type, Qt::Key_Space, Qt::NoModifier, QStringLiteral(" "));
                QCoreApplication::sendEvent(window, &event);
            }
            QCoreApplication::processEvents();
            QThread::msleep(100);
            QCoreApplication::processEvents();
        };

        const bool before = deckPlaying();
        pressSpace();
        const bool folded = !browserPanel->isVisible();
        pressSpace();
        const bool unfolded = browserPanel->isVisible();
        const bool after = deckPlaying();
        std::fprintf(stdout, "dj-ui: deck A playing before %s, after %s; folded %s, unfolded %s\n",
                     before ? "yes" : "no", after ? "yes" : "no", folded ? "yes" : "no",
                     unfolded ? "yes" : "no");
        if (before != after) {
            std::fprintf(stderr, "dj-ui: Space still reaches playback\n");
            return 1;
        }
        if (!folded || !unfolded) {
            std::fprintf(stderr, "dj-ui: Space does not fold the browser\n");
            return 1;
        }
        bridge->play(QStringLiteral("A"), false);
        std::fprintf(stdout, "dj-ui: Space leaves playback alone and folds the browser\n");
        return 0;
    }

    if (parser.isSet(refusalCheckOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        const QString track = parser.value(loadOption);
        if (track.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --refusal-check needs --load\n");
            return 2;
        }
        // The shell connects at the end of main; this mode returns before that.
        bridge->connectToEngine(parser.value(connectOption));
        QElapsedTimer linking;
        linking.start();
        while (!bridge->isConnected() && linking.elapsed() < 3000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(10);
        }
        bridge->load(QStringLiteral("A"), track);

        bool playing = false;
        QElapsedTimer waiting;
        waiting.start();
        while (!playing && waiting.elapsed() < 10000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant &entry : decks) {
                const QVariantMap deck = entry.toMap();
                if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    playing = deck.value(QStringLiteral("playing")).toBool();
                }
            }
            if (!playing && waiting.elapsed() > 1000) {
                bridge->play(QStringLiteral("A"), true);
            }
            QThread::msleep(20);
        }
        if (!playing) {
            std::fprintf(stderr, "dj-ui: deck A never started playing\n");
            return 1;
        }

        // The browser, pointed at the track's own folder with the track selected.
        browser.open(QFileInfo(track).absolutePath());
        QCoreApplication::processEvents();
        auto *panel = findItem(window->contentItem(), QStringLiteral("browserPanel"));
        auto *list = findItem(window->contentItem(), QStringLiteral("browserList"));
        if (panel == nullptr || list == nullptr) {
            std::fprintf(stderr, "dj-ui: the shell has no browser\n");
            return 1;
        }
        int fileIndex = -1;
        const QVariantList entries = browser.entries();
        for (int index = 0; index < entries.size(); ++index) {
            if (entries.at(index).toMap().value(QStringLiteral("path")).toString() == track) {
                fileIndex = index;
            }
        }
        if (fileIndex < 0) {
            std::fprintf(stderr, "dj-ui: the browser does not list the track\n");
            return 1;
        }
        list->setProperty("currentIndex", fileIndex);
        QVariant refused;
        QMetaObject::invokeMethod(panel, "loadInto", Q_RETURN_ARG(QVariant, refused),
                                  Q_ARG(QVariant, QStringLiteral("A")));
        QCoreApplication::processEvents();

        auto *deckButton = findItem(window->contentItem(), QStringLiteral("deckButton_A"));
        if (deckButton == nullptr) {
            std::fprintf(stderr, "dj-ui: the shell has no deck buttons\n");
            return 1;
        }
        // The fixture is short and a null device spends it faster than a hand can blink: rewind it and
        // put a loop on the deck, so the track cannot end while the colour is under test. A deck that
        // stopped is a deck whose button is grey for a reason that has nothing to do with the refusal.
        // Pausing before the seek is what makes the seek land.
        bridge->play(QStringLiteral("A"), false);
        bridge->seek(QStringLiteral("A"), 0);
        bridge->setLoop(QStringLiteral("A"), true, 0, 96000);
        bridge->play(QStringLiteral("A"), true);
        QCoreApplication::processEvents();
        QThread::msleep(200);
        QCoreApplication::processEvents();

        // A playing deck's button is blue before anything is pressed: the browser tells the truth
        // about the decks it feeds.
        const QColor accent(settings.themeAccent());
        const auto faceOf = [](QQuickItem *button) {
            return button->property("faceColor").value<QColor>();
        };
        std::fprintf(stdout, "dj-ui: deck A is playing, its browser button is %s\n",
                     qPrintable(faceOf(deckButton).name()));
        if (faceOf(deckButton) != accent) {
            std::fprintf(stderr, "dj-ui: a playing deck's button is not marked as playing\n");
            return 1;
        }

        // Pressing the button is the way a person finds out; it has to blink.
        QMetaObject::invokeMethod(deckButton, "clicked");
        QCoreApplication::processEvents();
        const QColor whileFlashing = faceOf(deckButton);
        const bool flashingNow = deckButton->property("flashing").toBool();
        if (whileFlashing != QColor(settings.themeDanger())) {
            std::fprintf(stderr, "dj-ui: the refusal did not turn the button red\n");
            return 1;
        }
        // ... and it has to come back to blue, not to grey, once the blink is over. The end of the
        // blink is the button's own flag: a guessed duration is a duration that races the track.
        QElapsedTimer blink;
        blink.start();
        while (blink.elapsed() < 2000 && deckButton->property("flashing").toBool()) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 30);
            QThread::msleep(20);
        }
        QCoreApplication::processEvents();
        const QColor afterBlink = faceOf(deckButton);
        std::fprintf(stdout, "dj-ui: after the blink the button is %s\n",
                     qPrintable(afterBlink.name()));
        if (afterBlink != accent) {
            std::fprintf(stderr, "dj-ui: the button did not return to its playing colour\n");
            return 1;
        }
        const int buttonRefusals = deckButton->property("refusals").toInt();
        const bool flashing = flashingNow;
        std::fprintf(stdout, "dj-ui: refused load: deck button %d, flashing %s\n", buttonRefusals,
                     flashing ? "yes" : "no");
        if (!refused.toBool() && buttonRefusals > 0 && flashing) {
            bridge->play(QStringLiteral("A"), false);
            std::fprintf(stdout, "dj-ui: a load into a playing deck is refused and the button blinks\n");
            return 0;
        }
        std::fprintf(stderr, "dj-ui: the refusal was not signalled\n");
        return 1;
    }

    if (parser.isSet(stripCheckOption)) {
        // The picture is the thing the user sees, so it gets its own measurement: the envelope the
        // engine publishes has to reach the strip that draws it.
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        QEventLoop settle;
        QTimer::singleShot(600, &settle, &QEventLoop::quit);
        settle.exec();

        // The shell connects at the end of main; this mode returns before that, so the link is made
        // here or the check would measure a UI that never heard from the engine.
        if (parser.isSet(connectOption)) {
            bridge->connectToEngine(parser.value(connectOption));
            QElapsedTimer linking;
            linking.start();
            while (!bridge->isConnected() && linking.elapsed() < 3000) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                QThread::msleep(10);
            }
            std::fprintf(stdout, "dj-ui: strip check link %s\n",
                         bridge->isConnected() ? "up" : "down");
        }

        auto *detail = findItem(window->contentItem(), QStringLiteral("deckDetail_A"));
        if (detail == nullptr) {
            std::fprintf(stderr, "dj-ui: deck A has no strip\n");
            return 1;
        }
        auto *envelope = qobject_cast<QObject *>(detail->property("envelope").value<QObject *>());
        if (envelope == nullptr) {
            std::fprintf(stderr, "dj-ui: the strip has no envelope\n");
            return 1;
        }
        const QString track = parser.value(loadOption);
        if (!track.isEmpty() && bridge->isConnected()) {
            bridge->load(QStringLiteral("A"), track);
            std::fprintf(stdout, "dj-ui: asked deck A for %s\n", qPrintable(track));
        }

        QElapsedTimer waited;
        waited.start();
        int buckets = 0;
        while (waited.elapsed() < 15000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 50);
            buckets = envelope->property("buckets").toInt();
            if (buckets > 0) {
                break;
            }
            QThread::msleep(20);
        }
        const QString error = envelope->property("error").toString();
        std::fprintf(stdout, "dj-ui: strip A holds %d columns after %lld ms%s%s\n", buckets,
                     static_cast<long long>(waited.elapsed()),
                     error.isEmpty() ? "" : ", error: ", qPrintable(error));
        if (buckets <= 0) {
            std::fprintf(stderr, "dj-ui: the strip never received an envelope\n");
            return 1;
        }
        return 0;
    }

    if (parser.isSet(keyCheckOption)) {
        // The key is a measurement, so the check reads the two places a person reads it: the label in
        // the deck's header, and the file the engine wrote beside the track. A key that only exists in
        // memory is a key the next load forgets.
        const QString track = parser.value(loadOption);
        if (track.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --key-check needs --load\n");
            return 2;
        }
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        if (parser.isSet(connectOption)) {
            bridge->connectToEngine(parser.value(connectOption));
            QElapsedTimer linking;
            linking.start();
            while (!bridge->isConnected() && linking.elapsed() < 3000) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                QThread::msleep(10);
            }
        }
        if (!bridge->isConnected()) {
            std::fprintf(stderr, "dj-ui: --key-check needs a running engine\n");
            return 1;
        }
        // The engine outlives one check: a deck that already holds a track would show the key of the
        // track before this one, and the check would read a stale answer as a fresh one. The wait is on
        // the deck's own state, not on the label: a label that has not been repainted yet is not a
        // load that has not happened.
        // What the engine said about the commands this check sends: a refusal is the reason a load
        // leaves the deck as it was, and a check that only reports "no key" hides it.
        QString lastAck;
        QObject::connect(bridge.get(), &BridgeClient::acked,
                         [&lastAck](int id, bool applied, bool rejected, QString reason) {
                             lastAck = QStringLiteral("command %1: %2%3")
                                           .arg(id)
                                           .arg(applied ? QStringLiteral("applied")
                                                        : (rejected ? QStringLiteral("rejected")
                                                                    : QStringLiteral("unknown")))
                                           .arg(reason.isEmpty() ? QString()
                                                                 : QStringLiteral(" (%1)").arg(reason));
                         });
        const auto deckNow = [&]() { return deckById(bridge->snapshot(), QStringLiteral("A")); };
        const auto trackOf = [&]() { return deckNow().value(QStringLiteral("track")).toMap(); };
        const auto keyOf = [&]() { return trackOf().value(QStringLiteral("key")).toString().trimmed(); };
        // The deck is emptied first, and not only stopped. A deck that is playing refuses a load — the
        // refusal is right, cutting the track the room is dancing to is the mistake a DJ never forgives
        // — and an earlier check in the suite leaves deck A playing. Emptying also makes the wait below
        // mean something: on a deck that already holds this very file the title matches before the load
        // has happened, and the check would read the previous answer as a fresh one.
        bridge->eject(QStringLiteral("A"));
        QElapsedTimer emptying;
        emptying.start();
        while (emptying.elapsed() < 5000 && !trackOf().isEmpty()) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(10);
        }
        bridge->load(QStringLiteral("A"), track);
        // The title says which track the deck is describing: the key of the one before it is not an
        // answer about this one.
        const QString wanted = QFileInfo(track).completeBaseName();
        QElapsedTimer waited;
        waited.start();
        QString shown;
        while (waited.elapsed() < 30000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 50);
            const QVariantMap current = trackOf();
            const QString title = current.value(QStringLiteral("title")).toString();
            if (title == wanted && !keyOf().isEmpty()) {
                shown = keyOf();
                break;
            }
            QThread::msleep(20);
        }
        std::fprintf(stdout, "dj-ui: deck A shows the key \"%s\" after %lld ms\n", qPrintable(shown),
                     static_cast<long long>(waited.elapsed()));
        if (shown.isEmpty()) {
            const QVariantMap current = trackOf();
            std::fprintf(stderr,
                         "dj-ui: the deck never showed a key (wanted \"%s\", deck says load %s, title "
                         "\"%s\", key \"%s\")\n",
                         qPrintable(wanted),
                         qPrintable(deckNow().value(QStringLiteral("load")).toString()),
                         qPrintable(current.value(QStringLiteral("title")).toString()),
                         qPrintable(current.value(QStringLiteral("key")).toString()));
            std::fprintf(stderr, "dj-ui: the engine last said %s\n", qPrintable(lastAck));
            return 1;
        }
        // The file beside the track: `.djmeta/<name>.json`, the same place the cues go.
        const QFileInfo info(track);
        const QString sidecar = info.absolutePath() + QStringLiteral("/.djmeta/") + info.fileName()
                                + QStringLiteral(".json");
        QFile stored(sidecar);
        if (!stored.open(QIODevice::ReadOnly)) {
            std::fprintf(stderr, "dj-ui: no metadata at %s\n", qPrintable(sidecar));
            return 1;
        }
        const QJsonObject meta = QJsonDocument::fromJson(stored.readAll()).object();
        const QString written = meta.value(QStringLiteral("key")).toString();
        std::fprintf(stdout, "dj-ui: %s records the key \"%s\"\n", qPrintable(sidecar),
                     qPrintable(written));
        if (written.isEmpty()) {
            std::fprintf(stderr, "dj-ui: the engine did not write the key beside the track\n");
            return 1;
        }
        // The format is part of the answer: the owner reads keys as numbers on the circle of fifths, so
        // a name in the header would be a different feature. 1d … 12d, 1m … 12m, nothing else.
        static const QRegularExpression circleOfFifths(QStringLiteral("^([1-9]|1[0-2])[dm]$"));
        if (!circleOfFifths.match(shown).hasMatch()) {
            std::fprintf(stderr, "dj-ui: \"%s\" is not a key on the circle of fifths\n",
                         qPrintable(shown));
            return 1;
        }
        // A file that already holds a code must agree with the header. A file holding a name was
        // translated for the display, and the translation itself is covered by the analysis tests.
        if (circleOfFifths.match(written).hasMatch() && written != shown) {
            std::fprintf(stderr, "dj-ui: the header says \"%s\" and the file says \"%s\"\n",
                         qPrintable(shown), qPrintable(written));
            return 1;
        }
        std::fprintf(stdout, "dj-ui: the key is measured, stored and shown\n");
        return 0;
    }

    if (parser.isSet(browserStartOption)) {
        // Where the browser opens matters to a working DJ more than any pixel: it is the difference
        // between two clicks and a walk through the file system.
        const QString sessionFile = parser.value(sessionFileOption);
        if (sessionFile.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --browser-start-check needs --session-file\n");
            return 2;
        }
        QTemporaryDir first;
        QTemporaryDir second;
        if (!first.isValid() || !second.isValid()) {
            std::fprintf(stderr, "dj-ui: cannot make temporary folders\n");
            return 1;
        }
        for (const QString &folder : {first.path(), second.path()}) {
            QFile file(folder + QStringLiteral("/track.wav"));
            if (!file.open(QIODevice::WriteOnly)) {
                std::fprintf(stderr, "dj-ui: cannot write into %s\n", qPrintable(folder));
                return 1;
            }
            file.write("RIFF", 4);
        }
        QJsonObject remembered;
        remembered.insert(QStringLiteral("folder"), first.path());
        QFile state(sessionFile);
        if (!state.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
            std::fprintf(stderr, "dj-ui: cannot write %s\n", qPrintable(sessionFile));
            return 1;
        }
        state.write(QJsonDocument(remembered).toJson());
        state.close();

        // The check needs its own run of the startup path, so the file it wrote is read now.
        SessionState reloaded;
        reloaded.setFile(sessionFile);
        if (!reloaded.load()) {
            std::fprintf(stderr, "dj-ui: the session file was not readable\n");
            return 1;
        }
        browser.open(reloaded.folder());
        std::fprintf(stdout, "dj-ui: browser opened %s (remembered)\n", qPrintable(browser.folder()));
        if (browser.folder() != first.path()) {
            std::fprintf(stderr, "dj-ui: the browser did not open the remembered folder\n");
            return 1;
        }

        // A load elsewhere has to move both the browser and the file.
        const QString elsewhere = second.path() + QStringLiteral("/track.wav");
        bridge->load(QStringLiteral("A"), elsewhere);
        std::fprintf(stdout, "dj-ui: browser moved to %s after a load\n", qPrintable(browser.folder()));
        if (browser.folder() != second.path()) {
            std::fprintf(stderr, "dj-ui: a load did not move the browser\n");
            return 1;
        }
        QFile written(sessionFile);
        if (!written.open(QIODevice::ReadOnly)) {
            std::fprintf(stderr, "dj-ui: the session file vanished\n");
            return 1;
        }
        const QString stored =
            QJsonDocument::fromJson(written.readAll()).object().value(QStringLiteral("folder")).toString();
        if (stored != second.path()) {
            std::fprintf(stderr, "dj-ui: the session file still says %s\n", qPrintable(stored));
            return 1;
        }
        std::fprintf(stdout, "dj-ui: the folder survives a restart and follows the last load\n");
        return 0;
    }

    if (parser.isSet(browserRestoreOption)) {
        // The second start: a new process, a layout file left by the first one, and no other hint. What
        // the panel and the deck hold now is what a person would see, not what the file says.
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        // The splitter has not laid anything out until the scene has had a frame: reading geometry
        // before that measures the minimum sizes, not the settings.
        QEventLoop settle;
        QTimer::singleShot(600, &settle, &QEventLoop::quit);
        settle.exec();
        QQuickItem *panel = findItem(window->contentItem(), QStringLiteral("browserPanel"));
        QQuickItem *detail = findItem(window->contentItem(), QStringLiteral("deckDetail_A"));
        if (panel == nullptr || detail == nullptr) {
            std::fprintf(stderr, "dj-ui: the shell has no browser panel or no deck waveform\n");
            return 1;
        }
        QQuickItem *shellItem = findItem(window->contentItem(), QStringLiteral("shellLayout"));
        if (shellItem == nullptr || shellItem->height() <= 0.0) {
            std::fprintf(stderr, "dj-ui: the shell has no height to divide\n");
            return 1;
        }
        const int rail = panel->property("railWidth").toInt();
        const int font = panel->property("listFont").toInt();
        const double span = detail->property("zoomSpan").toDouble();
        const double heightShare = panel->height() / shellItem->height() * 100.0;
        if (qAbs(heightShare - layout.browserPercent()) > 2.0) {
            std::fprintf(stderr,
                         "dj-ui: the browser came back %0.1f%% tall instead of %0.1f%%\n",
                         heightShare, layout.browserPercent());
            return 1;
        }
        if (rail != layout.browserRailWidth() || font != layout.browserFont()
            || qAbs(span - layout.waveformSpan()) > 1e-6) {
            std::fprintf(stderr,
                         "dj-ui: the shell did not come back as it was left (rail %d of %d, font %d "
                         "of %d, span %.3f of %.3f)\n",
                         rail, layout.browserRailWidth(), font, layout.browserFont(), span,
                         layout.waveformSpan());
            return 1;
        }
        std::fprintf(stdout,
                     "dj-ui: a new run opens with the rail at %d px, the browser text at %d px, the "
                     "browser %0.1f%% tall and the deck zoom at a span of %s, the numbers the last run "
                     "left\n",
                     rail, font, heightShare, qPrintable(QString::number(span, 'f', 3)));
        // The modes of the deck are settings too: the last run left snap and quantize off and the scrub
        // audible, and both the runtime and the switches have to say so.
        auto *snapSwitch = findItem(window->contentItem(), QStringLiteral("snapGlobal"));
        auto *quantSwitch = findItem(window->contentItem(), QStringLiteral("quantizeGlobal"));
        if (snapSwitch == nullptr || quantSwitch == nullptr) {
            std::fprintf(stderr, "dj-ui: the mixer has no global switches to read back\n");
            return 1;
        }
        const bool snapLit = snapSwitch->property("on").toBool();
        const bool quantLit = quantSwitch->property("on").toBool();
        std::fprintf(stdout,
                     "dj-ui: the deck's modes came back as they were left: snap %s, quantize %s, scrub "
                     "%s; the switches read %s and %s\n",
                     settings.snapGrid() ? "on" : "off", settings.quantizeGrid() ? "on" : "off",
                     settings.scrubAudible() ? "audible" : "silent", snapLit ? "lit" : "dark",
                     quantLit ? "lit" : "dark");
        if (settings.snapGrid() || settings.quantizeGrid() || !settings.scrubAudible() || snapLit
            || quantLit) {
            std::fprintf(stderr, "dj-ui: the deck's modes did not come back as the last run left them\n");
            return 1;
        }
        return 0;
    }

    if (parser.isSet(browserCheckOption)) {
        return runBrowserCheck(engine, browser, layout, settings);
    }

    if (parser.isSet(pageCheckOption)) {
        if (!parser.isSet(connectOption)) {
            std::fprintf(stderr, "dj-ui: --collection-page-check needs --connect\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        return runCollectionPageCheck(engine, browser, *bridge, parser.value(importFixtureOption),
                                      parser.values(collectionRootOption).value(0));
    }

    if (parser.isSet(importCheckOption)) {
        // The import is the engine's work, so this check is useless without one: say so rather than
        // report a shell that draws dialogs for a batch nobody runs.
        if (!parser.isSet(connectOption)) {
            std::fprintf(stderr, "dj-ui: --import-check needs --connect\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        return runImportCheck(engine, browser, layout, *bridge, parser.value(importFixtureOption),
                              parser.values(collectionRootOption).value(0));
    }

    if (parser.isSet(endCheckOption)) {
        bridge->connectToEngine(parser.value(connectOption));
        return runEndCheck(engine, *bridge, parser.value(loadOption));
    }

    if (parser.isSet(scrollCheckOption)) {
        bridge->connectToEngine(parser.value(connectOption));
        return runScrollCheck(engine, browser, *bridge, parser.value(collectionRootOption));
    }

    if (parser.isSet(lockCheckOption)) {
        bridge->connectToEngine(parser.value(connectOption));
        return runLockCheck(engine, browser, *bridge, parser.value(importFixtureOption),
                            parser.values(collectionRootOption).value(0));
    }

    if (parser.isSet(playlistCheckOption)) {
        // The playlists are the engine's index and the shell's panel at once: a check without an engine
        // would test half of the chain and call it whole.
        if (!parser.isSet(connectOption)) {
            std::fprintf(stderr, "dj-ui: --playlist-check needs --connect\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        return runPlaylistCheck(
            engine, browser, *bridge, parser.value(importFixtureOption),
            parser.value(loadSecondOption).isEmpty() ? parser.value(importFixtureOption)
                                                     : parser.value(loadSecondOption),
            parser.values(collectionRootOption).value(0));
    }

    if (parser.isSet(sortCheckOption)) {
        bridge->connectToEngine(parser.value(connectOption));
        return runSortCheck(engine, browser, layout, parser.value(importFixtureOption),
                            parser.values(collectionRootOption).value(0));
    }

    if (parser.isSet(genreCheckOption)) {
        // The chips are answered by the index: this check without an engine measured a list nobody asked
        // about, and reported the emptiness as a genre bug.
        bridge->connectToEngine(parser.value(connectOption));
        return runGenreCheck(engine, browser, layout, parser.value(importFixtureOption),
                             parser.values(collectionRootOption).value(0));
    }

    if (parser.isSet(collectionProbeOption)) {
        return runCollectionProbe(browser, parser.values(collectionRootOption).value(0));
    }

    if (parser.isSet(collectionLoadCheckOption)) {
        bridge->connectToEngine(parser.value(connectOption));
        return runCollectionLoadCheck(engine, browser, *bridge,
                                      parser.values(collectionRootOption).value(0));
    }

    if (parser.isSet(analyzeCheckOption)) {
        if (!parser.isSet(connectOption)) {
            std::fprintf(stderr, "dj-ui: --analyze-check needs --connect\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        bridge->connectToEngine(parser.value(connectOption));
        return runAnalyzeCheck(engine, browser, layout, *bridge, parser.value(importFixtureOption),
                               parser.values(collectionRootOption).value(0));
    }

    if (parser.isSet(themeCheckOption) || parser.isSet(screenshotOption)) {
        // A window that never reaches a screen still has to look right: grabbing it here, from the
        // engine the shell actually uses, is how the theme gets measured instead of described.
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        QEventLoop settle;
        QTimer::singleShot(600, &settle, &QEventLoop::quit);
        settle.exec();
        const QImage image = window->grabWindow();
        if (parser.isSet(screenshotOption)) {
            const QString target = parser.value(screenshotOption);
            if (!image.save(target)) {
                std::fprintf(stderr, "dj-ui: cannot write %s\n", qPrintable(target));
                return 1;
            }
        }
        // The theme, measured: the reference interface is dark greys with one blue accent, and a
        // regression to the light default shows up as a majority of bright pixels, not as a missing
        // property.
        QMap<QString, int> counts;
        int bright = 0;
        int total = 0;
        for (int y = 0; y < image.height(); y += 4) {
            for (int x = 0; x < image.width(); x += 4) {
                const QColor pixel = image.pixelColor(x, y);
                const int luma = (pixel.red() * 299 + pixel.green() * 587 + pixel.blue() * 114) / 1000;
                if (luma > 128) {
                    ++bright;
                }
                ++total;
                counts[pixel.name()] += 1;
            }
        }
        if (parser.isSet(screenshotOption)) {
            std::fprintf(stdout, "dj-ui: wrote %s (%dx%d)\n",
                         qPrintable(parser.value(screenshotOption)), image.width(), image.height());
        }
        std::fprintf(stdout, "dj-ui: theme: %.1f%% bright pixels, dominant colours:",
                     100.0 * bright / qMax(1, total));
        QList<QPair<int, QString>> ranked;
        for (auto it = counts.cbegin(); it != counts.cend(); ++it) {
            ranked.append({it.value(), it.key()});
        }
        std::sort(ranked.begin(), ranked.end(), [](const auto &left, const auto &right) {
            return left.first > right.first;
        });
        for (int index = 0; index < ranked.size() && index < 4; ++index) {
            std::fprintf(stdout, " %s %.0f%%", qPrintable(ranked.at(index).second),
                         100.0 * ranked.at(index).first / qMax(1, total));
        }
        std::fprintf(stdout, "\n");
        if (bright * 4 > total) {
            std::fprintf(stderr, "dj-ui: the shell is light: %.0f%% of its pixels are bright\n",
                         100.0 * bright / qMax(1, total));
            return 1;
        }
        return 0;
    }

    if (parser.isSet(smokeOption)) {
        if (g_warningCount != 0) {
            std::fprintf(stderr, "dj-ui: %d Qt warning(s) while loading the scene\n", g_warningCount);
            return 1;
        }
        std::fprintf(stdout, "dj-ui: QML scene loaded, no Qt warnings\n");
        return 0;
    }

    const QString path = parser.isSet(connectOption) ? parser.value(connectOption)
                                                     : defaultSocketPath();
    bridge->connectToEngine(path);
    std::fprintf(stdout, "dj-ui: connecting to %s\n", qPrintable(path));

    if (parser.isSet(exitAfterOption)) {
        bool ok = false;
        const int after = parser.value(exitAfterOption).toInt(&ok);
        if (!ok || after < 0) {
            std::fprintf(stderr, "dj-ui: --exit-after needs milliseconds\n");
            return 2;
        }
        // The shutdown path, exercised instead of described: close the window the way a person does.
        QTimer::singleShot(after, qApp, &QCoreApplication::quit);
    }

    return app.exec();
}
