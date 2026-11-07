// Crossfader (DJP-MIX-001).
//
// A plain slider fills its groove from the left, so a centred crossfader looks like one pushed against
// the left stop. This one is drawn as what it is: a track with a centre mark and a handle that sits
// where the value is — half way at 0.5 — so the picture and the sound agree.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Item {
    id: crossfader

    implicitHeight: 26

    readonly property real value: Bridge.snapshot.crossfader !== undefined
                                  ? Math.max(0.0, Math.min(1.0, Bridge.snapshot.crossfader)) : 0.5
    readonly property bool live: Bridge.connected
    readonly property int handleWidth: 20

    ColumnLayout {
        anchors.fill: parent
        spacing: 2

        Item {
            Layout.fillWidth: true
            Layout.fillHeight: true

            Rectangle {
                id: track
                anchors.verticalCenter: parent.verticalCenter
                anchors.left: parent.left
                anchors.right: parent.right
                height: 4
                radius: 2
                color: UiSettings.themeRaised
                border.color: UiSettings.themeBorder
                border.width: 1
            }

            // Where the two decks meet: the middle of the travel, marked so the rest position is
            // visible without moving the handle.
            Rectangle {
                anchors.horizontalCenter: parent.horizontalCenter
                anchors.verticalCenter: parent.verticalCenter
                width: 1
                height: 10
                color: UiSettings.themeBorder
            }

            Rectangle {
                id: handle
                objectName: "crossfaderHandle"
                width: crossfader.handleWidth
                height: parent.height
                radius: 2
                color: crossfader.live ? UiSettings.themeRaised : UiSettings.themePanel
                border.color: crossfader.live ? UiSettings.themeText : UiSettings.themeBorder
                border.width: 1
                x: (parent.width - width) * crossfader.value

                Rectangle {
                    anchors.centerIn: parent
                    width: 2
                    height: parent.height * 0.5
                    color: UiSettings.themeTextDim
                }
            }

            MouseArea {
                anchors.fill: parent
                enabled: crossfader.live
                onPositionChanged: function (mouse) { crossfader.apply(mouse.x) }
                onPressed: function (mouse) { crossfader.apply(mouse.x) }
                // A double click centres the crossfader; a notch moves it five percent of the throw.
                onDoubleClicked: crossfader.ask(0.5)
                onWheel: function (wheel) {
                    const notches = Math.max(-3, Math.min(3, Math.round(wheel.angleDelta.y / 120)));
                    if (notches !== 0) {
                        crossfader.ask(crossfader.value + notches * 0.05);
                    }
                }
            }
        }

        RowLayout {
            Layout.fillWidth: true
            Label {
                text: qsTr("A")
                font.pixelSize: 9
                color: crossfader.value < 0.5 ? UiSettings.themeText : UiSettings.themeTextDim
            }
            Item { Layout.fillWidth: true }
            Label {
                text: qsTr("B")
                font.pixelSize: 9
                color: crossfader.value > 0.5 ? UiSettings.themeText : UiSettings.themeTextDim
            }
        }
    }

    function apply(x) {
        const travel = Math.max(1, width - handleWidth);
        crossfader.ask((x - handleWidth / 2) / travel);
    }

    function ask(share) {
        Bridge.setCrossfader(Math.max(0.0, Math.min(1.0, share)));
    }
}
