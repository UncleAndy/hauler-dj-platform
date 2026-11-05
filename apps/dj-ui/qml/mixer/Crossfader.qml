// Crossfader (DJP-MIX-001). The knob does not own the value: it shows what the engine last
// published and sends SetCrossfader while the user drags. If the engine refuses or the link drops,
// the handle springs back to the published position on the next snapshot.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Pane {
    id: panel
    Layout.preferredHeight: 72

    readonly property bool live: Bridge.connected
    readonly property real published: Bridge.snapshot.crossfader !== undefined
                                       ? Bridge.snapshot.crossfader : 0.5

    RowLayout {
        anchors.fill: parent
        spacing: 12

        Label { text: qsTr("A") }
        Slider {
            id: fader
            Layout.fillWidth: true
            from: 0.0
            to: 1.0
            // Engine state is the truth; the local value only leads while the user is dragging.
            value: panel.published
            enabled: panel.live
            onMoved: Bridge.setCrossfader(value)
        }
        Label { text: qsTr("B") }
    }
}
