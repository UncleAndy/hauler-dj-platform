// Root window: two decks and a crossfader, all driven by the engine's published state.
// The shell owns no audio and no domain logic (DJP-ARCH-001, DJP-UI-001, ADR-0003).
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

import "decks"
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
    color: "#14161a"

    header: ToolBar {
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
                color: Bridge.connected ? "#9be29b" : "#e2a09b"
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
