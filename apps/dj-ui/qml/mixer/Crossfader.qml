// Crossfader placeholder. The real control sends SetCrossfader commands and
// renders the engine snapshot; it must not own the value (DJP-MIX-001).
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Pane {
    Layout.preferredHeight: 72

    RowLayout {
        anchors.fill: parent
        spacing: 12

        Label { text: qsTr("A") }
        Slider {
            id: fader
            Layout.fillWidth: true
            from: 0.0
            to: 1.0
            value: 0.5
            enabled: false
        }
        Label { text: qsTr("B") }
    }
}
