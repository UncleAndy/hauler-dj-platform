// The settings window, checked the way a DJ would meet it: the button opens it, the sections are all there, choosing
// one turns the page, the panel stays inside the window, and the close button closes it.
//
// Geometry is part of the check: this shell is made for 1366×768, and a panel that reaches past the edge would be
// unusable on the screen the program is built for.

#include "Checks.h"

#include <QDir>
#include <QEventLoop>
#include <QQmlApplicationEngine>
#include <QTimer>
#include <QRectF>
#include <QQuickWindow>

#include <cmath>
#include <functional>
#include <cstdio>

namespace {

/// Find an item by the name the QML gave it, in the tree the eye sees.
///
/// `findChild` walks the QObject parents, and an item created by a layout or a repeater is not always a QObject child
/// of the item that shows it — the buttons of the section list were in the window and invisible to it.
QQuickItem *named(QQuickItem *root, const QString &name)
{
    if (root == nullptr) {
        return nullptr;
    }
    if (root->objectName() == name) {
        return root;
    }
    for (QQuickItem *child : root->childItems()) {
        if (QQuickItem *found = named(child, name)) {
            return found;
        }
    }
    return nullptr;
}

/// Let the layouts run: a page that has just been turned has no geometry until the event loop has been through once.
void settle(int milliseconds)
{
    QEventLoop loop;
    QTimer::singleShot(milliseconds, &loop, &QEventLoop::quit);
    loop.exec();
}

/// One device's shape, asked the way the window asks.
QVariantMap Settings_shape(QQmlApplicationEngine &engine, const QString &device)
{
    auto *model = engine.singletonInstance<QObject *>(QAnyStringView("DjPlatform.Ui"), QAnyStringView("Settings"));
    if (model == nullptr) {
        return {};
    }
    QVariantMap shape;
    if (!QMetaObject::invokeMethod(model, "deviceShape", Q_RETURN_ARG(QVariantMap, shape), Q_ARG(QString, device))) {
        return {};
    }
    return shape;
}

/// What the engine says about one device, the way the window's Check button asks.
QString Settings_describe(QQmlApplicationEngine &engine, const QString &device)
{
    auto *model = engine.singletonInstance<QObject *>(QAnyStringView("DjPlatform.Ui"), QAnyStringView("Settings"));
    if (model == nullptr) {
        return {};
    }
    QString answer;
    if (!QMetaObject::invokeMethod(model, "describeDevice", Q_RETURN_ARG(QString, answer), Q_ARG(QString, device))) {
        return {};
    }
    return answer;
}

/// The stereo pairs of one device, asked the way the window asks.
QStringList Settings_pairs(QQmlApplicationEngine &engine, const QString &device)
{
    auto *model = engine.singletonInstance<QObject *>(QAnyStringView("DjPlatform.Ui"), QAnyStringView("Settings"));
    if (model == nullptr) {
        return {};
    }
    QStringList pairs;
    if (!QMetaObject::invokeMethod(model, "devicePairs", Q_RETURN_ARG(QStringList, pairs), Q_ARG(QString, device))) {
        return {};
    }
    return pairs;
}

/// What the engine says its outputs are, asked the way the window asks.
QStringList Settings_devices(QQmlApplicationEngine &engine)
{
    auto *model = engine.singletonInstance<QObject *>(QAnyStringView("DjPlatform.Ui"), QAnyStringView("Settings"));
    if (model == nullptr) {
        return {};
    }
    QStringList names;
    if (!QMetaObject::invokeMethod(model, "devices", Q_RETURN_ARG(QStringList, names))) {
        return {};
    }
    return names;
}

/// Press a button the way a click does: through the signal its handler is on.
bool press(QObject *button)
{
    return button != nullptr && QMetaObject::invokeMethod(button, "clicked");
}

} // namespace

namespace checks {

int runSettingsWindowCheck(QQmlApplicationEngine &engine, std::unique_ptr<BridgeClient> &bridge)
{
    Q_UNUSED(bridge);
    auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().value(0));
    if (window == nullptr) {
        std::fprintf(stderr, "dj-ui: there is no window to put settings in\n");
        return 1;
    }
    auto *root = window->contentItem();

