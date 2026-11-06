// A channel's level meter (DJP-UI-002): the engine's peak, drawn from the bottom up.
import QtQuick

Rectangle {
    id: meter

    required property string deckId
    required property real peak

    objectName: "meter_" + deckId
    color: UiSettings.themeBackground
    border.color: UiSettings.themeBorder
    border.width: 1

    Rectangle {
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        height: parent.height * Math.min(1.0, meter.peak)
        color: meter.peak > 0.95 ? UiSettings.themeDanger : UiSettings.themeAccent
    }
}
