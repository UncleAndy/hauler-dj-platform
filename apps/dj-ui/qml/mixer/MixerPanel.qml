// Mixer: one strip per deck and the crossfader below them (DJP-MIX-001).
//
// A strip is a gain knob beside a level meter, both as tall as the column: the meter shows what the
// engine reports, the knob sets the deck's own trim and reads back the same number the snapshot
// carries, so it cannot drift from the sound.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Pane {
    id: mixer

    readonly property var decks: Bridge.snapshot.decks !== undefined ? Bridge.snapshot.decks : []
    // The order is a setting (LayoutState.channelOrder); C first by default, as the owner asked — the
    // decks that are not the two outer ones still deserve a reachable strip.
    readonly property var order: LayoutState.channelOrder.split("")
    // The travel of a channel knob, in decibels. 0 dB is unity, the top is a little headroom.
    readonly property int minimumDb: -40
    readonly property int maximumDb: 6

    function deckOf(id) {
        for (let index = 0; index < mixer.decks.length; ++index) {
            if (mixer.decks[index].deck === id) {
                return mixer.decks[index];
            }
        }
        return null;
    }

    function toDb(gain) {
        return gain > 0.0 ? 20.0 * Math.log(gain) / Math.LN10 : mixer.minimumDb;
    }
    function toGain(db) {
        return Math.pow(10.0, db / 20.0);
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

        RowLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            spacing: 4

            Repeater {
                model: mixer.order
                delegate: ColumnLayout {
                    id: strip
                    required property string modelData
                    objectName: "strip_" + modelData
                    readonly property var state: mixer.deckOf(strip.modelData)
                    readonly property real peak: strip.state !== null && strip.state.levels !== undefined
                                                 ? Math.max(strip.state.levels.peak_left,
                                                            strip.state.levels.peak_right) : 0.0
                    readonly property real gain: strip.state !== null
                                                 && strip.state.linear_gain !== undefined
                                                 ? strip.state.linear_gain : 1.0

                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    spacing: 2

                    Label {
                        text: strip.modelData
                        color: UiSettings.themeTextDim
                        font.pixelSize: 10
                        Layout.alignment: Qt.AlignHCenter
                    }

                    // Knob and meter side by side, both full height.
                    RowLayout {
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        spacing: 4

                        // The gain knob: vertical travel in decibels, dragging anywhere in the track.
                        Item {
                            id: knob
                            objectName: "knob_" + strip.modelData
                            Layout.preferredWidth: 22
                            Layout.fillHeight: true

                            Rectangle {
                                anchors.fill: parent
                                radius: 2
                                color: UiSettings.themeBackground
                                border.color: UiSettings.themeBorder
                                border.width: 1
                            }
                            Rectangle {
                                anchors.left: parent.left
                                anchors.right: parent.right
                                anchors.bottom: parent.bottom
                                height: parent.height * strip.fraction
                                color: UiSettings.themeAccentDim
                            }
                            // Unity: where the knob lives when nothing has been touched.
                            Rectangle {
                                anchors.left: parent.left
                                anchors.right: parent.right
                                y: parent.height * (1.0 - strip.unityFraction)
                                height: 1
                                color: UiSettings.themeBorder
                            }
                            Rectangle {
                                width: parent.width
                                height: 8
                                radius: 1
                                y: parent.height * (1.0 - strip.fraction) - height / 2
                                color: strip.state !== null ? UiSettings.themeRaised
                                                            : UiSettings.themePanel
                                border.color: UiSettings.themeTextDim
                                border.width: 1
                            }
                            MouseArea {
                                anchors.fill: parent
                                enabled: strip.state !== null && Bridge.connected
                                onPositionChanged: function (mouse) { knob.set(mouse.y) }
                                onPressed: function (mouse) { knob.set(mouse.y) }
                            }

                            function set(y) {
                                const share = 1.0 - Math.max(0.0, Math.min(1.0, y / height));
                                const db = mixer.minimumDb + share * (mixer.maximumDb - mixer.minimumDb);
                                Bridge.setChannelGain(strip.modelData, mixer.toGain(db));
                            }
                        }

                        // The meter: the engine's peak, from the strip's bottom up.
                        Rectangle {
                            objectName: "meter_" + strip.modelData
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
                                color: strip.peak > 0.95 ? UiSettings.themeDanger
                                                         : UiSettings.themeAccent
                            }
                        }
                    }

                    Label {
                        text: strip.gain > 0.0
                              ? (mixer.toDb(strip.gain) >= 0 ? "+" : "") + mixer.toDb(strip.gain).toFixed(1)
                              : "-∞"
                        color: UiSettings.themeTextDim
                        font.pixelSize: 9
                        Layout.alignment: Qt.AlignHCenter
                    }

                    readonly property real fraction: Math.max(0.0, Math.min(1.0,
                        (mixer.toDb(strip.gain) - mixer.minimumDb)
                        / (mixer.maximumDb - mixer.minimumDb)))
                    readonly property real unityFraction: (0.0 - mixer.minimumDb)
                                                         / (mixer.maximumDb - mixer.minimumDb)
                }
            }
        }

        // The crossfader sits at the bottom of the column, under the strips, where the hand rests.
        Crossfader {
            Layout.fillWidth: true
            Layout.preferredHeight: 30
        }
    }
}