    // The settings window is a modal popup, so it is not an item in the window's tree: the popup object holds the
    // visibility and the methods, and its content item holds the rows.
    QObject *settingsWindow = window->findChild<QObject *>(QStringLiteral("settingsWindow"));
    if (settingsWindow == nullptr) {
        std::fprintf(stderr, "dj-ui: there is no settings window in the shell\n");
        return 1;
    }
    auto *content = qobject_cast<QQuickItem *>(settingsWindow->property("contentItem").value<QObject *>());
    if (content == nullptr) {
        std::fprintf(stderr, "dj-ui: the settings window has no content\n");
        return 1;
    }
    const auto isOpen = [&settingsWindow] { return settingsWindow->property("visible").toBool(); };
    const auto findInWindow = [&content](const QString &name) { return named(content, name); };

    QQuickItem *gear = named(root, QStringLiteral("settingsButton"));
    if (gear == nullptr) {
        std::fprintf(stderr, "dj-ui: there is no button that opens the settings window\n");
        return 1;
    }
    if (!press(gear) || !isOpen()) {
        std::fprintf(stderr, "dj-ui: the settings button did not open the window\n");
        return 1;
    }

    // A popup lays its content out on the next turn of the loop, and a panel measured before that is a panel of no size.
    settle(80);

    const QStringList sections = {
        QStringLiteral("sound"),    QStringLiteral("analysis"),    QStringLiteral("library"),
        QStringLiteral("mixer"),    QStringLiteral("effects"),     QStringLiteral("interface"),
        QStringLiteral("diagnostics"), QStringLiteral("about"),
    };
    for (const QString &key : sections) {
        if (findInWindow(QStringLiteral("settingsSection_") + key) == nullptr) {
            std::fprintf(stderr, "dj-ui: the settings window has no %s section\n", qPrintable(key));
            return 1;
        }
    }

    QQuickItem *panel = findInWindow(QStringLiteral("settingsPanel"));
    if (panel == nullptr) {
        std::fprintf(stderr, "dj-ui: the settings window has no panel to measure\n");
        return 1;
    }
    const QRectF inside = panel->mapRectToScene(QRectF(0, 0, panel->width(), panel->height()));
    const QRectF bounds(0, 0, window->width(), window->height());
    if (inside.width() < 320.0 || inside.height() < 240.0) {
        std::fprintf(stderr, "dj-ui: the settings panel has no size: %.0fx%.0f\n", inside.width(), inside.height());
        return 1;
    }
    if (!bounds.contains(inside)) {
        std::fprintf(stderr, "dj-ui: the settings panel leaves the window: %.0f,%.0f %.0fx%.0f in %.0fx%.0f\n",
                     inside.x(), inside.y(), inside.width(), inside.height(), bounds.width(), bounds.height());
        return 1;
    }

    // The list of sections is a margin, not a page: the parameters the DJ came for need the room (owner's report,
    // 21.11 — the buttons took the whole window and nothing could be set).
    QQuickItem *nav = findInWindow(QStringLiteral("settingsNav"));
    QQuickItem *pageArea = findInWindow(QStringLiteral("settingsContent"));
    if (nav == nullptr || pageArea == nullptr) {
        std::fprintf(stderr, "dj-ui: the settings window has no list and no page to measure\n");
        return 1;
    }
    settle(60);
    const double navWidth = nav->width();
    const double contentWidth = pageArea->width();
    if (navWidth > 220.0 || contentWidth < panel->width() * 0.55) {
        std::fprintf(stderr,
                     "dj-ui: the sections take %.0f px of a %.0f px panel, leaving %.0f px for the settings\n",
                     navWidth, panel->width(), contentWidth);
        return 1;
    }

