// Placeholder deck: transport buttons exist so the layout is real, but they are
// local UI state only. A06 replaces them with acknowledged engine snapshots
// (DJP-ARCH-004) — a button must never confirm a command the engine rejected.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Pane {
    id: panel
    required property string deckLabel

    Label {
        anchors.centerIn: parent
        text: panel.deckLabel
        font.pixelSize: 28
        opacity: 0.35
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 8

        RowLayout {
            Layout.alignment: Qt.AlignHCenter
            Button { text: qsTr("Play"); enabled: false }
            Button { text: qsTr("Cue"); enabled: false }
            Button { text: qsTr("Loop"); enabled: false }
        }

        Item { Layout.fillHeight: true }

        Label {
            Layout.alignment: Qt.AlignHCenter
            text: qsTr("no track loaded")
            opacity: 0.5
        }
    }
}
