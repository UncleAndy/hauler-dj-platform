// The settings window, checked the way a DJ would meet it: the button opens it, the sections are all there, choosing
// one turns the page, the panel stays inside the window, and the close button closes it.
//
// Geometry is part of the check: this shell is made for 1366×768, and a panel that reaches past the edge would be
// unusable on the screen the program is built for.

#include "Checks.h"

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

    QQuickItem *settingsWindow = named(root, QStringLiteral("settingsWindow"));
    if (settingsWindow == nullptr) {
        std::fprintf(stderr, "dj-ui: there is no settings window in the shell\n");
        return 1;
    }

    QQuickItem *gear = named(root, QStringLiteral("settingsButton"));
    if (gear == nullptr) {
        std::fprintf(stderr, "dj-ui: there is no button that opens the settings window\n");
        return 1;
    }
    if (!press(gear) || !settingsWindow->isVisible()) {
        std::fprintf(stderr, "dj-ui: the settings button did not open the window\n");
        return 1;
    }

    const QStringList sections = {
        QStringLiteral("sound"),    QStringLiteral("analysis"),    QStringLiteral("library"),
        QStringLiteral("mixer"),    QStringLiteral("effects"),     QStringLiteral("interface"),
        QStringLiteral("diagnostics"), QStringLiteral("about"),
    };
    for (const QString &key : sections) {
        if (named(root, QStringLiteral("settingsSection_") + key) == nullptr) {
            std::fprintf(stderr, "dj-ui: the settings window has no %s section\n", qPrintable(key));
            return 1;
        }
    }

    QQuickItem *panel = named(root, QStringLiteral("settingsPanel"));
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
    QQuickItem *nav = named(root, QStringLiteral("settingsNav"));
    QQuickItem *content = named(root, QStringLiteral("settingsContent"));
    if (nav == nullptr || content == nullptr) {
        std::fprintf(stderr, "dj-ui: the settings window has no list and no page to measure\n");
        return 1;
    }
    settle(60);
    const double navWidth = nav->width();
    const double contentWidth = content->width();
    if (navWidth > 220.0 || contentWidth < panel->width() * 0.55) {
        std::fprintf(stderr,
                     "dj-ui: the sections take %.0f px of a %.0f px panel, leaving %.0f px for the settings\n",
                     navWidth, panel->width(), contentWidth);
        return 1;
    }

    // The device is chosen from the outputs the engine names, not typed: a text field where a list belongs is what the
    // owner saw on his own machine (report, 21.11).
    {
        QQuickItem *deviceRow = named(root, QStringLiteral("settingRow_engine_audio_device"));
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

    // The headphone device is the same list, locked by the mode: only "device" lets the DJ name it, and a locked line
    // has to say whose choice it is.
    {
        QQuickItem *modeRow = named(root, QStringLiteral("settingRow_engine_audio_headphones_mode"));
        QQuickItem *headphoneRow = named(root, QStringLiteral("settingRow_engine_audio_headphones_device"));
        if (modeRow == nullptr || headphoneRow == nullptr) {
            std::fprintf(stderr, "dj-ui: the sound page has no headphone lines\n");
            return 1;
        }
        const QStringList devices = Settings_devices(engine);
        if (!devices.isEmpty()) {
            const QVariantList offered = headphoneRow->property("offered").toList();
            if (offered.size() < devices.size()) {
                std::fprintf(stderr, "dj-ui: the headphone line offers %lld of %lld outputs\n",
                             static_cast<long long>(offered.size()), static_cast<long long>(devices.size()));
                return 1;
            }
        }
        // Locked in "auto": the engine picks, and the line must not pretend otherwise.
        if (!QMetaObject::invokeMethod(modeRow, "write", Q_ARG(QVariant, QVariant(QStringLiteral("auto"))))) {
            std::fprintf(stderr, "dj-ui: the headphone mode line refused to change\n");
            return 1;
        }
        settle(40);
        if (headphoneRow->property("open").toBool()) {
            std::fprintf(stderr, "dj-ui: the headphone device is open while the mode is auto\n");
            return 1;
        }
        // Open in "device": the name is the DJ's to choose.
        if (!QMetaObject::invokeMethod(modeRow, "write", Q_ARG(QVariant, QVariant(QStringLiteral("device"))))) {
            std::fprintf(stderr, "dj-ui: the headphone mode line refused the device mode\n");
            return 1;
        }
        settle(40);
        if (!headphoneRow->property("open").toBool()) {
            std::fprintf(stderr, "dj-ui: the headphone device stays locked in the device mode\n");
            return 1;
        }
        if (!devices.isEmpty() && !headphoneRow->property("asList").toBool()) {
            std::fprintf(stderr, "dj-ui: the headphone device is not a list, though the engine named outputs\n");
            return 1;
        }
        // The channels are the pairs the chosen device really has: the engine is asked about that device, and the row
        // must offer exactly those numbers.
        QQuickItem *pairRow = named(root, QStringLiteral("settingRow_engine_audio_headphones_pair"));
        if (pairRow == nullptr) {
            std::fprintf(stderr, "dj-ui: the sound page has no headphone channel line\n");
            return 1;
        }
        const QString chosen = QString::fromUtf8(qgetenv("HAULER_CHECK_DEVICE"));
        const QStringList pairs = Settings_pairs(engine, chosen);
        if (!pairs.isEmpty()) {
            const QVariantList offered = pairRow->property("offered").toList();
            for (const QString &pair : pairs) {
                bool found = false;
                for (const QVariant &one : offered) {
                    if (one.toString() == pair) {
                        found = true;
                        break;
                    }
                }
                if (!found) {
                    std::fprintf(stderr, "dj-ui: the channel line does not offer %s, which the device has\n",
                                 qPrintable(pair));
                    return 1;
                }
            }
            std::fprintf(stderr, "dj-ui: the channel line offers the device's %lld pairs: %s\n",
                         static_cast<long long>(pairs.size()), qPrintable(pairs.join(QStringLiteral(", "))));
        }
        if (!pairRow->property("open").toBool()) {
            std::fprintf(stderr, "dj-ui: the channel line stays locked in the device mode\n");
            return 1;
        }

        // The main output is routed by the DJ in every mode, and both orders of a pair are offered: an interface whose
        // numbering is not the booth's needs `[1, 0]`, not a crossed cable.
        QQuickItem *mainRow = named(root, QStringLiteral("settingRow_engine_audio_master"));
        if (mainRow == nullptr) {
            std::fprintf(stderr, "dj-ui: the sound page has no main channel line\n");
            return 1;
        }
        if (!mainRow->property("open").toBool()) {
            std::fprintf(stderr, "dj-ui: the main channel line is locked\n");
            return 1;
        }
        const QVariantList mainOffered = mainRow->property("offered").toList();
        bool forward = false;
        bool swapped = false;
        for (const QVariant &one : mainOffered) {
            const QString text = one.toString();
            if (text == QStringLiteral("[0, 1]")) {
                forward = true;
            }
            if (text == QStringLiteral("[1, 0]")) {
                swapped = true;
            }
        }
        if (!forward) {
            std::fprintf(stderr, "dj-ui: the main channel line does not offer [0, 1]\n");
            return 1;
        }
        if (!pairs.isEmpty() && !swapped) {
            std::fprintf(stderr, "dj-ui: the main channel line does not offer the swapped order [1, 0]\n");
            return 1;
        }
        std::fprintf(stderr, "dj-ui: the main channels offer %lld routes, the swapped order among them\n",
                     static_cast<long long>(mainOffered.size()));

        std::fprintf(stderr, "dj-ui: the headphone device is locked in auto and a list in device mode\n");
    }

    // Every line shows a control. A line whose list is empty and whose field is hidden is a line the DJ cannot see at
    // all, and that is what "the setting is not in the interface" looks like from the outside (owner's report, 21.11).
    {
        int without = 0;
        std::function<void(QQuickItem *)> walk = [&](QQuickItem *node) {
            if (node->objectName().startsWith(QStringLiteral("settingRow_"))) {
                const QString kind = node->property("kind").toString();
                const bool shows = node->property("asList").toBool() || node->property("asField").toBool()
                                   || kind == QStringLiteral("switch") || kind == QStringLiteral("slider")
                                   || kind == QStringLiteral("number") || kind == QStringLiteral("check");
                if (!shows) {
                    std::fprintf(stderr, "dj-ui: the %s line shows nothing to change\n",
                                 qPrintable(node->objectName()));
                    ++without;
                }
            }
            for (QQuickItem *child : node->childItems()) {
                walk(child);
            }
        };
        walk(settingsWindow);
        if (without > 0) {
            return 1;
        }
    }

    // Every list in the window has something in it. An empty dropdown is the same defect as a text field where a list
    // belongs: the theme line was offering nothing at all until the owner's request put a list there (21.11).
    {
        int checked = 0;
        std::function<void(QQuickItem *)> sweep = [&](QQuickItem *node) {
            const QString name = node->objectName();
            if (name.startsWith(QStringLiteral("settingRow_"))) {
                const QString kind = node->property("kind").toString();
                if (kind == QStringLiteral("choice") || kind == QStringLiteral("device")
                    || kind == QStringLiteral("pair")) {
                    const int offered = node->property("offered").toList().size();
                    if (offered == 0) {
                        std::fprintf(stderr, "dj-ui: the %s line offers nothing to choose from\n", qPrintable(name));
                        checked = -1;
                        return;
                    }
                    ++checked;
                }
            }
            for (QQuickItem *child : node->childItems()) {
                sweep(child);
                if (checked < 0) {
                    return;
                }
            }
        };
        sweep(settingsWindow);
        if (checked < 0) {
            return 1;
        }
        std::fprintf(stderr, "dj-ui: every list in the window offers something: %d of them\n", checked);
    }

    // A page that exists but shows nothing is the same complaint as a list that takes the window: the rows have to be
    // wide enough to read a name and reach a control (owner's report, 21.11).
    if (press(named(root, QStringLiteral("settingsSection_interface")))) {
        settle(60);
        QQuickItem *row = named(root, QStringLiteral("settingRow_layout_waveformBeats"));
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
    QQuickItem *pages = named(root, QStringLiteral("settingsPages"));
    if (pages == nullptr) {
        std::fprintf(stderr, "dj-ui: the settings window has no pages\n");
        return 1;
    }
    int turned = 0;
    for (int index = 1; index < sections.size(); ++index) {
        QQuickItem *button = named(root, QStringLiteral("settingsSection_") + sections.at(index));
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

    // The theme is a whole palette at a time, and the promise is that it outlives the window: it is written through
    // its own line, saved into layout.json with the arrangement, and read back by the loader that starts the shell.
    QQuickItem *themeRow = named(root, QStringLiteral("settingRow_ui_themeSet"));
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

    if (!press(named(root, QStringLiteral("settingsClose"))) || settingsWindow->isVisible()) {
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