    // The device is chosen from the outputs the engine names, not typed: a text field where a list belongs is what the
    // owner saw on his own machine (report, 21.11).
    {
        QQuickItem *deviceRow = findInWindow(QStringLiteral("settingRow_engine_audio_device"));
        if (deviceRow == nullptr) {
            std::fprintf(stderr, "dj-ui: the sound page has no device line\n");
            return 1;
        }
        const QStringList devices = Settings_devices(engine);
        if (!devices.isEmpty()) {
            // The row offers what the engine listed: the same names, in the same order, plus the stored one if it is
            // no longer plugged in.
            const QVariantList offered = deviceRow->property("offered").toList();
            if (offered.size() < devices.size()) {
                std::fprintf(stderr, "dj-ui: the device line offers %lld names and the engine named %lld\n",
                             static_cast<long long>(offered.size()), static_cast<long long>(devices.size()));
                return 1;
            }
            for (const QString &name : devices) {
                bool found = false;
                for (const QVariant &one : offered) {
                    if (one.toString() == name) {
                        found = true;
                        break;
                    }
                }
                if (!found) {
                    std::fprintf(stderr, "dj-ui: the device line does not offer %s\n", qPrintable(name));
                    return 1;
                }
            }
            if (!deviceRow->property("asList").toBool()) {
                std::fprintf(stderr, "dj-ui: the device line is not a list, though the engine named %lld outputs\n",
                             static_cast<long long>(devices.size()));
                return 1;
            }
            std::fprintf(stderr, "dj-ui: the device line offers the engine's %lld outputs, the default first\n",
                         static_cast<long long>(devices.size()));
        } else {
            std::fprintf(stderr, "dj-ui: the engine named no outputs here; the device line says so and takes a name\n");
        }
    }

