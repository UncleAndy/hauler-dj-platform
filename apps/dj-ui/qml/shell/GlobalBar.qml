// The strip above the decks: what belongs to the whole set (DJP-UI-001).
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Pane {
    id: bar

    background: Rectangle { color: UiSettings.themeBackground }

    RowLayout {
        anchors.fill: parent
        anchors.leftMargin: 8
        anchors.rightMargin: 8
        spacing: 12

        Label {
            text: qsTr("DJ Platform")
            font.bold: true
            color: UiSettings.themeText
        }
        Label {
            text: qsTr("layout %1").arg(LayoutState.preset)
            color: UiSettings.themeTextDim
            font.pixelSize: 10
        }
        Item { Layout.fillWidth: true }
        Label {
            text: Bridge.status
            color: Bridge.connected ? UiSettings.themeText : UiSettings.themeDanger
        }
        Label {
            text: Bridge.connected
                  ? qsTr("rev %1").arg(Bridge.snapshot.revision !== undefined
                                        ? Bridge.snapshot.revision : 0)
                  : qsTr("no engine")
            color: UiSettings.themeTextDim
            font.pixelSize: 10
        }
    }
}
