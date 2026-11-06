// Mixer: one strip per deck and the crossfader below them (DJP-MIX-001).
//
// A strip is a gain knob beside a level meter, both as tall as the column. The knob sits on the inner
// side: the left-hand decks (A and C) keep theirs on the left, the right-hand ones (B and D) on the
// right, so the two halves of the mixer mirror each other instead of every knob crowding the middle.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Pane {
    id: mixer
    objectName: "mixerPanel"

    readonly property var decks: Bridge.snapshot.decks !== undefined ? Bridge.snapshot.decks : []
    // The order is a setting (LayoutState.channelOrder); C first by default, as the owner asked.
    readonly property var order: LayoutState.channelOrder.split("")

    function deckOf(id) {
        for (let index = 0; index < mixer.decks.length; ++index) {
            if (mixer.decks[index].deck === id) {
                return mixer.decks[index];
            }
        }
        return null;
    }
    function toDb(gain) {
        return gain > 0.0 ? 20.0 * Math.log(gain) / Math.LN10 : -40;
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
                    readonly property var deck: mixer.deckOf(strip.modelData)
                    readonly property real peak: strip.deck !== null && strip.deck.levels !== undefined
                                                 ? Math.max(strip.deck.levels.peak_left,
                                                            strip.deck.levels.peak_right) : 0.0
                    readonly property real gain: strip.deck !== null
                                                 && strip.deck.linear_gain !== undefined
                                                 ? strip.deck.linear_gain : 1.0
                    // What the last wheel step sent, so a check can see the wheel working.
                    property real lastWheelGain: -1
                    // One notch is one decibel: fine enough to trim by ear, coarse enough to move.
                    readonly property real wheelStepDb: 1.0

                    // The value the wheel has already asked for. A fast spin sends several events
                    // before the engine answers; without this, each one would start from the same
                    // number and the gain would not move.
                    property real pendingGain: -1

                    function stepGain(notches) {
                        const from = strip.pendingGain > 0.0
                                     ? strip.pendingGain
                                     : (strip.deck !== null && strip.deck.linear_gain !== undefined
                                        ? strip.deck.linear_gain : 1.0);
                        const decibels = from > 0.0 ? 20.0 * Math.log(from) / Math.LN10 : -40.0;
                        const wanted = Math.max(-40.0, Math.min(6.0, decibels + notches * strip.wheelStepDb));
                        const gain = Math.pow(10.0, wanted / 20.0);
                        strip.lastWheelGain = gain;
                        strip.pendingGain = gain;
                        Bridge.setChannelGain(strip.modelData, gain);
                        return gain;
                    }

                    // The decks that live to the left of the mixer keep their knob on the left.
                    readonly property bool knobLeft: strip.modelData === "A"
                                                     || strip.modelData === "C"

                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    spacing: 2

                    Label {
                        text: strip.modelData
                        color: UiSettings.themeTextDim
                        font.pixelSize: 10
                        Layout.alignment: Qt.AlignHCenter
                    }

                    // Knob and meter, mirrored: the row runs left to right for the left-hand decks
                    // and right to left for the right-hand ones, so nothing but the direction differs.
                    RowLayout {
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        spacing: 4
                        layoutDirection: strip.knobLeft ? Qt.LeftToRight : Qt.RightToLeft

                        GainKnob {
                            Layout.preferredWidth: 22
                            Layout.fillHeight: true
                            deckId: strip.modelData
                            gain: strip.gain
                            onScrolled: function (notches) { strip.stepGain(notches) }
                        }
                        LevelMeter {
                            Layout.fillWidth: true
                            Layout.fillHeight: true
                            deckId: strip.modelData
                            peak: strip.peak
                            onScrolled: function (notches) { strip.stepGain(notches) }
                        }
                    }

                    // The engine has answered: from here the snapshot is the truth again.
                    Connections {
                        target: Bridge
                        function onSnapshotChanged() { strip.pendingGain = -1 }
                    }

                    Label {
                        text: strip.gain > 0.0
                              ? (mixer.toDb(strip.gain) >= 0 ? "+" : "")
                                + mixer.toDb(strip.gain).toFixed(1)
                              : "-∞"
                        color: UiSettings.themeTextDim
                        font.pixelSize: 9
                        Layout.alignment: Qt.AlignHCenter
                    }
                }
            }
        }

        // The crossfader sits at the bottom of the column, under the strips, where the hand rests.
        Crossfader {
            Layout.fillWidth: true
            Layout.preferredHeight: 30
        }

        // The law in force, as four buttons, each showing its own curve. The picture is the point:
        // the shapes differ at a glance, the names do not. Two columns rather than four so the
        // buttons can be the size a hand can hit inside a strip-width column.
        Grid {
            id: curveRow
            objectName: "crossfaderCurves"
            Layout.alignment: Qt.AlignHCenter
            columns: 2
            spacing: 4

            readonly property string current: Bridge.snapshot.crossfader_curve !== undefined
                                                ? Bridge.snapshot.crossfader_curve : "constant_power"

            Repeater {
                model: [
                    { law: "constant_power", hint: qsTr("Constant power: the energy sum stays at unity") },
                    { law: "linear", hint: qsTr("Linear: each side falls by the same step") },
                    { law: "smooth", hint: qsTr("Smooth: holds its level, then falls through the middle") },
                    { law: "sharp", hint: qsTr("Sharp: a cut — both sides open across the middle") }
                ]

                delegate: Rectangle {
                    required property var modelData
                    objectName: "curveButton_" + modelData.law
                    readonly property bool lit: curveRow.current === modelData.law
                    width: 64
                    height: 32
                    radius: 2
                    color: lit ? UiSettings.themePanel : UiSettings.themeSurface
                    border.width: 1
                    border.color: lit ? UiSettings.themeAccent : UiSettings.themeBorder

                    CurveGlyph {
                        anchors.fill: parent
                        anchors.margins: 3
                        law: parent.modelData.law
                        open: parent.lit ? UiSettings.themeAccent : UiSettings.themeText
                        closed: UiSettings.themeTextDim
                        guide: UiSettings.themeBorder
                    }

                    MouseArea {
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: Bridge.setCrossfaderCurve(parent.modelData.law)
                        ToolTip.visible: containsMouse
                        ToolTip.text: parent.modelData.hint
                    }
                }
            }
        }
    }

}