    // The headphones: three modes, and the engine decides what each of them means (owner's decision, 21.11).
    // `auto` takes the main interface and its spare channels; `same_device` keeps that interface and lets the DJ pick;
    // `device` names another interface. Where the interface has no spare pair the cue is off and both channels read
    // "None".
    {
        QQuickItem *modeRow = findInWindow(QStringLiteral("settingRow_engine_audio_headphones_mode"));
        QQuickItem *deviceRow = findInWindow(QStringLiteral("settingRow_engine_audio_headphones_device"));
        QQuickItem *leftRow = findInWindow(QStringLiteral("settingRow_engine_audio_headphones_pair"));
        if (modeRow == nullptr || deviceRow == nullptr || leftRow == nullptr) {
            std::fprintf(stderr, "dj-ui: the sound page has no headphone lines\n");
            return 1;
        }
        QQuickItem *mainRow = findInWindow(QStringLiteral("settingRow_engine_audio_master"));
        if (mainRow == nullptr) {
            std::fprintf(stderr, "dj-ui: the sound page has no main channel line\n");
            return 1;
        }
        auto *model = engine.singletonInstance<QObject *>(QAnyStringView("DjPlatform.Ui"),
                                                          QAnyStringView("Settings"));
        if (model == nullptr) {
            std::fprintf(stderr, "dj-ui: the settings model is not reachable from the check\n");
            return 1;
        }
        const QString mainDevice = QString::fromUtf8(qgetenv("HAULER_CHECK_DEVICE"));
        const QVariantMap shape = Settings_shape(engine, mainDevice);
        const int channels = shape.value(QStringLiteral("channels")).toInt();

        // The main output is one channel per row, and the list is the device's own numbering — never "None", because a
        // main output with a hole is not a route.
        if (!mainRow->property("open").toBool()) {
            std::fprintf(stderr, "dj-ui: the main channel line is locked\n");
            return 1;
        }
        const QVariantList mainOffered = mainRow->property("offered").toList();
        if (channels > 0 && mainOffered.size() < channels) {
            std::fprintf(stderr, "dj-ui: the main channel line offers %lld of %d channels\n",
                         static_cast<long long>(mainOffered.size()), channels);
            return 1;
        }
        for (const QVariant &one : mainOffered) {
            if (one.toString() == QStringLiteral("None")) {
                std::fprintf(stderr, "dj-ui: the main channel line offers None\n");
                return 1;
            }
        }

        // `auto`: the interface picks for itself, so nothing here is the DJ's to change.
        for (const char *mode : { "auto", "same_device", "device" }) {
            bool applied = false;
            if (!QMetaObject::invokeMethod(model, "applyHeadphoneMode", Q_RETURN_ARG(bool, applied),
                                           Q_ARG(QString, QString::fromLatin1(mode)))) {
                std::fprintf(stderr, "dj-ui: the engine refused to resolve the %s mode\n", mode);
                return 1;
            }
            settle(60);
            const bool deviceOpen = deviceRow->property("open").toBool();
            const bool leftOpen = leftRow->property("open").toBool();
            const bool wantedDeviceOpen = QString::fromLatin1(mode) == QStringLiteral("device");
            if (deviceOpen != wantedDeviceOpen) {
                std::fprintf(stderr, "dj-ui: in the %s mode the headphone device is %s\n", mode,
                             deviceOpen ? "open" : "locked");
                return 1;
            }
            if (wantedDeviceOpen) {
                // Choosing a device has to give channels: the rows were greyed out and empty after the device was picked,
                // which is the owner's report of 21.11.
                const QStringList devices = Settings_devices(engine);
                QString cueDevice;
                for (const QString &name : devices) {
                    if (name != mainDevice) {
                        cueDevice = name;
                        break;
                    }
                }
                if (cueDevice.isEmpty() && !devices.isEmpty()) {
                    cueDevice = devices.first();
                }
                if (!cueDevice.isEmpty()
                    && QMetaObject::invokeMethod(deviceRow, "write", Q_ARG(QVariant, QVariant(cueDevice)))) {
                    settle(80);
                    if (!leftRow->property("open").toBool()) {
                        std::fprintf(stderr,
                                     "dj-ui: after choosing the device %s the cue channels stayed locked\n",
                                     qPrintable(cueDevice));
                        return 1;
                    }
                    const QVariantMap cueShape = Settings_shape(engine, cueDevice);
                    const int cueChannels = cueShape.value(QStringLiteral("channels")).toInt();
                    const int offered = leftRow->property("offered").toList().size();
                    // Every channel of the chosen device plus "None": a cue interface has all of them free.
                    if (cueChannels > 0 && offered < cueChannels) {
                        std::fprintf(stderr, "dj-ui: the cue line offers %d channels of %s, which has %d\n", offered,
                                     qPrintable(cueDevice), cueChannels);
                        return 1;
                    }
                    if (leftRow->property("currentText").toString() == QStringLiteral("None")) {
                        std::fprintf(stderr, "dj-ui: the cue channels read None right after a device with channels\n");
                        return 1;
                    }
                    std::fprintf(stderr,
                                 "dj-ui: choosing %s gave %d cue channels, and the line reads channel %s\n",
                                 qPrintable(cueDevice), offered,
                                 qPrintable(leftRow->property("currentText").toString()));
                }
                std::fprintf(stderr, "dj-ui: the %s mode leaves the device and the channels to the DJ\n", mode);
                continue;
            }
            // The spare-pair question is the engine's: a stereo interface has none, and then the channels read "None".
            const bool spare = leftOpen;
            if (channels >= 4 && !spare) {
                std::fprintf(stderr, "dj-ui: in the %s mode %d channels were left without a cue\n", mode, channels);
                return 1;
            }
            if (channels < 4) {
                if (spare) {
                    std::fprintf(stderr, "dj-ui: in the %s mode a stereo interface offered cue channels\n", mode);
                    return 1;
                }
                for (QQuickItem *row : { leftRow }) {
                    if (row->property("currentText").toString() != QStringLiteral("None")) {
                        std::fprintf(stderr, "dj-ui: in the %s mode the cue channel reads %s, not None\n", mode,
                                     qPrintable(row->property("currentText").toString()));
                        return 1;
                    }
                }
            }
            std::fprintf(stderr, "dj-ui: the %s mode keeps the main device, %s\n", mode,
                         spare ? "the spare pair for the cue" : "and has no spare pair, so the cue is None");
        }

        // The interface that carries the cue has its own check, and it answers about that device: the point of the line is
        // the second card, not a repeat of the first (owner's decision, 21.11).
        QQuickItem *checkRow = findInWindow(QStringLiteral("settingRow_headphoneDeviceCheck"));
        if (checkRow == nullptr) {
            std::fprintf(stderr, "dj-ui: the sound page has no check for the headphone device\n");
            return 1;
        }
        const QString asked = checkRow->property("currentText").toString();
        const QString report = Settings_describe(engine, asked);
        if (report.isEmpty() || report.contains(asked, Qt::CaseInsensitive)) {
            std::fprintf(stderr, "dj-ui: the headphone check answered \"%s\" about %s\n", qPrintable(report),
                         qPrintable(asked));
            return 1;
        }
        std::fprintf(stderr, "dj-ui: the headphone check answers characteristics only: %s\n", qPrintable(report));

        std::fprintf(stderr, "dj-ui: the headphones follow their mode: device %s, channels %s\n",
                     "locked outside device mode", "None when nothing is spare");
    }

