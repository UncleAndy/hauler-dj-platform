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
    // What the engine published for the master bus: the bar's held value when the engine sends one
    // (the peak falls back over half a second), the raw block peak from an older engine.
    readonly property real masterPeak: {
        const master = Bridge.snapshot.master;
        if (master === undefined) return 0.0;
        if (master.held_left !== undefined)
            return Math.max(master.held_left, master.held_right);
        return Math.max(master.peak_left, master.peak_right);
    }
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
    // The headphone knobs are smaller than the master: two of them and two captions share the column's width.
    // The headphone knobs are the master's size: one column, one class of control (owner's decision, 20.11).
    // The knobs are the originals' size (owner's decision, 20.11): the volume is a master knob and the proportion
    // is a channel gain knob, so they are as big as those two.
    readonly property real cueKnobSide: masterKnobSide

    /// The headphone value the engine published, by knob kind.
    function headphoneValue(kind) {
        const phones = Bridge.snapshot.headphones !== undefined ? Bridge.snapshot.headphones : null;
        if (phones === null || phones === undefined) {
            return kind === "phones" ? 1.0 : 1.0;
        }
        return kind === "phones" ? (phones.gain !== undefined ? phones.gain : 1.0)
                                 : (phones.mix !== undefined ? phones.mix : 1.0);
    }
    readonly property real eqLabelHeight: 11
    readonly property real eqStackHeight: 3 * (eqKnobSide + eqLabelHeight) + 2
    // The gain knob takes one more row of the same size, right under the deck's letter: it belongs
    // above the EQ, because gain is what the EQ sees.
    readonly property real trimKnobHeight: eqKnobSide + eqLabelHeight
    // The deck letter above the knobs (10 px of text, 2 px of margin, 2 px of gap) and the decibel
    // label under the meter with its own margin. Measured on the built scene: the number has to come
    // out even, or the meter grows into the LO label by exactly the difference.
    readonly property real stripHeaderHeight: 14
    // The row under the meter belongs to the monitoring button: a square a finger can find (owner's decision,
    // 20.11). The fader's number moved onto the fader, and the master column keeps a row of the same height —
    // which is what holds both meters on one line.
    readonly property real stripFooterHeight: 36
    // Everything a strip puts above its meter: the letter, the gain knob and the EQ stack, with gaps.
    readonly property real stripKnobsHeight: stripHeaderHeight + trimKnobHeight + 2 + eqStackHeight + 2
    // Three pixels of air between the EQ labels and the meter: without them the meter starts exactly where the
    // knobs end, and the check that guards the arrangement reads a sub-pixel overlap as a collision.
    /// The effect block under the EQ: a knob, the switch and the chooser, one above the other (owner's decision,
    /// 21.11: «ручка и кнопка — по вертикали»). It is paid for out of the meter, which the owner accepted with every
    /// price known: the meter of a deck loses these 51 px.
    readonly property real fxKnobSide: eqKnobSide
    readonly property real fxButtonHeight: 14
    readonly property real fxBlockHeight: fxKnobSide + fxButtonHeight * 2 + 5

    readonly property real meterHeight: Math.max(
        18, bandHeight - stripKnobsHeight - stripFooterHeight - 3 - fxBlockHeight)

    // The law the shell remembers is handed to the engine on the first snapshot: the engine starts from its own
    // default, and a restarted session must not sound different from the one before (owner's request, 20.11).
    property bool curveRestored: false
    Connections {
        target: Bridge
        function onSnapshotChanged() {
            if (mixer.curveRestored) {
                return;
            }
            mixer.curveRestored = true;
            const saved = Session.crossfaderCurve;
            const current = Bridge.snapshot.crossfader_curve;
            if (saved !== undefined && saved !== "" && saved !== current) {
                Bridge.setCrossfaderCurve(saved);
                console.log("dj-ui: crossfader curve restored: " + saved);
            }
        }
    }

    // The effect each deck was left on is handed back to the engine on the first snapshot: the engine starts from
    // its own default, and the slot a DJ uses every night must not have to be chosen again every night.
    property bool fxRestored: false
    Connections {
        target: Bridge
        function onSnapshotChanged() {
            if (mixer.fxRestored) {
                return;
            }
            mixer.fxRestored = true;
            const remembered = Session.deckFx;
            const decks = Bridge.snapshot.decks !== undefined ? Bridge.snapshot.decks : [];
            for (let index = 0; index < decks.length; ++index) {
                const deck = decks[index];
                const want = remembered[deck.deck];
                const current = deck.fx !== undefined ? deck.fx : null;
                if (want === undefined || current === null || want === current.kind) {
                    continue;
                }
                Bridge.setDeckFx(deck.deck, want, current.amount, current.enabled);
                Bridge.note("fx of deck " + deck.deck + " restored: " + want);
            }
        }
    }

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
                // Wide enough for the two global switches that stand under the knob: the master is where
                // the whole mix is decided, and the modes of the whole mix belong under it (owner's
                // decision, 14.11).
                Layout.preferredWidth: 48
                Layout.fillHeight: true

                // The knob sits at the top of its column, level with the channel's top knob: with
                // every meter the same height, the master column has room to spare above its meter,
                // and a knob hanging under that gap would read as part of the void.
                MasterKnob {
                    id: masterKnobItem
                    anchors.horizontalCenter: parent.horizontalCenter
                    anchors.top: parent.top
                    // Level with the channels' gain knobs: the master is the gain of the whole mix,
                    // and one line across the mixer is how a person reads four trims and a master.
                    anchors.topMargin: mixer.stripHeaderHeight
                    width: mixer.eqKnobSide
                    height: mixer.eqKnobSide
                    diameter: mixer.eqKnobSide
                    gain: mixer.masterGain
                }
                // The two headphone knobs, under the master's, one above the other. They are the components the
                // mixer already has, not look-alikes: the volume is `MasterKnob` (its scale, its arc, its law) and
                // the proportion is `TrimKnob` (a channel's gain knob), each wired to its own bus (owner's
                // instruction, 20.11: "просто повтори дизайн уже имеющихся ручек").
                Column {
                    id: phonesColumn
                    anchors.horizontalCenter: parent.horizontalCenter
                    anchors.top: masterKnobItem.bottom
                    anchors.topMargin: 4
                    spacing: 2

                    Item {
                        width: mixer.cueKnobSide
                        height: 10
                        Text {
                            anchors.horizontalCenter: parent.horizontalCenter
                            text: qsTr("PHONES")
                            color: UiSettings.themeTextDim
                            font.pixelSize: 8
                        }
                    }

                    MasterKnob {
                        itemName: "cueKnob_phones"
                        sink: "phones"
                        gain: mixer.headphoneValue("phones")
                        diameter: mixer.cueKnobSide
                    }

                    Item {
                        width: mixer.cueKnobSide
                        height: 10
                        Text {
                            anchors.horizontalCenter: parent.horizontalCenter
                            text: qsTr("MIX")
                            color: UiSettings.themeTextDim
                            font.pixelSize: 8
                        }
                    }

                    TrimKnob {
                        itemName: "cueKnob_mix"
                        sink: "mix"
                        // The proportion as a gain knob sees it: 0 dB is even, ±6 dB are its ends, and the pointer
                        // stands where a channel's trim would (owner's decision, 20.11).
                        deckId: "mix"
                        showNumber: false
                        // The master's fill: same geometry and the same look as the knob above it (owner's
                        // decision, 20.11).
                        capColor: UiSettings.themeSurface
                        // Six decibels each way: the proportion's ends are its ends, not a seventh of a channel
                        // gain's travel.
                        negativeRangeDb: 6
                        gain: Math.pow(10.0, ((0.5 - mixer.headphoneValue("mix")) * 12.0) / 20.0)
                        diameter: mixer.cueKnobSide
                    }
                }

                // The two modes a DJ sets once for the set rather than per deck: snap (a loop point lands
                // on the grid) and quantize (the loop's end is whole beats after its start). They stand in
                // the void between the master's knob and its meter, centred on the knob, and the decks
                // read them (owner's decision, 14.11).
                // The row's bottom sits on the line where the strips' smallest knob ends (the LO label), so the
                // two switches read as part of the same block (owner's decision, 20.11). A spacer carries the
                // height the strips' knob block takes, and the row is pinned to its foot.
                Item {
                    id: knobsBlock
                    anchors.left: parent.left
                    anchors.right: parent.right
                    anchors.top: parent.top
                    height: mixer.stripKnobsHeight
                }

                ColumnLayout {
                    anchors.horizontalCenter: parent.horizontalCenter
                    anchors.bottom: knobsBlock.bottom
                    spacing: 4

                    Rectangle {
                        objectName: "snapGlobal"
                        readonly property bool on: UiSettings.snapGrid
                        Layout.preferredWidth: 46
                        Layout.preferredHeight: 18
                        radius: 2
                        color: on ? UiSettings.themeAccent : UiSettings.themeRaised
                        border.width: 1
                        border.color: UiSettings.themeBorder
                        Text {
                            anchors.centerIn: parent
                            text: qsTr("snap")
                            font.pixelSize: 10
                            color: parent.on ? UiSettings.themeTextOnAccent : UiSettings.themeText
                        }
                        MouseArea {
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: Qt.PointingHandCursor
                            onClicked: {
                        UiSettings.snapGrid = !UiSettings.snapGrid;
                        // A mode is a setting: the file carries it across a restart (owner's decision).
                        LayoutState.snapGrid = UiSettings.snapGrid;
                    }
                        }
                    }
                    Rectangle {
                        objectName: "quantizeGlobal"
                        readonly property bool on: UiSettings.quantizeGrid
                        Layout.preferredWidth: 46
                        Layout.preferredHeight: 18
                        radius: 2
                        color: on ? UiSettings.themeAccent : UiSettings.themeRaised
                        border.width: 1
                        border.color: UiSettings.themeBorder
                        Text {
                            anchors.centerIn: parent
                            text: qsTr("quant")
                            font.pixelSize: 10
                            color: parent.on ? UiSettings.themeTextOnAccent : UiSettings.themeText
                        }
                        MouseArea {
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: Qt.PointingHandCursor
                            onClicked: {
                        UiSettings.quantizeGrid = !UiSettings.quantizeGrid;
                        LayoutState.quantizeGrid = UiSettings.quantizeGrid;
                    }
                        }
                    }
                }
                MasterMeter {
                    id: masterMeterItem
                    anchors.horizontalCenter: parent.horizontalCenter
                    anchors.bottom: masterFooter.top
                    anchors.bottomMargin: 2
                    width: 28
                    height: mixer.meterHeight
                    peak: mixer.masterPeak
                }
                // The master keeps its number, but in a row as tall as the strips' button row: that is
                // what puts the master's meter on the same line as the deck meters.
                Item {
                    id: masterFooter
                    anchors.left: parent.left
                    anchors.right: parent.right
                    anchors.bottom: parent.bottom
                    height: mixer.stripFooterHeight

                    Label {
                        id: masterLabel
                        anchors.horizontalCenter: parent.horizontalCenter
                        anchors.verticalCenter: parent.verticalCenter
                        text: mixer.masterGain > 0.0
                              ? (mixer.toDb(mixer.masterGain) >= 0 ? "+" : "")
                                + mixer.toDb(mixer.masterGain).toFixed(1)
                              : "-∞"
                        color: UiSettings.themeTextDim
                        font.pixelSize: 9
                    }
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

            // The law in force. A cut-in law travels as an object (its own number rides with it), so the row
            // reads the object's name and its number, and a plain name stays a name.
            readonly property var raw: Bridge.snapshot.crossfader_curve
            readonly property string current: raw === undefined
                                              ? "constant_power"
                                              : (typeof raw === "object" && raw !== null
                                                 ? Object.keys(raw)[0] : raw)
            readonly property int cutIn: (typeof raw === "object" && raw !== null && raw.cut !== undefined)
                                          ? raw.cut.cut_in : 5

            Repeater {
                model: [
                    { law: "constant_power", hint: qsTr("Constant power: the energy sum stays at unity") },
                    { law: "linear", hint: qsTr("Linear: each side falls by the same step") },
                    { law: "smooth", hint: qsTr("Smooth: holds its level, then falls through the middle") },
                    { law: "sharp", hint: qsTr("Sharp: a cut — both sides open across the middle") },
                    { law: "cut", hint: qsTr("Cut-in: closed until it opens, like a scratch mixer's fader") }
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
                        cutIn: curveRow.cutIn
                        open: parent.lit ? UiSettings.themeAccent : UiSettings.themeText
                        closed: UiSettings.themeTextDim
                        guide: UiSettings.themeBorder
                    }

                    /// Choose this law. A hand clicks the area below and a check calls this by name, so both go
                    /// through the remembering path and neither can drift from the other (owner's request, 20.11).
                    function pick() {
                        const law = modelData.law === "cut" ? "cut:" + curveRow.cutIn : modelData.law;
                        Session.rememberCrossfaderCurve(law);
                        Bridge.setCrossfaderCurve(law);
                    }

                    MouseArea {
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: parent.pick()
                        ToolTip.visible: containsMouse
                        ToolTip.text: parent.modelData.hint
                    }
                }
            }
            // Where the cut opens, in percent of the travel: two steps, because a fader opening is a
            // decision about feel, not a measurement. Visible only while the cut-in law is in force.
            RowLayout {
                objectName: "cutInRow"
                Layout.fillWidth: true
                Layout.preferredHeight: visible ? 24 : 0
                spacing: 4
                visible: curveRow.current === "cut"

                Text {
                    text: qsTr("Cut-in")
                    color: UiSettings.themeTextDim
                    font.pixelSize: 9
                    Layout.alignment: Qt.AlignVCenter
                }

                Rectangle {
                    objectName: "cutInDown"
                    Layout.preferredWidth: 22
                    Layout.preferredHeight: 20
                    color: UiSettings.themeSurface
                    border.width: 1
                    border.color: UiSettings.themeBorder
                    radius: 2
                    Text {
                        anchors.centerIn: parent
                        text: "\u2212"
                        color: UiSettings.themeText
                        font.pixelSize: 9
                    }
                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: {
                            const next = Math.max(0, curveRow.cutIn - 1);
                            Bridge.setCrossfaderCurve("cut:" + next);
                        }
                    }
                }

                Text {
                    objectName: "cutInValue"
                    text: curveRow.cutIn + qsTr(" %")
                    color: UiSettings.themeText
                    font.pixelSize: 9
                    Layout.alignment: Qt.AlignVCenter
                }

                Rectangle {
                    objectName: "cutInUp"
                    Layout.preferredWidth: 22
                    Layout.preferredHeight: 20
                    color: UiSettings.themeSurface
                    border.width: 1
                    border.color: UiSettings.themeBorder
                    radius: 2
                    Text {
                        anchors.centerIn: parent
                        text: "+"
                        color: UiSettings.themeText
                        font.pixelSize: 9
                    }
                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: {
                            const next = Math.min(50, curveRow.cutIn + 1);
                            Bridge.setCrossfaderCurve("cut:" + next);
                        }
                    }
                }

                Item { Layout.fillWidth: true }
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
            // The bar's value: held and falling back. An engine that only publishes peaks still
            // draws, so the shell works against an older process.
            readonly property real peak: strip.deck !== null && strip.deck.levels !== undefined
                                         ? (strip.deck.levels.held_left !== undefined
                                            ? Math.max(strip.deck.levels.held_left,
                                                       strip.deck.levels.held_right)
                                            : Math.max(strip.deck.levels.peak_left,
                                                       strip.deck.levels.peak_right))
                                         : 0.0
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

            // The deck's effect as the engine has it. A strip built against an older engine has no `fx` field at
            // all, which reads here as the slot at its centre and switched out.
            readonly property var fx: strip.deck !== null && strip.deck.fx !== undefined ? strip.deck.fx : null
            readonly property string fxKind: strip.fx !== null && strip.fx.kind !== undefined
                                            ? strip.fx.kind : "filter"
            readonly property real fxAmount: strip.fx !== null && strip.fx.amount !== undefined
                                             ? strip.fx.amount : 0.5
            readonly property bool fxEnabled: strip.fx !== null && strip.fx.enabled !== undefined
                                              ? strip.fx.enabled : false
            /// The three letters the switch wears: the unit's own abbreviation, not a translated word.
            readonly property string fxCode: {
                const codes = { filter: "FLT", echo: "ECH", roll: "ROL", flanger: "FLG",
                                phaser: "PHS", bitcrusher: "BIT", noise: "NSE" };
                return codes[strip.fxKind] !== undefined ? codes[strip.fxKind] : "FLT";
            }

            /// Send the whole patch: the engine never has to guess which part of it moved.
            ///
            /// The unit is remembered as a habit — a deck that was on echo yesterday is on echo again today — while
            /// the amount stays with the track (owner's decision, 21.11).
            function setFx(kind, amount, enabled) {
                Session.rememberDeckFx(strip.modelData, kind);
                Bridge.setDeckFx(strip.modelData, kind, amount, enabled);
            }

            // The channel fader as the engine published it: 1.0 is unity, 0.0 is silence.
            readonly property real fader: strip.deck !== null && strip.deck.fader !== undefined
                                          ? strip.deck.fader : 1.0
            // What the last wheel step sent, so a check can see the wheel working.
            property real lastWheelFader: -1
            // One notch is five percent of the throw: fine enough to ride, coarse enough to move.
            readonly property real wheelStepFader: 0.05
            // The value the wheel has already asked for. A fast spin sends several events before the
            // engine answers; without this, each one would start from the same number.
            property real pendingFader: -1

            function stepFader(notches) {
                const from = strip.pendingFader >= 0.0 ? strip.pendingFader : strip.fader;
                const wanted = Math.max(0.0, Math.min(1.0,
                                                     from + notches * strip.wheelStepFader));
                strip.pendingFader = wanted;
                strip.lastWheelFader = wanted;
                Bridge.setChannelFader(strip.modelData, wanted);
                return wanted;
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

            // The gain: a knob, set once per track to line its level up before the EQ. It is not the
            // fader — that is the line on the meter — and keeping the two apart is the whole point of
            // the two controls.
            TrimKnob {
                id: trimKnob
                anchors.horizontalCenter: parent.horizontalCenter
                anchors.top: deckLabel.bottom
                anchors.topMargin: 2
                width: strip.width
                deckId: strip.modelData
                diameter: mixer.eqKnobSide
                gain: strip.gain
            }

            // The channel's EQ: three knobs under the gain, high at the top — the order a strip in
            // the reference draws, and the order a hand learns. Every value comes from the snapshot:
            // the knobs show what the engine applied, not what the last click asked for.
            Column {
                id: eqStack
                anchors.horizontalCenter: parent.horizontalCenter
                anchors.top: trimKnob.bottom
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

            // The deck's effect: a knob (how much), a switch (whether it is in the path) and a chooser (which of the
            // seven), one above the other. It stands on the meter's top edge, so the meter and the master's still end
            // on the same line while this block takes its height — the owner's layout of 21.11.
            Column {
                id: fxColumn
                anchors.horizontalCenter: parent.horizontalCenter
                anchors.bottom: meterBand.top
                anchors.bottomMargin: 2
                spacing: 1

                TrimKnob {
                    id: fxKnob
                    objectName: "fxKnob_" + strip.modelData
                    itemName: "fxKnob_" + strip.modelData
                    sink: "fx"
                    deckId: strip.modelData
                    showNumber: false
                    capColor: UiSettings.themeSurface
                    diameter: mixer.fxKnobSide
                    // Six decibels each side of the top: the pointer's two ends are the amount's two ends, and the top is
                    // half the effect (owner's report, 21.11).
                    negativeRangeDb: 6
                    gain: Math.pow(10.0, ((strip.fxAmount * 12.0) - 6.0) / 20.0)
                    onFxAmountAsked: function (amount) {
                        strip.setFx(strip.fxKind, amount, strip.fxEnabled);
                    }
                }

                Rectangle {
                    objectName: "fx_" + strip.modelData
                    readonly property bool lit: strip.fxEnabled
                    width: mixer.fxKnobSide
                    height: mixer.fxButtonHeight
                    radius: 2
                    color: lit ? UiSettings.themeAccent : UiSettings.themeRaised
                    border.width: 1
                    border.color: UiSettings.themeBorder

                    /// The switch, callable by name so a check presses the same path a hand does.
                    function toggle() {
                        strip.setFx(strip.fxKind, strip.fxAmount, !strip.fxEnabled);
                    }

                    /// Press and release. A roll is held — it captures the beat fraction on the way down and gives
                    /// the live signal back on the way up — while every other unit is switched on and off by a
                    /// click (owner's decision 21.11: the roll is momentary).
                    function press() {
                        if (strip.fxKind === "roll") {
                            strip.setFx(strip.fxKind, strip.fxAmount, true);
                        } else {
                            toggle();
                        }
                    }

                    function release() {
                        if (strip.fxKind === "roll") {
                            strip.setFx(strip.fxKind, strip.fxAmount, false);
                        }
                    }

                    readonly property bool held: strip.fxKind === "roll"

                    Text {
                        anchors.centerIn: parent
                        text: strip.fxCode
                        font.pixelSize: 8
                        color: parent.lit ? UiSettings.themeTextOnAccent : UiSettings.themeText
                    }

                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        // A held roll must not latch: `onReleased` is the way back to the live signal, and it fires
                        // even when the pointer leaves the button.
                        onPressed: parent.press()
                        onReleased: parent.release()
                        onCanceled: parent.release()
                    }
                }

                Rectangle {
                    id: fxPickButton
                    objectName: "fxPick_" + strip.modelData
                    width: mixer.fxKnobSide
                    height: mixer.fxButtonHeight
                    radius: 2
                    color: UiSettings.themeSurface
                    border.width: 1
                    border.color: UiSettings.themeBorder

                    /// Choose the unit in the slot. The menu is below; a check calls this by name.
                    function pick(kind) {
                        strip.setFx(kind, strip.fxAmount, strip.fxEnabled);
                    }

                    Text {
                        anchors.centerIn: parent
                        text: "\u25be"
                        font.pixelSize: 9
                        color: UiSettings.themeText
                    }

                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: fxMenu.open()
                    }

                    Menu {
                        id: fxMenu
                        objectName: "fxMenu_" + strip.modelData
                        parent: fxPickButton
                        y: parent.height
                        // Seven rows, the current one marked: the panel is narrow, and a row of seven buttons would
                        // not fit a strip (owner's decision, 21.11).
                        Repeater {
                            model: [
                                { kind: "filter", label: qsTr("Filter") },
                                { kind: "echo", label: qsTr("Echo") },
                                { kind: "roll", label: qsTr("Roll") },
                                { kind: "flanger", label: qsTr("Flanger") },
                                { kind: "phaser", label: qsTr("Phaser") },
                                { kind: "bitcrusher", label: qsTr("Bitcrusher") },
                                { kind: "noise", label: qsTr("Noise") }
                            ]
                            MenuItem {
                                required property var modelData
                                text: (strip.fxKind === modelData.kind ? "\u2713 " : "   ") + modelData.label
                                onTriggered: strip.setFx(modelData.kind, strip.fxAmount, strip.fxEnabled)
                            }
                        }
                    }
                }
            }

            // The meter carries the fader: one column does both jobs, and the strip keeps its width
            // for the meter itself. The line sits across the meter at the fader's height.
            Item {
                id: meterBand
                objectName: "meter_" + strip.modelData
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.bottom: footer.top
                anchors.bottomMargin: 2
                height: mixer.meterHeight

                LevelMeter {
                    anchors.fill: parent
                    deckId: strip.modelData
                    peak: strip.peak
                    onScrolled: function (notches) { strip.stepFader(notches) }
                }
                FaderKnob {
                    anchors.fill: parent
                    deckId: strip.modelData
                    position: strip.fader
                    onScrolled: function (notches) { strip.stepFader(notches) }
                }
            }

            // The row under the meter belongs to the monitoring button. The fader's number is drawn on the
            // fader itself now (owner's decision, 20.11).
            Item {
                id: footer
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.bottom: parent.bottom
                height: mixer.stripFooterHeight
            }

            // Monitoring: the headphones beside this deck. It sits under the meter, in the row the fader number
            // already uses — so the meters keep their line and the button is where the deck is (owner's decision,
            // 20.11). Drawn, not typed: the shell's font has no headphone glyph on Windows.
            Rectangle {
                id: cueButton
                // `cue_<deck>` is the deck's transport CUE: the monitoring switch needs its own name.
                objectName: "headphones_" + strip.modelData
                /// True when this channel feeds the headphones, as the engine last published it.
                readonly property bool lit: strip.deck !== null && strip.deck.cue === true
                anchors.horizontalCenter: footer.horizontalCenter
                anchors.verticalCenter: footer.verticalCenter
                // Square and big enough for a finger: the old 16×13 was a dot (owner's decision, 20.11).
                width: 32
                height: 32
                radius: 3
                color: lit ? UiSettings.themeAccent : UiSettings.themeSurface
                border.width: 1
                border.color: lit ? UiSettings.themeAccent : UiSettings.themeBorder

                /// Toggle monitoring for this channel. A check calls this; a hand clicks the area below.
                function toggle() {
                    Bridge.setChannelCue(strip.modelData, !cueButton.lit);
                }

                HeadphoneGlyph {
                    anchors.centerIn: parent
                    width: 20
                    height: 20
                    colour: cueButton.lit ? UiSettings.themePanel : UiSettings.themeText
                }

                MouseArea {
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: cueButton.toggle()
                    ToolTip.visible: containsMouse
                    ToolTip.text: qsTr("Headphones: listen to this channel before the fader")
                }
            }

            // The engine has answered: from here the snapshot is the truth again.
            Connections {
                target: Bridge
                function onSnapshotChanged() { strip.pendingFader = -1 }
            }
        }
    }
}
