// The settings window, checked the way a DJ would meet it: the button opens it, the sections are all there, choosing
// one turns the page, the panel stays inside the window, and the close button closes it.
//
// Geometry is part of the check: this shell is made for 1366×768, and a panel that reaches past the edge would be
// unusable on the screen the program is built for.

#include "Checks.h"

#include <QDir>
#include <QEventLoop>
#include <QQmlApplicationEngine>
#include <QSet>
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

/// The value the window would show for a path, asked the way the window asks.
double Settings_value(QQmlApplicationEngine &engine, const QString &path)
{
    auto *model = engine.singletonInstance<QObject *>(QAnyStringView("DjPlatform.Ui"), QAnyStringView("Settings"));
    if (model == nullptr) {
        return 0.0;
    }
    QVariant answer;
    if (!QMetaObject::invokeMethod(model, "value", Q_RETURN_ARG(QVariant, answer), Q_ARG(QString, path))) {
        return 0.0;
    }
    return answer.toDouble();
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
    // Every line of the window has to say what it is for: the hint is what the DJ reads when the pointer rests on the
    // label, and a line without one is a word nobody can look up. Collected while the pages turn, because a page's rows
    // exist only while that page is on screen.
    QSet<QString> seenRows;
    QStringList withoutHint;
    const auto collectHints = [&]() {
        const std::function<void(QQuickItem *)> walk = [&](QQuickItem *node) {
            const QString name = node->objectName();
            if (name.startsWith(QStringLiteral("settingRow_")) && !seenRows.contains(name)) {
                seenRows.insert(name);
                if (node->property("hint").toString().trimmed().isEmpty()) {
                    withoutHint << name;
                }
            }
            for (QQuickItem *child : node->childItems()) {
                walk(child);
            }
        };
        walk(content);
    };
    collectHints();
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
        settle(40);
        collectHints();

    }
    if (!withoutHint.isEmpty()) {
        std::fprintf(stderr, "dj-ui: %lld lines of the window have no description: %s\n",
                     static_cast<long long>(withoutHint.size()),
                     qPrintable(withoutHint.join(QStringLiteral(", "))));
        return 1;
    }
    // A slider whose ends do not match its value is a slider that writes the wrong number: the percent rows were 0..1
    // while the layout stores 3..100, so dragging changed the interface to one clamped value (owner's report, 22.11).
    // Writing a quarter of the travel and reading it back catches the whole class, not just those three.
    QStringList slides;
    const std::function<void(QQuickItem *)> walkSliders = [&](QQuickItem *node) {
        if (node->property("kind").toString() == QStringLiteral("slider")) {
            slides << node->objectName();
        }
        for (QQuickItem *child : node->childItems()) {
            walkSliders(child);
        }
    };
    walkSliders(content);
    for (const QString &name : slides) {
        QQuickItem *sliderRow = findInWindow(name);
        if (sliderRow == nullptr) {
            continue;
        }
        const QString path = sliderRow->property("path").toString();
        const double low = sliderRow->property("from").toDouble();
        const double high = sliderRow->property("to").toDouble();
        const double wanted = low + (high - low) * 0.25;
        const double before = Settings_value(engine, path);
        if (!QMetaObject::invokeMethod(sliderRow, "write", Q_ARG(QVariant, QVariant(wanted)))) {
            std::fprintf(stderr, "dj-ui: the %s slider refused its own value\n", qPrintable(name));
            return 1;
        }
        const double stored = Settings_value(engine, path);
        if (std::abs(stored - wanted) > 0.6) {
            std::fprintf(stderr, "dj-ui: the %s slider spans %.2f..%.2f, wrote %.2f and the file holds %.2f\n",
                         qPrintable(name), low, high, wanted, stored);
            return 1;
        }
        // Put back what the file had: a check must not leave the window changed.
        QMetaObject::invokeMethod(sliderRow, "write", Q_ARG(QVariant, QVariant(before)));
    }
    std::fprintf(stderr, "dj-ui: %lld sliders reach the values they write\n", static_cast<long long>(slides.size()));

    if (seenRows.size() < 30) {
        std::fprintf(stderr, "dj-ui: only %lld lines were seen while the pages turned — the hint check proves little\n",
                     static_cast<long long>(seenRows.size()));
        return 1;
    }
    std::fprintf(stderr, "dj-ui: all %lld lines of the window describe themselves in a tooltip\n",
                 static_cast<long long>(seenRows.size()));


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
    // The table has to be a table the eye can use: tall enough for four rows even when the collection is empty, and
    // nothing drawn over it (owner's report, 22.11: the list was too low and the lines below sat on top of it).
    QQuickItem *table = findInWindow(QStringLiteral("folderTable"));
    QQuickItem *addButton = findInWindow(QStringLiteral("folderAdd"));
    QQuickItem *removeButton = findInWindow(QStringLiteral("folderRemove"));
    if (table == nullptr || addButton == nullptr || removeButton == nullptr) {
        std::fprintf(stderr, "dj-ui: the collection row has no table or no buttons\n");
        return 1;
    }
    // Four rows of 20 px and the frame's own border: the frame must end under the fourth row, not hang below it
    // (owner's report, 22.11).
    const double tableHeight = table->height();
    if (tableHeight < 4 * 20.0 || tableHeight > 4 * 20.0 + 4) {
        std::fprintf(stderr, "dj-ui: the collection table is %.0f px tall, where four rows and their border are 82\n",
                     tableHeight);
        return 1;
    }
    // The block itself is one framed thing: its frame has to hold the label, the table, the field and the buttons.
    QQuickItem *block = findInWindow(QStringLiteral("folderBlock"));
    if (block == nullptr) {
        std::fprintf(stderr, "dj-ui: the collection folders have no block of their own\n");
        return 1;
    }
    const QRectF blockRect(block->mapToScene(QPointF(0, 0)), block->size());
    for (QQuickItem *part : {table, addButton, removeButton}) {
        const QRectF partRect(part->mapToScene(QPointF(0, 0)), part->size());
        if (!blockRect.contains(partRect)) {
            std::fprintf(stderr,
                         "dj-ui: %s at x %.0f..%.0f y %.0f..%.0f is outside the collection block "
                         "x %.0f..%.0f y %.0f..%.0f\n",
                         qPrintable(part->objectName()), partRect.left(), partRect.right(), partRect.top(),
                         partRect.bottom(), blockRect.left(), blockRect.right(), blockRect.top(), blockRect.bottom());
            return 1;
        }
    }
    // The buttons live in one row under the path field: Add first, Remove to its right (owner's order, 22.11).
    const QRectF addRect(addButton->mapToScene(QPointF(0, 0)), addButton->size());
    const QRectF removeRect(removeButton->mapToScene(QPointF(0, 0)), removeButton->size());
    QQuickItem *fieldForButtons = findInWindow(QStringLiteral("folderAddField"));
    if (fieldForButtons == nullptr) {
        std::fprintf(stderr, "dj-ui: the collection row has no path field\n");
        return 1;
    }
    const QRectF fieldAbove(fieldForButtons->mapToScene(QPointF(0, 0)), fieldForButtons->size());
    if (addRect.top() < fieldAbove.bottom()) {
        std::fprintf(stderr, "dj-ui: Add at y %.0f is not under the path field, which ends at %.0f\n", addRect.top(),
                     fieldAbove.bottom());
        return 1;
    }
    if (std::abs(addRect.top() - removeRect.top()) > 2.0) {
        std::fprintf(stderr, "dj-ui: Add and Remove are not on one line: y %.0f and %.0f\n", addRect.top(),
                     removeRect.top());
        return 1;
    }
    if (addRect.left() >= removeRect.left()) {
        std::fprintf(stderr, "dj-ui: Add at x %.0f is not first (Remove at %.0f)\n", addRect.left(),
                     removeRect.left());
        return 1;
    }
    const QRectF rowRect(foldersRow->mapToScene(QPointF(0, 0)), foldersRow->size());
    const QRectF tableRect(table->mapToScene(QPointF(0, 0)), table->size());
    if (!rowRect.contains(tableRect)) {
        std::fprintf(stderr, "dj-ui: the collection table sticks out of its own line (%.0f x %.0f in %.0f x %.0f)\n",
                     tableRect.width(), tableRect.height(), rowRect.width(), rowRect.height());
        return 1;
    }
    // A button the DJ cannot reach is not a button: both have to lie inside the panel, not past its right edge
    // (owner's report, 22.11: Add and Remove were outside the window).
    const QRectF panelRect(panel->mapToScene(QPointF(0, 0)), panel->size());
    for (QQuickItem *button : {addButton, removeButton}) {
        const QRectF buttonRect(button->mapToScene(QPointF(0, 0)), button->size());
        if (!panelRect.contains(buttonRect)) {
            std::fprintf(stderr,
                         "dj-ui: the %s button is outside the panel: x %.0f..%.0f where the panel ends at %.0f\n",
                         button == addButton ? "Add" : "Remove", buttonRect.left(), buttonRect.right(),
                         panelRect.right());
            return 1;
        }
    }
    // The page's own width, not the panel's: the sections on the left take a share of the panel, and a block sized
    // against the panel fits the panel and still leaves the page (owner's report, 22.11).
    QQuickItem *pageFlick = nullptr;
    for (QQuickItem *pageParent = foldersRow->parentItem(); pageParent != nullptr; pageParent = pageParent->parentItem()) {
        if (pageParent->inherits("QQuickFlickable")) {
            pageFlick = pageParent;
            break;
        }
    }
    const QRectF pageRect(pageFlick != nullptr ? pageFlick->mapToScene(QPointF(0, 0)) : QPointF(0, 0),
                          pageFlick != nullptr ? pageFlick->size() : panel->size());
    for (QQuickItem *inside : {table, addButton, removeButton, findInWindow(QStringLiteral("folderAddField")),
                               findInWindow(QStringLiteral("folderBrowse"))}) {
        if (inside == nullptr) {
            std::fprintf(stderr, "dj-ui: a part of the collection row is missing\n");
            return 1;
        }
        const QRectF rect(inside->mapToScene(QPointF(0, 0)), inside->size());
        // Inside the page, and no further: the block spans the row, so its right edge sits a margin away from the
        // page's edge — the same margin it keeps on the left.
        if (rect.right() > pageRect.right() - 8 || rect.left() < pageRect.left() - 0.5) {
            std::fprintf(stderr,
                         "dj-ui: %s spans x %.0f..%.0f where the page ends at %.0f (page %.0f wide)\n",
                         qPrintable(inside->objectName()), rect.left(), rect.right(), pageRect.right(),
                         pageRect.width());
            return 1;
        }
    }
    // The path field carries a button for picking a folder, and it sits to the right of the field. The dialog itself is
    // not opened here: a native dialog in a headless run waits for a hand that is not there. What is checked is that the
    // button is where it belongs and that the field still only fills — the list changes when Add is pressed, not before.
    QQuickItem *pathField = findInWindow(QStringLiteral("folderAddField"));
    QQuickItem *browseButton = findInWindow(QStringLiteral("folderBrowse"));
    if (pathField == nullptr || browseButton == nullptr) {
        std::fprintf(stderr, "dj-ui: the collection row has no path field or no browse button\n");
        return 1;
    }
    const QRectF fieldRect(pathField->mapToScene(QPointF(0, 0)), pathField->size());
    const QRectF browseRect(browseButton->mapToScene(QPointF(0, 0)), browseButton->size());
    if (browseRect.left() < fieldRect.right() - 1) {
        std::fprintf(stderr, "dj-ui: the browse button sits at x %.0f inside the field (field ends at %.0f)\n",
                     browseRect.left(), fieldRect.right());
        return 1;
    }
    const auto beforeBrowse = foldersRow->property("folders").toList().size();
    if (!press(browseButton)) {
        std::fprintf(stderr, "dj-ui: the browse button does not answer a click\n");
        return 1;
    }
    settle(40);
    if (foldersRow->property("folders").toList().size() != beforeBrowse) {
        std::fprintf(stderr, "dj-ui: pressing the browse button added a folder without Add\n");
        return 1;
    }
    QQuickItem *nextRow = findInWindow(QStringLiteral("settingRow_engine_library_history_min_seconds"));
    if (nextRow != nullptr) {
        const QRectF nextRect(nextRow->mapToScene(QPointF(0, 0)), nextRow->size());
        if (rowRect.intersects(nextRect)) {
            std::fprintf(stderr, "dj-ui: the collection line (bottom %.0f) runs into the next one (top %.0f)\n",
                         rowRect.bottom(), nextRect.top());
            return 1;
        }
    }
    std::fprintf(stderr,
                 "dj-ui: the collection folders are a table %.0f px tall, with the path field and a row of Add/Remove "
                 "under it, nothing drawn over anything: %lld shown, one removed, the rest kept in the file "
                 "(page %.0f px, table x %.0f..%.0f, panel right %.0f)\n",
                 tableHeight, static_cast<long long>(shownFolders.size()), pageRect.width(),
                 tableRect.left(), tableRect.right(), panelRect.right());

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
