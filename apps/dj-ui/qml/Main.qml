// Root window of the M0 scaffold: two track decks and a crossfader, no engine yet.
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
    title: qsTr("DJ Platform — M0 shell")
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
                // Honest about the state of the build: nothing is wired to audio yet.
                text: qsTr("M0 scaffold — no audio engine connected")
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
                deckLabel: qsTr("Deck A")
            }
            DeckPanel {
                Layout.fillWidth: true
                Layout.fillHeight: true
                deckLabel: qsTr("Deck B")
            }
        }

        Crossfader {
            Layout.fillWidth: true
        }
    }
}