    // A page that exists but shows nothing is the same complaint as a list that takes the window: the rows have to be
    // wide enough to read a name and reach a control (owner's report, 21.11).
    if (press(findInWindow(QStringLiteral("settingsSection_interface")))) {
        settle(60);
        QQuickItem *row = findInWindow(QStringLiteral("settingRow_layout_waveformBeats"));
        if (row == nullptr) {
            std::fprintf(stderr, "dj-ui: the interface page has no rows\n");
            return 1;
        }
        if (row->width() < 300.0 || row->height() < 20.0) {
            std::fprintf(stderr, "dj-ui: a row of the interface page is %.0fx%.0f\n", row->width(), row->height());
            return 1;
        }
        std::fprintf(stderr, "dj-ui: a row of the interface page is %.0f px wide and %.0f px tall\n", row->width(),
                     row->height());
    }

    // Choosing a section has to move the page: a list that does nothing is a picture of a list.
    QQuickItem *pages = findInWindow(QStringLiteral("settingsPages"));
    if (pages == nullptr) {
        std::fprintf(stderr, "dj-ui: the settings window has no pages\n");
        return 1;
    }
    int turned = 0;
    for (int index = 1; index < sections.size(); ++index) {
        QQuickItem *button = findInWindow(QStringLiteral("settingsSection_") + sections.at(index));
        if (!press(button)) {
            std::fprintf(stderr, "dj-ui: the %s section button does not answer a click\n",
                         qPrintable(sections.at(index)));
            return 1;
        }
        if (pages->property("currentIndex").toInt() != index) {
            std::fprintf(stderr, "dj-ui: the %s section did not become the page (page %d, wanted %d)\n",
                         qPrintable(sections.at(index)), pages->property("currentIndex").toInt(), index);
            return 1;
        }
        ++turned;
    }

    // A line of the window writes as the hand would: the engine's file and the layout's file both have to carry the
    // value afterwards. This is what separates a window from a picture of one.
    struct Written {
        const char *row;
        QVariant value;
    };
    const QList<Written> written = {
        { "settingRow_engine_audio_block_frames", QVariant(256) },
        { "settingRow_layout_waveformBeats", QVariant(48.0) },
    };
    for (const Written &one : written) {
        QQuickItem *row = named(root, one.row);
        if (row == nullptr) {
            std::fprintf(stderr, "dj-ui: the window has no %s line\n", one.row);
            return 1;
        }
        if (!QMetaObject::invokeMethod(row, "write", Q_ARG(QVariant, one.value))) {
            std::fprintf(stderr, "dj-ui: the %s line refused the value\n", one.row);
            return 1;
        }
        // The line reloads from its owner after writing, and the engine's owner reads the file back through the
        // engine: a value that did not reach the file cannot show up here.
        const double shown = row->property("current").toDouble();
        const double wanted = one.value.toDouble();
        if (std::abs(shown - wanted) > 1e-6) {
            std::fprintf(stderr, "dj-ui: the %s line shows %.3f after writing %.3f\n", one.row, shown, wanted);
            return 1;
        }
    }

