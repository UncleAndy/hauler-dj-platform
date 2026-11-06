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

    // The wheel trims the channel over the meter as well as over the knob.
    signal scrolled(int notches)

    MouseArea {
        anchors.fill: parent
        onWheel: function (wheel) {
            const notches = Math.max(-3, Math.min(3, Math.round(wheel.angleDelta.y / 120)));
            if (notches !== 0) {
                meter.scrolled(notches);
            }
        }
    }
}
