// Mixer: one strip per deck and the crossfader between them (DJP-MIX-001).
//
// The column is the layout's centre: it sits between the two decks of each row and is drawn once, so
// the strips line up with the decks they belong to however the window is resized.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Pane {
    id: mixer

    readonly property var decks: Bridge.snapshot.decks !== undefined ? Bridge.snapshot.decks : []
    readonly property var crossfader: Bridge.snapshot.crossfader !== undefined
                                      ? Bridge.snapshot.crossfader : 0.5

    function deckOf(id) {
        for (let index = 0; index < mixer.decks.length; ++index) {
            if (mixer.decks[index].deck === id) {
                return mixer.decks[index];
            }
        }
        return null;
    }

    background: Rectangle {
        color: UiSettings.themePanel
        border.color: UiSettings.themeBorder
        border.width: 1
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 6
        spacing: 6

        // Crossfader at the top: on the reference it sits above the channel strips, between the two
        // decks, where the hand that is not on a deck can reach it.
        ColumnLayout {
            Layout.fillWidth: true
            spacing: 2
            Label {
                text: qsTr("Crossfader")
                color: UiSettings.themeTextDim
                font.pixelSize: 10
            }
            RowLayout {
                Layout.fillWidth: true
                spacing: 6
                Label { text: qsTr("A"); color: UiSettings.themeText }
                Slider {
                    Layout.fillWidth: true
                    from: 0.0
                    to: 1.0
                    value: mixer.crossfader
                    enabled: Bridge.connected
                    onMoved: Bridge.setCrossfader(value)
                }
                Label { text: qsTr("B"); color: UiSettings.themeText }
            }
        }

        // Channel strips: level from the engine, gain sent back to it.
        RowLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            spacing: 4

            Repeater {
                model: ["A", "B", "C", "D"]
                delegate: ColumnLayout {
                    id: strip
                    required property string modelData
                    readonly property var state: mixer.deckOf(strip.modelData)
                    readonly property real peak: state !== null && state.levels !== undefined
                                                 ? Math.max(state.levels.peak_left,
                                                            state.levels.peak_right) : 0.0

                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    spacing: 2

                    Label {
                        text: strip.modelData
                        color: UiSettings.themeTextDim
                        font.pixelSize: 10
                        Layout.alignment: Qt.AlignHCenter
                    }
                    // The meter is the engine's peak, drawn from the strip's bottom up.
                    Rectangle {
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        color: UiSettings.themeBackground
                        border.color: UiSettings.themeBorder
                        border.width: 1
                        Rectangle {
                            anchors.left: parent.left
                            anchors.right: parent.right
                            anchors.bottom: parent.bottom
                            height: parent.height * Math.min(1.0, strip.peak)
                            color: strip.peak > 0.95 ? UiSettings.themeDanger : UiSettings.themeAccent
                        }
                    }
                    Slider {
                        Layout.fillWidth: true
                        from: 0.0
                        to: 1.5
                        value: strip.state !== null && strip.state.linear_gain !== undefined
                               ? strip.state.linear_gain : 1.0
                        enabled: Bridge.connected
                        onMoved: Bridge.setChannelGain(strip.modelData, value)
                    }
                }
            }
        }
    }
}