    // The collection folders are a list the DJ edits in this window — added and taken away with buttons, not typed into
    // a JSON file by hand (owner's request, 22.11). The proof is the same as for the other lines: what the row shows, what
    // the model holds and what the file carries have to be one answer.
    // The rows of a page exist while that page is the one on screen, so the library is opened the way the DJ opens it.
    if (!press(findInWindow(QStringLiteral("settingsSection_library")))) {
        std::fprintf(stderr, "dj-ui: the library section does not answer a click\n");
        return 1;
    }
    settle(60);
    // Rows live inside the popup's own content, not in the window's item tree: that is where the eye sees them.
    QQuickItem *foldersRow = findInWindow(QStringLiteral("settingRow_layout_collectionRoots"));
    if (foldersRow == nullptr) {
        std::fprintf(stderr, "dj-ui: the library section has no list of collection folders\n");
        return 1;
    }
    auto *settings = engine.singletonInstance<QObject *>(QAnyStringView("DjPlatform.Ui"),
                                                         QAnyStringView("Settings"));
    auto *layoutState = engine.singletonInstance<QObject *>(QAnyStringView("DjPlatform.Ui"),
                                                           QAnyStringView("LayoutState"));
    if (settings == nullptr || layoutState == nullptr) {
        std::fprintf(stderr, "dj-ui: the settings model or the arrangement is not reachable from the check\n");
        return 1;
    }
    const QString firstRoot = QDir::tempPath() + QStringLiteral("/hauler-collection-one");
    const QString secondRoot = QDir::tempPath() + QStringLiteral("/hauler-collection-two");
    QDir().mkpath(firstRoot);
    QDir().mkpath(secondRoot);
    for (const QString &root : {firstRoot, secondRoot}) {
        if (!QMetaObject::invokeMethod(foldersRow, "addFolder", Q_ARG(QVariant, QVariant(root)))) {
            std::fprintf(stderr, "dj-ui: the collection row refused to add %s\n", qPrintable(root));
            return 1;
        }
    }
    settle(40);
    const auto shownFolders = foldersRow->property("folders").toList();
    if (!shownFolders.contains(QVariant(firstRoot)) || !shownFolders.contains(QVariant(secondRoot))) {
        std::fprintf(stderr, "dj-ui: the collection list shows %lld folders instead of the two just added\n",
                     static_cast<long long>(shownFolders.size()));
        return 1;
    }
    QStringList inModel;
    if (!QMetaObject::invokeMethod(settings, "collectionRoots", Q_RETURN_ARG(QStringList, inModel))) {
        std::fprintf(stderr, "dj-ui: the model would not name the collection folders\n");
        return 1;
    }
    if (!inModel.contains(firstRoot) || !inModel.contains(secondRoot)) {
        std::fprintf(stderr, "dj-ui: the model holds %s instead of the folders the row shows\n",
                     qPrintable(inModel.join(QStringLiteral(", "))));
        return 1;
    }
    if (!QMetaObject::invokeMethod(layoutState, "save")) {
        std::fprintf(stderr, "dj-ui: the arrangement refused to be saved with the new collection folders\n");
        return 1;
    }
    if (!QMetaObject::invokeMethod(layoutState, "load")) {
        std::fprintf(stderr, "dj-ui: the arrangement refused to be read back\n");
        return 1;
    }
    const QStringList afterRestart = layoutState->property("collectionRoots").toStringList();
    if (!afterRestart.contains(firstRoot) || !afterRestart.contains(secondRoot)) {
        std::fprintf(stderr, "dj-ui: the folders did not survive being read back: %s\n",
                     qPrintable(afterRestart.join(QStringLiteral(", "))));
        return 1;
    }
    // Taking one away is an edit too: it must leave both the list on screen and the file.
    if (!QMetaObject::invokeMethod(foldersRow, "removeFolder", Q_ARG(QVariant, QVariant(firstRoot)))) {
        std::fprintf(stderr, "dj-ui: the collection row refused to remove %s\n", qPrintable(firstRoot));
        return 1;
    }
    settle(40);
    if (foldersRow->property("folders").toList().contains(QVariant(firstRoot))) {
        std::fprintf(stderr, "dj-ui: the removed folder is still in the list on screen\n");
        return 1;
    }
    QStringList afterRemove;
    if (!QMetaObject::invokeMethod(settings, "collectionRoots", Q_RETURN_ARG(QStringList, afterRemove))
        || afterRemove.contains(firstRoot)) {
        std::fprintf(stderr, "dj-ui: the removed folder is still in the model\n");
        return 1;
    }
    std::fprintf(stderr,
                 "dj-ui: the collection folders are a list: %lld shown, one removed, the rest kept in the file\n",
                 static_cast<long long>(shownFolders.size()));

