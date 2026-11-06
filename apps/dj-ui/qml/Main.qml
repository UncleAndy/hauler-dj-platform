// Root window: two decks and a crossfader, all driven by the engine's published state.
// The shell owns no audio and no domain logic (DJP-ARCH-001, DJP-UI-001, ADR-0003).
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

import "decks"
import "." as Shell
import "mixer"

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

    header: ToolBar {
        // The title bar sits a step darker than the surfaces below it, as in the reference: the
        // frame recedes and the working area comes forward.
        background: Rectangle { color: UiSettings.themeBackground }
        RowLayout {
            anchors.fill: parent
            Label {
                text: qsTr("DJ Platform")
                font.bold: true
            }
            Item { Layout.fillWidth: true }
            Label {
                // The link state and the engine's own words, not a guess.
                text: Bridge.status
                color: Bridge.connected ? UiSettings.themeText : UiSettings.themeDanger
            }
            Label {
                text: Bridge.connected
                      ? qsTr("rev %1").arg(Bridge.snapshot.revision !== undefined
                                           ? Bridge.snapshot.revision : 0)
                      : qsTr("no engine")
                opacity: 0.6
            }
        }
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 8
        spacing: 8

        RowLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            spacing: 8

            DeckPanel {
                Layout.fillWidth: true
                Layout.fillHeight: true
                deckId: "A"
                deckLabel: qsTr("Deck A")
            }
            DeckPanel {
                Layout.fillWidth: true
                Layout.fillHeight: true
                deckId: "B"
                deckLabel: qsTr("Deck B")
            }
        }

        Crossfader {
            Layout.fillWidth: true
        }
    }
}
