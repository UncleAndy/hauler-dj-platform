// The settings window, checked the way a DJ would meet it: the button opens it, the sections are all there, choosing
// one turns the page, the panel stays inside the window, and the close button closes it.
//
// Geometry is part of the check: this shell is made for 1366×768, and a panel that reaches past the edge would be
// unusable on the screen the program is built for.

#include "Checks.h"

#include <QQmlApplicationEngine>
#include <QRectF>
#include <QQuickWindow>

#include <cmath>
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
    std::fprintf(stderr, "dj-ui: the panel stays inside the window: %.0fx%.0f in %.0fx%.0f\n", inside.width(),
                 inside.height(), bounds.width(), bounds.height());
    std::fprintf(stderr, "dj-ui: two lines of the window wrote their value: block frames and waveform beats\n");
    std::fprintf(stderr, "dj-ui: the theme set outlived a reload: %s to %s, and %s to %s\n", qPrintable(before),
                 qPrintable(after), "graphite", qPrintable(reloaded));
    std::fprintf(stderr, "dj-ui: all cases passed\n");
    return 0;
}

} // namespace checks