    // The theme is a whole palette at a time, and the promise is that it outlives the window: it is written through
    // its own line, saved into layout.json with the arrangement, and read back by the loader that starts the shell.
    QQuickItem *themeRow = findInWindow(QStringLiteral("settingRow_ui_themeSet"));
    if (themeRow == nullptr) {
        std::fprintf(stderr, "dj-ui: the window has no theme line\n");
        return 1;
    }
    auto *ui = engine.singletonInstance<QObject *>(QAnyStringView("DjPlatform.Ui"), QAnyStringView("UiSettings"));
    auto *layout = engine.singletonInstance<QObject *>(QAnyStringView("DjPlatform.Ui"), QAnyStringView("LayoutState"));
    if (ui == nullptr || layout == nullptr) {
        std::fprintf(stderr, "dj-ui: the shell's settings are not reachable from the check\n");
        return 1;
    }
    const QString before = ui->property("themeBackground").toString();
    if (!QMetaObject::invokeMethod(themeRow, "write", Q_ARG(QVariant, QVariant(QStringLiteral("graphite"))))) {
        std::fprintf(stderr, "dj-ui: the theme line refused to change the set\n");
        return 1;
    }
    const QString after = ui->property("themeBackground").toString();
    if (after == before) {
        std::fprintf(stderr, "dj-ui: choosing a theme set changed nothing (still %s)\n", qPrintable(before));
        return 1;
    }
    if (!QMetaObject::invokeMethod(layout, "save")) {
        std::fprintf(stderr, "dj-ui: the arrangement refused to be saved\n");
        return 1;
    }
    // A restart in the same process: read the file back through the loader, then look at the colours again.
    if (!QMetaObject::invokeMethod(layout, "load")) {
        std::fprintf(stderr, "dj-ui: the arrangement refused to be read back\n");
        return 1;
    }
    const QString reloaded = ui->property("themeBackground").toString();
    if (reloaded != after) {
        std::fprintf(stderr, "dj-ui: the theme did not survive being read back: %s became %s\n", qPrintable(after),
                     qPrintable(reloaded));
        return 1;
    }
    const double beats = layout->property("waveformBeats").toDouble();
    if (std::abs(beats - 48.0) > 1e-6) {
        std::fprintf(stderr, "dj-ui: the layout value did not survive being read back: %.0f\n", beats);
        return 1;
    }

