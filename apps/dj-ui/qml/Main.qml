// Root window: two decks and a crossfader, all driven by the engine's published state.
// The shell owns no audio and no domain logic (DJP-ARCH-001, DJP-UI-001, ADR-0003).
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

import "shell"

ApplicationWindow {
    id: root
    width: 1366
    height: 768
    minimumWidth: 1366
    minimumHeight: 768
    visible: true
    title: qsTr("DJ Platform — %1").arg(Bridge.connected ? qsTr("engine connected")
                                                        : qsTr("engine offline"))
    color: UiSettings.themeBackground

    // Controls take their colours from the window palette; setting it here propagates to every
    // Pane, Button and Label in the shell. The values are the measured Traktor scheme: neutral dark
    // greys and one blue accent, so nothing glares in a dark booth.
    palette.window: UiSettings.themeSurface
    palette.windowText: UiSettings.themeText
    palette.base: UiSettings.themeBackground
    palette.alternateBase: UiSettings.themePanel
    palette.text: UiSettings.themeText
    palette.button: UiSettings.themeRaised
    palette.buttonText: UiSettings.themeText
    palette.brightText: UiSettings.themePlayhead
    palette.highlight: UiSettings.themeAccent
    palette.highlightedText: UiSettings.themeAccentDim
    palette.toolTipBase: UiSettings.themePanel
    palette.toolTipText: UiSettings.themeText
    palette.placeholderText: UiSettings.themeTextDim

    // The arrangement lives in ShellLayout: the proportions are settings, the splitters write them
    // back, and this window only owns the palette and the size.
    ShellLayout {
        anchors.fill: parent
        anchors.margins: 6
    }
}
