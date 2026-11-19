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
#ifdef Q_OS_WIN
    // A named pipe, not a file: the engine adds the `\\.\pipe\` prefix itself, and the name carries the
    // user so two sessions on one machine do not meet.
    const QString user = qEnvironmentVariable("USERNAME", QStringLiteral("dj"));
    return QStringLiteral("hauler-dj-engine-") + user;
#else
    const QByteArray runtime = qgetenv("XDG_RUNTIME_DIR");
    if (!runtime.isEmpty()) {
        return QString::fromLocal8Bit(runtime) + QStringLiteral("/hauler-dj-engine.sock");
    }
    return QStringLiteral("/tmp/hauler-dj-engine.sock");
#endif
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
    const QCommandLineOption analyzeBatchCheckOption(
        QStringLiteral("analyze-batch-check"),
        QStringLiteral("A batch over the collection: the list stands still while it runs and Cancel stops it "
                       "(needs --collection-root)."));
    const QCommandLineOption wholeListCheckOption(
        QStringLiteral("whole-list-check"),
        QStringLiteral("The list is read whole: every row of a four-hundred-row collection arrives without a "
                       "scroll (needs --collection-root and --import-fixture)."));
    const QCommandLineOption reconnectCheckOption(
        QStringLiteral("reconnect-check"),
        QStringLiteral("The shell is started before the engine and has to come back on its own: one attempt "
                       "at startup used to leave a window that was alive and deaf."));
    const QCommandLineOption selectionCheckOption(
        QStringLiteral("selection-check"),
        QStringLiteral("Shift+arrows, PgUp/PgDown and Shift+Home/End take runs in the browser's lists "
                       "(needs --collection-root and --import-fixture)."));
    const QCommandLineOption layoutMenuCheckOption(
        QStringLiteral("layout-menu-check"),
        QStringLiteral("The layout is chosen from the strip's list, the label follows, and no page is "
                       "asked again."));
    const QCommandLineOption layoutNumbersCheckOption(
        QStringLiteral("layout-numbers-check"),
        QStringLiteral("Every layout keeps its own numbers, and a file from the previous build still "
                       "opens (needs --layout-file)."));
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
    parser.addOption(analyzeBatchCheckOption);
    parser.addOption(wholeListCheckOption);
    parser.addOption(reconnectCheckOption);
    parser.addOption(selectionCheckOption);
    parser.addOption(layoutMenuCheckOption);
    parser.addOption(layoutNumbersCheckOption);
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
        return runMasterCheck(engine, bridge.get(), settings, parser, loadOption, connectOption);
    }

    if (parser.isSet(curveCheckOption)) {
        return runCurveCheck(engine, bridge.get(), parser, connectOption);
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
        return runWheelCheck(engine, bridge.get(), parser, connectOption);
    }

    if (parser.isSet(gainCurveCheckOption)) {
        return runGainCurveCheck(engine, settings);
    }

    if (parser.isSet(tempoCheckOption)) {
        return runTempoCheck(engine, bridge.get(), parser, loadOption, connectOption);
    }

    if (parser.isSet(loopBeatsOption)) {
        return runLoopBeatsCheck(engine, bridge.get(), parser, loadOption, connectOption);
    }

    if (parser.isSet(transportExtrasOption)) {
        return runTransportExtrasCheck(engine, bridge.get(), parser, loadOption, connectOption);
    }

    if (parser.isSet(gridHereOption)) {
        return runGridHereCheck(engine, bridge.get(), parser, loadOption, connectOption);
    }

    if (parser.isSet(tempoRecheckOption)) {
        return runTempoRecheckCheck(engine, bridge.get(), parser, loadOption, connectOption);
    }

    if (parser.isSet(gridReconnectOption)) {
        return runGridReconnectCheck(engine, bridge.get(), parser, connectOption);
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
        return runMetronomeCheck(engine, bridge.get(), parser, loadOption, connectOption);
    }

    if (parser.isSet(scratchOption)) {
        return runScratchCheck(engine, bridge.get(), parser, loadOption, connectOption);
    }

    if (parser.isSet(gridLinesOption)) {
        return runGridLines(engine, bridge, parser, connectOption, loadOption);
    }

    if (parser.isSet(gridCreateOption)) {
        return runGridCreateCheck(engine, bridge.get(), parser, loadOption, loadSecondOption, connectOption);
    }

    if (parser.isSet(gridCheckOption)) {
        return runGridCheck(engine, bridge.get(), parser, loadOption, connectOption);
    }

    if (parser.isSet(eqCheckOption)) {
        return runEqCheck(engine, bridge, parser, connectOption);
    }






    if (parser.isSet(loopOption)) {
        return runLoop(engine, bridge, parser, connectOption, loadOption);
    }

    if (parser.isSet(scenarioOption)) {
        return runScenario(engine, bridge.get(), parser, loadOption, connectOption);
    }

    if (parser.isSet(padsOption)) {
        return runPadsCheck(engine, bridge.get(), parser, loadOption, connectOption);
    }

    if (parser.isSet(transportOption)) {
        return runTransportCheck(engine, bridge.get(), settings, parser, loadOption, connectOption);
    }

    if (parser.isSet(fourDeckOption)) {
        return runFourDeckCheck(engine, bridge.get(), parser, loadOption, connectOption);
    }

    if (parser.isSet(syncSoakOption)) {
        return runSyncSoak(engine, bridge, parser, connectOption, soakSecondsOption);
    }

    if (parser.isSet(syncCheckOption)) {
        return runSyncCheck(engine, bridge, parser, connectOption, loadOption, loadSecondOption);
    }

    if (parser.isSet(pitchOption)) {
        return runPitchCheck(engine, bridge.get(), parser, loadOption, connectOption);
    }

    if (parser.isSet(neutralOption)) {
        return runNeutralCheck(engine, bridge.get(), parser, connectOption);
    }

    if (parser.isSet(spaceCheckOption)) {
        return runSpaceCheck(engine, bridge.get(), parser, loadOption, connectOption);
    }

    if (parser.isSet(refusalCheckOption)) {
        return runRefusalCheck(engine, browser, bridge.get(), settings, parser, loadOption, connectOption);
    }

    if (parser.isSet(stripCheckOption)) {
        return runStripCheck(engine, bridge.get(), parser, loadOption, connectOption);
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
        return runBrowserRestoreCheck(engine, layout, settings);
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
        return runCollectionPageCheck(engine, browser, *bridge, layout,
                                      parser.value(importFixtureOption),
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

    if (parser.isSet(analyzeBatchCheckOption)) {
        bridge->connectToEngine(parser.value(connectOption));
        return runAnalyzeBatchCheck(engine, browser, layout, *bridge, parser.value(importFixtureOption),
                                    parser.values(collectionRootOption).value(0));
    }

    if (parser.isSet(reconnectCheckOption)) {
        // A name of its own: the default endpoint may be busy with the DJ's own engine.
        return checks::runReconnectCheck(*bridge,
                                         QStringLiteral("hauler-dj-reconnect-check-%1")
                                             .arg(QCoreApplication::applicationPid()));
    }

    if (parser.isSet(wholeListCheckOption)) {
        bridge->connectToEngine(parser.value(connectOption));
        return runWholeListCheck(engine, browser, *bridge, parser.values(collectionRootOption).value(0),
                                 parser.value(importFixtureOption));
    }

    if (parser.isSet(selectionCheckOption)) {
        bridge->connectToEngine(parser.value(connectOption));
        return runSelectionCheck(engine, browser, *bridge, parser.values(collectionRootOption).value(0),
                                 parser.value(importFixtureOption));
    }

    if (parser.isSet(layoutMenuCheckOption)) {
        return runLayoutMenuCheck(engine, browser, layout);
    }

    if (parser.isSet(layoutNumbersCheckOption)) {
        return runLayoutNumbersCheck(layout, parser.value(layoutFileOption));
    }

    if (parser.isSet(scrollCheckOption)) {
        bridge->connectToEngine(parser.value(connectOption));
        return runScrollCheck(engine, browser, *bridge, parser.value(collectionRootOption));
    }

    if (parser.isSet(lockCheckOption)) {
        bridge->connectToEngine(parser.value(connectOption));
        return runLockCheck(engine, browser, *bridge, parser.value(connectOption),
                            parser.value(importFixtureOption),
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
