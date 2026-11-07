// Mixer: one strip per deck and the crossfader below them (DJP-MIX-001).
//
// A strip is a level meter with the gain pointer drawn on it, as tall as the column. There is no
// separate fader column: a strip is narrow, and a column for the fader took a quarter of the meter's
// width. The pointer is a bright line across the meter at the gain's height, with a grip in the
// middle, and the wheel or a drag on the meter trims the channel.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Pane {
    id: mixer
    objectName: "mixerPanel"

    readonly property var decks: Bridge.snapshot.decks !== undefined ? Bridge.snapshot.decks : []
    // The order is a setting (LayoutState.channelOrder); C first by default, as the owner asked.
    readonly property var order: LayoutState.channelOrder.split("")
    // Two halves with the master between them: the master belongs in the middle of the mixer.
    readonly property int half: Math.floor(order.length / 2)
    readonly property var leftStrips: order.slice(0, half)
    readonly property var rightStrips: order.slice(half)
    // What the engine published for the master bus.
    readonly property real masterPeak: Bridge.snapshot.master !== undefined
                                       ? Math.max(Bridge.snapshot.master.peak_left,
                                                  Bridge.snapshot.master.peak_right) : 0.0
    readonly property real masterGain: Bridge.snapshot.master_gain !== undefined
                                       ? Bridge.snapshot.master_gain : 1.0

    // One meter band for the whole mixer. The master's knob stands above its meter, so that column
    // decides how much room a meter gets; the deck meters take the same height instead of running
    // taller, and everything lines up at the bottom.
    // The strip band: panel height minus margins, the crossfader and the law buttons. Taken from the
    // panel on purpose — the row's own height depends on what the meters ask for, and reading it back
    // is a loop that settles stale.
    // Everything in the column that is not the meter band: the pane's own padding, the two gaps, the
    // crossfader and the row of law buttons. Measured on the built scene, not guessed: with 86 the
    // strip row came out 18 px shorter than the number the meters were sized from, and the meter grew
    // into the EQ labels.
    readonly property real chromeHeight: 104
    readonly property real bandHeight: Math.max(56, mixer.height - chromeHeight)
    readonly property real masterKnobSide: Math.max(14, Math.min(30, bandHeight * 0.32))
    // The three EQ knobs of a channel are the size of the master knob, so the strip needs room for
    // three of those caps plus their labels. The meter gives up exactly that room and not a pixel
    // more: the height is computed from what the *deck* strip has above and below its meter, because
    // the deck is the strip that carries the most controls. Every meter in the mixer then takes that
    // one height, master included: channel meters and the master end on one line at the bottom, and
    // that line is what makes a strip readable at a glance.
    readonly property real eqKnobSide: masterKnobSide
    readonly property real eqLabelHeight: 11
    readonly property real eqStackHeight: 3 * (eqKnobSide + eqLabelHeight) + 2
    // The deck letter above the knobs (10 px of text, 2 px of margin, 2 px of gap) and the decibel
    // label under the meter with its own margin. Measured on the built scene: the number has to come
    // out even, or the meter grows into the LO label by exactly the difference.
    readonly property real stripHeaderHeight: 14
    readonly property real stripFooterHeight: 14
    readonly property real meterHeight: Math.max(
        18, bandHeight - stripHeaderHeight - eqStackHeight - stripFooterHeight)

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
            id: stripRow
            objectName: "stripRow"
            Layout.fillWidth: true
            Layout.fillHeight: true
            // A meter band has to survive a short window: knob, label and at least 14 px of meter.
            Layout.minimumHeight: 56
            spacing: 4

            Repeater {
                model: mixer.leftStrips
                delegate: stripComponent
            }

            // The master column: a round gain knob over a coloured meter. Everything is anchored to
            // the bottom edge, so the meter ends on the same line as the deck meters.
            Item {
                id: masterColumn
                objectName: "masterColumn"
                Layout.preferredWidth: 34
                Layout.fillHeight: true

                // The knob sits at the top of its column, level with the channel's top knob: with
                // every meter the same height, the master column has room to spare above its meter,
                // and a knob hanging under that gap would read as part of the void.
                MasterKnob {
                    anchors.horizontalCenter: parent.horizontalCenter
                    anchors.top: parent.top
                    anchors.topMargin: 2
                    width: mixer.masterKnobSide
                    height: mixer.masterKnobSide
                    gain: mixer.masterGain
                }
                MasterMeter {
                    id: masterMeterItem
                    anchors.horizontalCenter: parent.horizontalCenter
                    anchors.bottom: masterLabel.top
                    anchors.bottomMargin: 2
                    width: 28
                    height: mixer.meterHeight
                    peak: mixer.masterPeak
                }
                Label {
                    id: masterLabel
                    anchors.horizontalCenter: parent.horizontalCenter
                    anchors.bottom: parent.bottom
                    text: mixer.masterGain > 0.0
                          ? (mixer.toDb(mixer.masterGain) >= 0 ? "+" : "")
                            + mixer.toDb(mixer.masterGain).toFixed(1)
                          : "-∞"
                    color: UiSettings.themeTextDim
                    font.pixelSize: 9
                }
            }
            Repeater {
                model: mixer.rightStrips
                delegate: stripComponent
            }
        }

        // The crossfader sits at the bottom of the column, under the strips, where the hand rests.
        Crossfader {
            Layout.fillWidth: true
            Layout.preferredHeight: 30
        }

        // The law in force, as four buttons in one row, each showing its own curve. The picture is
        // the point: the shapes differ at a glance, the names do not. One row, not two: vertical room
        // belongs to the strips and the crossfader.
        RowLayout {
            id: curveRow
            objectName: "crossfaderCurves"
            Layout.fillWidth: true
            Layout.preferredHeight: 32
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
                    // The four share the column's width; the height stays the one a hand can hit.
                    Layout.fillWidth: true
                    Layout.preferredHeight: 32
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

    // One channel strip, drawn twice: the halves only differ in which decks they hold.
    Component {
        id: stripComponent
        Item {
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
            // A band's gain as the engine published it, in decibels.
            function bandValue(band) {
                const eq = strip.deck !== null && strip.deck.eq !== undefined ? strip.deck.eq : null;
                if (eq === null) {
                    return 0.0;
                }
                const key = band + "_db";
                return eq[key] !== undefined ? eq[key] : 0.0;
            }

            // What the last wheel step sent, so a check can see the wheel working.
            property real lastWheelGain: -1
            // One notch is one decibel: fine enough to trim by ear, coarse enough to move.
            readonly property real wheelStepDb: 1.0

            // The value the wheel has already asked for. A fast spin sends several events before the
            // engine answers; without this, each one would start from the same number and the gain
            // would not move.
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

            Layout.fillWidth: true
            Layout.fillHeight: true

            // Anchored to the edges: the meter band sits on the bottom line, level with the master's,
            // and the deck's letter keeps the top. No positioner, so nothing to settle stale.
            Label {
                id: deckLabel
                anchors.horizontalCenter: parent.horizontalCenter
                anchors.top: parent.top
                text: strip.modelData
                color: UiSettings.themeTextDim
                font.pixelSize: 10
            }

            // The channel's EQ: three knobs above the meter, high at the top — the order a strip in
            // the reference draws, and the order a hand learns. Every value comes from the snapshot:
            // the knobs show what the engine applied, not what the last click asked for.
            Column {
                id: eqStack
                anchors.horizontalCenter: parent.horizontalCenter
                anchors.top: deckLabel.bottom
                anchors.topMargin: 2
                spacing: 1

                Repeater {
                    model: [
                        { band: "high", letter: "HI" },
                        { band: "mid", letter: "MID" },
                        { band: "low", letter: "LO" }
                    ]

                    delegate: EqKnob {
                        required property var modelData
                        width: strip.width
                        deckId: strip.modelData
                        band: modelData.band
                        letter: modelData.letter
                        diameter: mixer.eqKnobSide
                        value: strip.bandValue(modelData.band)
                    }
                }
            }

            // The meter carries the gain pointer: one column does both jobs, and the strip keeps its
            // width for the meter itself. The pointer is a line across the meter at the gain's height.
            Item {
                id: meterBand
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.bottom: gainLabel.top
                anchors.bottomMargin: 2
                height: mixer.meterHeight

                LevelMeter {
                    anchors.fill: parent
                    deckId: strip.modelData
                    peak: strip.peak
                    onScrolled: function (notches) { strip.stepGain(notches) }
                }
                GainKnob {
                    anchors.fill: parent
                    deckId: strip.modelData
                    gain: strip.gain
                    onScrolled: function (notches) { strip.stepGain(notches) }
                }
            }

            Label {
                id: gainLabel
                anchors.horizontalCenter: parent.horizontalCenter
                anchors.bottom: parent.bottom
                text: strip.gain > 0.0
                      ? (mixer.toDb(strip.gain) >= 0 ? "+" : "")
                        + mixer.toDb(strip.gain).toFixed(1)
                      : "-∞"
                color: UiSettings.themeTextDim
                font.pixelSize: 9
            }

            // The engine has answered: from here the snapshot is the truth again.
            Connections {
                target: Bridge
                function onSnapshotChanged() { strip.pendingGain = -1 }
            }
        }
    }
}