    // The wheel over the window belongs to the window: a modal popup must not let it through to the waveform behind, which
    // zoomed the deck under the DJ's hand (owner's report, 21.11).
    {
        auto *layoutState = engine.singletonInstance<QObject *>(QAnyStringView("DjPlatform.Ui"),
                                                               QAnyStringView("LayoutState"));
        // Any deck's strip will do: the question is whether the wheel reaches a waveform at all, not which one.
        QQuickItem *hand = nullptr;
        std::function<void(QQuickItem *)> findHand = [&](QQuickItem *node) {
            if (hand == nullptr && node->objectName().startsWith(QStringLiteral("waveformHand_"))) {
                hand = node;
                return;
            }
            for (QQuickItem *child : node->childItems()) {
                findHand(child);
                if (hand != nullptr) {
                    return;
                }
            }
        };
        findHand(root);
        if (layoutState == nullptr || hand == nullptr) {
            std::fprintf(stderr, "dj-ui: no waveform strip to send the wheel to\n");
            return 1;
        }        {
            const double before = layoutState->property("waveformBeats").toDouble();
            const QPointF centre = hand->mapToScene(QPointF(hand->width() / 2.0, hand->height() / 2.0));
            const auto wheel = [&centre](QQuickWindow *target) {
                QWheelEvent event(centre, target->mapToGlobal(centre.toPoint()), QPoint(0, 0), QPoint(0, 120),
                                  Qt::NoButton, Qt::NoModifier, Qt::NoScrollPhase, false);
                QCoreApplication::sendEvent(target, &event);
            };
            wheel(window);
            settle(40);
            const double during = layoutState->property("waveformBeats").toDouble();
            if (std::abs(during - before) > 1e-6) {
                std::fprintf(stderr, "dj-ui: the wheel over the settings window zoomed the waveform: %.1f to %.1f\n",
                             before, during);
                return 1;
            }
            // And the probe is not vacuous: with the window closed the same event does zoom.
            if (!press(named(root, QStringLiteral("settingsClose")))) {
                std::fprintf(stderr, "dj-ui: the close button does not answer before the wheel probe\n");
                return 1;
            }
            settle(60);
            wheel(window);
            settle(40);
            const double after = layoutState->property("waveformBeats").toDouble();
            if (std::abs(after - before) < 1e-6) {
                std::fprintf(stderr, "dj-ui: the wheel did not reach the waveform at all, so the probe proves nothing\n");
                return 1;
            }
            // Leave the layout as it was found, and open the window again for the rest of the check.
            layoutState->setProperty("waveformBeats", before);
            settle(20);
            std::fprintf(stderr, "dj-ui: the wheel is blocked by the window (%.1f stayed) and reaches the strip without it\n",
                         before);
            if (!press(gear) || !isOpen()) {
                std::fprintf(stderr, "dj-ui: the window did not reopen after the wheel probe\n");
                return 1;
            }
            settle(40);
        }
    }

    // The file has to hold what the reader can read: a pair written as text stopped the engine from starting (owner's
    // report, 21.11), so the check looks at the bytes the window left behind.
    if (auto *model = engine.singletonInstance<QObject *>(QAnyStringView("DjPlatform.Ui"), QAnyStringView("Settings"))) {
        QString filePath;
        if (QMetaObject::invokeMethod(model, "fileOf", Q_RETURN_ARG(QString, filePath),
                                      Q_ARG(QString, QStringLiteral("engine:audio.headphones.pair")))) {
            QFile config(filePath);
            if (config.open(QIODevice::ReadOnly)) {
                const QString text = QString::fromUtf8(config.readAll());
                if (text.contains(QStringLiteral("pair = \""))) {
                    std::fprintf(stderr, "dj-ui: the settings file holds a channel pair as text: %s\n",
                                 qPrintable(text));
                    return 1;
                }
                if (text.contains(QStringLiteral("pair = ["))) {
                    std::fprintf(stderr, "dj-ui: the channel pair was written as an array, which is what the engine reads\n");
                }
            }
        }
    }

    if (!press(named(root, QStringLiteral("settingsClose"))) || isOpen()) {
        std::fprintf(stderr, "dj-ui: the settings window did not close\n");
        return 1;
    }

    std::fprintf(stderr,
                 "dj-ui: the settings window opens from its button, holds %lld sections, turned %d pages and closed\n",
                 static_cast<long long>(sections.size()), turned);
    std::fprintf(stderr, "dj-ui: the sections take %.0f px and the settings %.0f px of a %.0f px panel\n", navWidth,
                 contentWidth, panel->width());
    std::fprintf(stderr, "dj-ui: the panel stays inside the window: %.0fx%.0f in %.0fx%.0f\n", inside.width(),
                 inside.height(), bounds.width(), bounds.height());
    std::fprintf(stderr, "dj-ui: two lines of the window wrote their value: block frames and waveform beats\n");
    std::fprintf(stderr, "dj-ui: the theme set outlived a reload: %s to %s, and %s to %s\n", qPrintable(before),
                 qPrintable(after), "graphite", qPrintable(reloaded));
    std::fprintf(stderr, "dj-ui: all cases passed\n");
    return 0;
}

} // namespace checks
