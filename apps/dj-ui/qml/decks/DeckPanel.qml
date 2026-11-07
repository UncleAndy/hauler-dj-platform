// A deck panel driven entirely by the engine snapshot (DJP-ARCH-004).
//
// The arrangement follows the reference: the deck's letter and the track's title share one line, the
// zoomed strip takes the room, the whole track sits under it as a thin overview, and the transport
// controls are a row at the bottom. There is no load button: tracks come from the browser, into the
// deck's own button there, which is also the only place that has to say "this deck is playing". Nothing is optimistic: pressing Play sends a command and waits,
// and the button only reads "playing" once a snapshot says so.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import "../views"
import "../mixer"

Pane {
    id: panel

    required property string deckId
    required property string deckLabel
    // The deck the browser will load into; the panel shows it instead of leaving the user to guess.
    property bool targeted: false
    signal chosen()

    background: Rectangle {
        objectName: "deckFrame_" + panel.deckId
        color: UiSettings.themePanel
        // The frame answers one question — is this deck playing? A border that marks the browser's
        // target instead sits on one deck for ever and says nothing about the sound.
        border.color: panel.playing ? UiSettings.themeAccent : UiSettings.themeBorder
        border.width: panel.playing ? 2 : 1
        radius: 2

        MouseArea {
            anchors.fill: parent
            // Behind the controls: a click on empty panel space chooses this deck as the browser's
            // target, a click on a control does what the control says.
            onClicked: panel.chosen()
        }
    }

    readonly property bool live: Bridge.connected
    /// Whether a hand on the waveform is heard: a scratch. Silent by default — a booth is not a place
    /// to discover that dragging a strip makes noise.
    property bool scrubAudio: false
    readonly property var deck: {
        const decks = Bridge.snapshot.decks;
        if (decks === undefined || decks === null || decks.length === undefined) {
            return ({});
        }
        for (let index = 0; index < decks.length; ++index) {
            if (decks[index].deck === panel.deckId) {
                return decks[index];
            }
        }
        return ({});
    }

    readonly property bool playing: panel.deck.playing === true
    readonly property var track: panel.deck.track !== undefined ? panel.deck.track : null
    readonly property var loop: panel.deck.loop_region !== undefined ? panel.deck.loop_region : null

    // One envelope per deck, shared by the zoomed strip and the overview: loading into A can never
    // repaint B, and the two strips cannot disagree about how long the track is.
    WaveformEnvelope { id: envelope }

    function timeLabel(frames) {
        const rate = envelope.sampleRate > 0 ? envelope.sampleRate : 48000;
        if (frames === undefined || frames === null || frames < 0) {
            return "—";
        }
        const seconds = Math.floor(frames / rate);
        const minutes = Math.floor(seconds / 60);
        return minutes + ":" + ("0" + (seconds % 60)).slice(-2);
    }

    function positionLabel() {
        return panel.timeLabel(panel.deck.source_frame);
    }

    /// The point Stop and Cue return to: hotcue 0 when it is set, the start of the file otherwise.
    function cueFrame() {
        const slots = panel.deck.hotcues !== undefined ? panel.deck.hotcues : null;
        if (slots !== null && slots.length > 0 && slots[0] !== null && slots[0] !== undefined) {
            return slots[0];
        }
        return 0;
    }

    /// Stop is not Pause: it puts the playhead back on the point the deck starts from, which is what a
    /// hand expects from the button with that name (DJP-DECK-002).
    function stop() {
        Bridge.play(panel.deckId, false);
        Bridge.seek(panel.deckId, panel.cueFrame());
    }

    // ---- the eight pads (DJP-DECK-003) ----------------------------------------------------
    // Eight colours, readable on a dark panel and told apart from each other in a booth. Not called
    // `palette`: that name belongs to the control's own theme palette, and shadowing it is a warning.
    readonly property var cuePalette: ["#ff4d4d", "#ff9f1a", "#7cf03d", "#00e0c6",
                                    "#3d9bff", "#b07bff", "#ff5cc8", "#ffffff"]
    /// Loops wear yellow and points never do, so a glance tells a loop from a cue point.
    readonly property string loopColor: "#ffd400"

    function hotcueFrame(slot) {
        const cues = panel.deck !== null && panel.deck.hotcues !== undefined ? panel.deck.hotcues : [];
        return slot < cues.length && cues[slot] !== null && cues[slot] !== undefined ? cues[slot] : null;
    }

    function hotcueLabel(slot) {
        const list = panel.deck !== null && panel.deck.cue_labels !== undefined
                     ? panel.deck.cue_labels : [];
        return slot < list.length && list[slot] !== null && list[slot] !== undefined ? list[slot] : "";
    }

    function padLoop(slot) {
        const loops = panel.deck !== null && panel.deck.pad_loops !== undefined
                      ? panel.deck.pad_loops : [];
        const region = slot < loops.length ? loops[slot] : null;
        return region === null || region === undefined ? null : region;
    }

    /// Setting a loop takes two presses: the first marks the start, the second closes it, switches it
    /// on and puts the playhead at its beginning, because a loop that is only drawn is not a loop.
    property bool loopPending: false
    property real loopStartFrame: 0
    /// The region stays here while the loop is switched off, so its edges keep their markers and the
    /// next shift can put it back (DJP-DECK-004).
    property var savedLoop: null

    /// What the strips paint: the loop in force, or the one remembered while it is off.
    readonly property var shownLoop: panel.loop !== null && panel.loop !== undefined
                                     ? panel.loop : panel.savedLoop

    function seekInto(region) {
        if (region === null || region === undefined || region.start === undefined) {
            return;
        }
        Bridge.seek(panel.deckId, region.start);
    }

    function loopContains(region, frame) {
        if (region === null || region === undefined || region.start === undefined) {
            return false;
        }
        return frame >= region.start && frame < region.end;
    }

    /// Every loop the deck is watching in loop mode: the saved ones plus the button's own. A track
    /// holds many loops and one switch arms them all.
    readonly property var activeLoops: panel.deck !== null && panel.deck.active_loops !== undefined
                                       ? panel.deck.active_loops : []

    readonly property bool loopModeOn: panel.activeLoops.length > 0

    /// The region the playhead stands in, out of everything the deck knows: armed loops first, then
    /// the last loop the button made.
    function loopAt(frame) {
        const armed = panel.activeLoops;
        for (let index = 0; index < armed.length; ++index) {
            const region = armed[index];
            if (region.start !== undefined && frame >= region.start && frame < region.end) {
                return region;
            }
        }
        const remembered = panel.savedLoop;
        if (remembered !== null && remembered !== undefined && frame >= remembered.start
                && frame < remembered.end) {
            return remembered;
        }
        return null;
    }

    /// A line in the deck's status when a gesture was refused, so a press that does nothing says why.
    property string loopNotice: ""

    /// Whether a region would sit on a loop that already exists. Loops never overlap: a nested loop
    /// would never play, because the outer one wraps first.
    function loopClashes(from, to) {
        const loops = panel.deck !== null && panel.deck.pad_loops !== undefined
                      ? panel.deck.pad_loops : [];
        const candidates = [];
        for (let slot = 0; slot < loops.length; ++slot) {
            if (loops[slot] !== null && loops[slot] !== undefined) {
                candidates.push(loops[slot]);
            }
        }
        if (panel.loop !== null && panel.loop !== undefined) {
            candidates.push(panel.loop);
        }
        for (let index = 0; index < candidates.length; ++index) {
            const region = candidates[index];
            if (region.start === from && region.end === to) {
                continue;
            }
            if (from < region.end && region.start < to) {
                return true;
            }
        }
        return false;
    }

    /// The Loop button: inside a loop it switches the mode on where the playhead stands; outside it
    /// starts marking another loop. Shift only switches the mode, and never moves anything.
    function pressLoop(cancel) {
        const frame = panel.deck !== null && panel.deck.source_frame !== undefined
                      ? panel.deck.source_frame : 0;

        if (cancel) {
            if (panel.loopPending) {
                panel.loopPending = false;
                return;
            }
            Bridge.setLoopMode(panel.deckId, !panel.loopModeOn);
            return;
        }

        // Inside any loop the deck knows, a press only makes sure the mode is on.
        if (panel.loopAt(frame) !== null) {
            if (!panel.loopModeOn) {
                Bridge.setLoopMode(panel.deckId, true);
            }
            return;
        }

        // Outside them: this press marks the start of another loop, so the mode steps aside while the
        // pair is being made — otherwise an old loop would wrap the playhead mid-gesture.
        if (!panel.loopPending) {
            if (panel.loopModeOn) {
                Bridge.setLoopMode(panel.deckId, false);
            }
            panel.loopPending = true;
            panel.loopStartFrame = frame;
            return;
        }

        const from = Math.min(panel.loopStartFrame, frame);
        const to = Math.max(panel.loopStartFrame, frame);
        panel.loopPending = false;
        if (to - from < 480) {
            panel.loopNotice = qsTr("too short to loop");
            return;
        }
        if (panel.loopClashes(from, to)) {
            panel.loopNotice = qsTr("it would cross another loop");
            return;
        }
        panel.loopNotice = "";
        panel.savedLoop = { "start": from, "end": to };
        Bridge.setLoop(panel.deckId, true, from, to);
        panel.seekInto(panel.savedLoop);
    }

    function hotcueColor(slot, fallback) {
        const list = panel.deck !== null && panel.deck.cue_colors !== undefined
                     ? panel.deck.cue_colors : [];
        const hex = slot < list.length ? list[slot] : null;
        return hex !== null && hex !== undefined && String(hex).length > 0 ? String(hex) : fallback;
    }

    /// The measured correction, in decibels: shown, never applied to a knob.
    readonly property real autoGainDb: {
        const gain = panel.deck !== null && panel.deck.auto_gain !== undefined
                     ? panel.deck.auto_gain : 1.0;
        return gain > 0.0 ? 20.0 * Math.log(gain) / Math.LN10 : 0.0;
    }

    function remainingLabel() {
        return panel.timeLabel(panel.deck.remaining_frames);
    }

    function lengthLabel() {
        if (panel.columns <= 0 || panel.framesPerBucket <= 0) {
            return "—";
        }
        return panel.timeLabel(panel.columns * panel.framesPerBucket);
    }

    readonly property int columns: panel.deck.waveform !== undefined
                                   && panel.deck.waveform.buckets !== undefined
                                   ? panel.deck.waveform.buckets : 0
    readonly property real framesPerBucket: panel.deck.waveform !== undefined
                                            && panel.deck.waveform.frames_per_bucket !== undefined
                                            ? panel.deck.waveform.frames_per_bucket : 0

    // Load once per reference: the snapshot repeats the same path twenty times a second.
    function reload() {
        const reference = panel.deck.waveform !== undefined && panel.deck.waveform.path !== undefined
                          ? String(panel.deck.waveform.path) : "";
        if (reference.length === 0) {
            if (envelope.buckets > 0) {
                envelope.clear();
            }
            return;
        }
        if (envelope.source === reference) {
            return;
        }
        if (!envelope.load(reference)) {
            console.warn("waveform:", envelope.error);
        }
    }

    // The property is `deck`; `onStateChanged` would listen to every Item's built-in state and
    // fire never, which is how the strip lost its picture.
    onDeckChanged: reload()
    Component.onCompleted: reload()

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 4
        spacing: 4

        // One line: which deck it is, what is loaded, where the playhead is in the track.
        RowLayout {
            Layout.fillWidth: true
            spacing: 6

            Label {
                text: panel.deckId
                font.pixelSize: 15
                font.bold: true
                color: panel.playing ? UiSettings.themeAccent : UiSettings.themeText
            }
            Label {
                Layout.fillWidth: true
                text: panel.track !== null && panel.track.title !== undefined
                      ? panel.track.title : qsTr("no track loaded")
                color: panel.track !== null ? UiSettings.themeText : UiSettings.themeTextDim
                font.pixelSize: 12
                elide: Text.ElideRight
            }
            Label {
                objectName: "clock_" + panel.deckId
                // Elapsed first, then what is left: a DJ reads the time that is running out, not the
                // time on the clock (DJP-DECK-002).
                text: panel.positionLabel() + " / −" + panel.remainingLabel()
                Label {
                    objectName: "autogain_" + panel.deckId
                    anchors.left: parent.right
                    anchors.leftMargin: 6
                    anchors.verticalCenter: parent.verticalCenter
                    // Only when there is something to say: a "+0,0 дБ" on every track is noise.
                    visible: Math.abs(panel.autoGainDb) >= 0.5
                    text: (panel.autoGainDb > 0 ? "+" : "") + panel.autoGainDb.toFixed(1) + qsTr(" dB")
                    color: UiSettings.themeTextDim
                    font.pixelSize: 9
                }
                color: UiSettings.themeTextDim
                font.pixelSize: 11
            }
        }

        // The sticks and the fader in one row: the tempo belongs to the deck, and a hand riding the
        // fader has to see the playhead at the same time.
        RowLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            spacing: 4

            PitchFader {
                deckId: panel.deckId
                ratio: panel.deck !== null && panel.deck.tempo_ratio !== undefined
                       ? panel.deck.tempo_ratio : 1.0
                Layout.fillHeight: true
                Layout.preferredWidth: 24
                Layout.minimumWidth: 18
            }

            ColumnLayout {
                Layout.fillWidth: true
                Layout.fillHeight: true
                spacing: 4

                // The zoomed strip: what is around the playhead right now.
                WaveformView {
                    id: detail
                    objectName: "deckDetail_" + panel.deckId
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    Layout.minimumHeight: 40
                    deckState: panel.deck
                    envelope: envelope
                    scrubAudio: panel.scrubAudio
                    shownLoop: panel.shownLoop
                    pendingLoopStart: panel.loopPending ? panel.loopStartFrame : -1
                }

                // The whole track, with a marker that slides along it; a click here seeks.
                TrackOverview {
                    objectName: "deckOverview_" + panel.deckId
                    Layout.fillWidth: true
                    // Twice the detail strip's old overview height: the whole track is easier to aim at for
                    // a seek than a hairline, and the zoomed strip still keeps the most room.
                    Layout.preferredHeight: 44
                    Layout.minimumHeight: 28
                    deckState: panel.deck
                    envelope: envelope
                    shownLoop: panel.shownLoop
                    pendingLoopStart: panel.loopPending ? panel.loopStartFrame : -1
                }
            }
        }

        // Transport, at the bottom where the hand already is.
        RowLayout {
            Layout.fillWidth: true
            spacing: 4

            Button {
                objectName: "play_" + panel.deckId
                text: panel.playing ? qsTr("Pause") : qsTr("Play")
                font.pixelSize: 11
                enabled: panel.live
                // Buttons must not take keyboard focus: Space belongs to the browser, not to
                // whichever control happened to be focused.
                focusPolicy: Qt.NoFocus
                highlighted: panel.playing
                onClicked: Bridge.play(panel.deckId, !panel.playing)
            }
            // The cue button of a transport, not a hotcue button: pressed, it stops the deck on the cue
            // point; held, the deck is silent there; released, it plays from the cue point. That is the
            // gesture every DJ already knows (DJP-DECK-002).
            Button {
                objectName: "cue_" + panel.deckId
                text: qsTr("Cue")
                font.pixelSize: 11
                enabled: panel.live
                focusPolicy: Qt.NoFocus
                onPressed: {
                    Bridge.play(panel.deckId, false);
                    Bridge.seek(panel.deckId, panel.cueFrame());
                }
                onReleased: Bridge.play(panel.deckId, true)
            }
            // The scrub mode of this deck, where the hand already is: silent, or heard as a scratch.
            Button {
                objectName: "scrubMode_" + panel.deckId
                text: panel.scrubAudio ? qsTr("scrub ♪") : qsTr("scrub ␀")
                font.pixelSize: 10
                enabled: panel.live
                focusPolicy: Qt.NoFocus
                highlighted: panel.scrubAudio
                onClicked: panel.scrubAudio = !panel.scrubAudio
            }
            Button {
                objectName: "stop_" + panel.deckId
                text: qsTr("Stop")
                font.pixelSize: 11
                enabled: panel.live
                focusPolicy: Qt.NoFocus
                onClicked: panel.stop()
            }
            Button {
                text: qsTr("Set")
                font.pixelSize: 11
                enabled: panel.live
                focusPolicy: Qt.NoFocus
                onClicked: Bridge.cue(panel.deckId, "Set", 0)
            }
            // A Button's clicked signal carries no mouse, so the shift modifier has to be read where
            // the click happens: this is a flat button drawn like its neighbours.
            Rectangle {
                objectName: "loop_" + panel.deckId
                readonly property bool pending: panel.loopPending
                // The button is lit while loop mode is on, and only then: a loop that is merely
                // remembered has edges on the strip, not a lit button.
                readonly property bool active: panel.loopModeOn
                Layout.preferredWidth: loopLabel.implicitWidth + 14
                Layout.preferredHeight: 22
                radius: 2
                color: panel.loopPending || active ? UiSettings.themeAccent : UiSettings.themeRaised
                border.width: 1
                border.color: UiSettings.themeBorder

                Text {
                    id: loopLabel
                    anchors.centerIn: parent
                    text: panel.loopPending ? qsTr("Loop …") : qsTr("Loop")
                    font.pixelSize: 11
                    color: panel.loopPending || panel.loop != null ? UiSettings.themeTextOnAccent
                                                                  : UiSettings.themeText
                }

                MouseArea {
                    anchors.fill: parent
                    enabled: panel.live
                    acceptedButtons: Qt.LeftButton
                    onClicked: function (mouse) {
                        panel.pressLoop((mouse.modifiers & Qt.ShiftModifier) !== 0);
                    }
                }
            }
            Item { Layout.fillWidth: true }
            Label {
                objectName: "status_" + panel.deckId
                text: panel.statusText()
                color: panel.deck.load !== undefined && panel.deck.load.Failed !== undefined
                       ? UiSettings.themeDanger : UiSettings.themeTextDim
                font.pixelSize: 10
                elide: Text.ElideRight
                Layout.maximumWidth: 140
            }
        }

        // The pads, right under the transport where the hand already is.
        RowLayout {
            objectName: "cuepads_" + panel.deckId
            Layout.fillWidth: true
            spacing: 3

            Repeater {
                model: 8
                delegate: padDelegate
            }
        }
    }

    // What a right click offers: the eight colours, a name and a way to clear the pad.
    Menu {
        id: cueMenu
        property int slot: 0

        MenuItem {
            text: qsTr("Name…")
            onTriggered: {
                rename.slot = cueMenu.slot;
                rename.field.text = panel.hotcueLabel(cueMenu.slot);
                rename.open();
            }
        }
        MenuSeparator {}
        Repeater {
            model: 8
            delegate: MenuItem {
                required property int index
                text: qsTr("Colour %1").arg(index + 1)
                icon.color: panel.cuePalette[index % panel.cuePalette.length]
                onTriggered: Bridge.setCueMeta(panel.deckId, cueMenu.slot, "",
                                               panel.cuePalette[index % panel.cuePalette.length])
            }
        }
        MenuSeparator {}
        MenuItem {
            text: qsTr("Clear")
            onTriggered: Bridge.cue(panel.deckId, "Delete", cueMenu.slot)
        }
    }

    // A pad's name, typed once and then carried with the file.
    Popup {
        id: rename
        property int slot: 0
        anchors.centerIn: Overlay.overlay
        modal: true
        focus: true
        padding: 8
        width: Math.min(320, Overlay.overlay !== null ? Overlay.overlay.width * 0.6 : 320)

        ColumnLayout {
            spacing: 6
            Label {
                text: qsTr("Name for pad %1").arg(rename.slot + 1)
                color: UiSettings.themeText
                font.pixelSize: 11
            }
            TextField {
                id: field
                Layout.fillWidth: true
                font.pixelSize: 11
                selectByMouse: true
                onAccepted: {
                    Bridge.setCueMeta(panel.deckId, rename.slot, field.text, "");
                    rename.close();
                }
            }
            RowLayout {
                spacing: 6
                Item { Layout.fillWidth: true }
                Button {
                    text: qsTr("Cancel")
                    font.pixelSize: 11
                    focusPolicy: Qt.NoFocus
                    onClicked: rename.close()
                }
                Button {
                    text: qsTr("Save")
                    font.pixelSize: 11
                    focusPolicy: Qt.NoFocus
                    onClicked: {
                        Bridge.setCueMeta(panel.deckId, rename.slot, field.text, "");
                        rename.close();
                    }
                }
            }
        }
    }

    // Eight pads, one row: click sets an empty pad or jumps to a set one, shift-click or the menu
    // clears it, and the menu names and colours it. A pad's colour and name travel with the file, so
    // the same cue looks the same tomorrow.
    Component {
        id: padDelegate
        Rectangle {
            id: pad
            required property int index
            objectName: "cuepad_" + panel.deckId + "_" + pad.index
            readonly property var frame: panel.hotcueFrame(pad.index)
            readonly property var loop: panel.padLoop(pad.index)
            readonly property string label: panel.hotcueLabel(pad.index)
            Layout.fillWidth: true
            Layout.preferredHeight: 20
            Layout.minimumWidth: 14
            radius: 2
            // A loop pad is amber and says so with ⟲: the colour is the only thing a glance reads.
            color: pad.loop !== null
                   ? panel.loopColor
                   : (pad.frame !== null
                      ? panel.hotcueColor(pad.index, panel.cuePalette[pad.index % panel.cuePalette.length])
                      : UiSettings.themeRaised)
            border.width: 1
            border.color: UiSettings.themeBorder
            opacity: pad.frame !== null || pad.loop !== null ? 1.0 : 0.7

            Text {
                anchors.fill: parent
                anchors.margins: 1
                text: pad.loop !== null ? "⟲" : (pad.label.length > 0 ? pad.label
                                                                   : String(pad.index + 1))
                font.pixelSize: 8
                font.bold: true
                horizontalAlignment: Text.AlignHCenter
                verticalAlignment: Text.AlignVCenter
                elide: Text.ElideRight
                // Dark on a lit pad, dim on an empty one: a bright pad with white text is a blank.
                color: pad.frame !== null || pad.loop !== null ? "#101010" : UiSettings.themeTextDim
            }

            MouseArea {
                anchors.fill: parent
                acceptedButtons: Qt.LeftButton | Qt.RightButton
                onClicked: function (mouse) {
                    if (!panel.live) {
                        return;
                    }
                    if (mouse.button === Qt.RightButton) {
                        cueMenu.slot = pad.index;
                        cueMenu.popup();
                        return;
                    }
                    if (mouse.modifiers & Qt.ShiftModifier) {
                        Bridge.cue(panel.deckId, "Delete", pad.index);
                    } else if (pad.loop !== null || pad.frame !== null) {
                        Bridge.cue(panel.deckId, "Jump", pad.index);
                    } else if (panel.loop !== null && panel.loop !== undefined) {
                        // An empty pad while a loop is active stores that loop: the four bars become part
                        // of the set, which is what a DJ does between two mixes. The loop lives on the
                        // pad from now on, so the deck's own copy — and its remembered outline — go away.
                        Bridge.setPadLoop(panel.deckId, pad.index);
                        panel.savedLoop = null;
                    } else {
                        Bridge.cue(panel.deckId, "Set", pad.index);
                    }
                }
            }
        }
    }

    function statusText() {
        if (panel.loopNotice.length > 0) {
            return panel.loopNotice;
        }
        const load = panel.deck.load;
        if (load === undefined) {
            return qsTr("empty");
        }
        if (load.Loading !== undefined) {
            return qsTr("preparing… %1%").arg(Math.round((load.Loading.progress || 0) * 100));
        }
        if (load.Failed !== undefined) {
            return qsTr("cannot play: %1").arg(load.Failed.message);
        }
        return qsTr("ready");
    }

}
