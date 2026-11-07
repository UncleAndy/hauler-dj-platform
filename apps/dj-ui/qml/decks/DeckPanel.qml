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
            Button {
                text: qsTr("Loop")
                font.pixelSize: 11
                enabled: panel.live
                focusPolicy: Qt.NoFocus
                checkable: false
                highlighted: panel.loop !== null && panel.loop !== undefined
                onClicked: {
                    const from = panel.deck.source_frame !== undefined ? panel.deck.source_frame : 0;
                    Bridge.setLoop(panel.deckId, panel.loop === null, from, from + 48000);
                }
            }
            Item { Layout.fillWidth: true }
            Label {
                text: panel.statusText()
                color: panel.deck.load !== undefined && panel.deck.load.Failed !== undefined
                       ? UiSettings.themeDanger : UiSettings.themeTextDim
                font.pixelSize: 10
                elide: Text.ElideRight
                Layout.maximumWidth: 140
            }
        }
    }

    function statusText() {
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
