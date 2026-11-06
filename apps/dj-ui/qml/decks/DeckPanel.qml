// A deck panel driven entirely by the engine snapshot (DJP-ARCH-004).
//
// The arrangement follows the reference: the deck's letter and the track's title share one line, the
// zoomed strip takes the room, the whole track sits under it as a thin overview, and the transport
// controls are a row at the bottom. Nothing is optimistic: pressing Play sends a command and waits,
// and the button only reads "playing" once a snapshot says so.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import "../views"

Pane {
    id: panel

    required property string deckId
    required property string deckLabel
    // The deck the browser will load into; the panel shows it instead of leaving the user to guess.
    property bool targeted: false
    signal chosen()

    background: Rectangle {
        color: UiSettings.themePanel
        border.color: panel.targeted ? UiSettings.themeAccent : UiSettings.themeBorder
        border.width: 1
        radius: 2

        MouseArea {
            anchors.fill: parent
            // Behind the controls: a click on empty panel space chooses this deck as the browser's
            // target, a click on a control does what the control says.
            onClicked: panel.chosen()
        }
    }

    readonly property bool live: Bridge.connected
    readonly property var state: {
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

    readonly property bool playing: panel.state.playing === true
    readonly property var track: panel.state.track !== undefined ? panel.state.track : null
    readonly property var loop: panel.state.loop_region !== undefined ? panel.state.loop_region : null

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
        return panel.timeLabel(panel.state.source_frame);
    }

    function lengthLabel() {
        if (panel.columns <= 0 || panel.framesPerBucket <= 0) {
            return "—";
        }
        return panel.timeLabel(panel.columns * panel.framesPerBucket);
    }

    readonly property int columns: panel.state.waveform !== undefined
                                   && panel.state.waveform.buckets !== undefined
                                   ? panel.state.waveform.buckets : 0
    readonly property real framesPerBucket: panel.state.waveform !== undefined
                                            && panel.state.waveform.frames_per_bucket !== undefined
                                            ? panel.state.waveform.frames_per_bucket : 0

    // Load once per reference: the snapshot repeats the same path twenty times a second.
    function reload() {
        const reference = panel.state.waveform !== undefined && panel.state.waveform.path !== undefined
                          ? String(panel.state.waveform.path) : "";
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

    onStateChanged: reload()
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
                color: panel.targeted ? UiSettings.themeAccent : UiSettings.themeText
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
                text: panel.positionLabel() + " / " + panel.lengthLabel()
                color: UiSettings.themeTextDim
                font.pixelSize: 11
            }
        }

        // The zoomed strip: what is around the playhead right now.
        WaveformView {
            id: detail
            objectName: "deckDetail_" + panel.deckId
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.minimumHeight: 48
            deckState: panel.state
            envelope: envelope
        }

        // The whole track, with a marker that slides along it; a click here seeks.
        TrackOverview {
            objectName: "deckOverview_" + panel.deckId
            Layout.fillWidth: true
            Layout.preferredHeight: 22
            Layout.minimumHeight: 14
            deckState: panel.state
            envelope: envelope
        }

        // Transport, at the bottom where the hand already is.
        RowLayout {
            Layout.fillWidth: true
            spacing: 4

            Button {
                text: qsTr("Load")
                font.pixelSize: 11
                enabled: panel.live
                onClicked: loadDialog.open()
            }
            Button {
                text: panel.playing ? qsTr("Pause") : qsTr("Play")
                font.pixelSize: 11
                enabled: panel.live
                highlighted: panel.playing
                onClicked: Bridge.play(panel.deckId, !panel.playing)
            }
            Button {
                text: qsTr("Cue")
                font.pixelSize: 11
                enabled: panel.live
                onClicked: Bridge.cue(panel.deckId, "Jump", 0)
            }
            Button {
                text: qsTr("Set")
                font.pixelSize: 11
                enabled: panel.live
                onClicked: Bridge.cue(panel.deckId, "Set", 0)
            }
            Button {
                text: qsTr("Loop")
                font.pixelSize: 11
                enabled: panel.live
                checkable: false
                highlighted: panel.loop !== null && panel.loop !== undefined
                onClicked: {
                    const from = panel.state.source_frame !== undefined ? panel.state.source_frame : 0;
                    Bridge.setLoop(panel.deckId, panel.loop === null, from, from + 48000);
                }
            }
            Item { Layout.fillWidth: true }
            Label {
                text: panel.statusText()
                color: panel.state.load !== undefined && panel.state.load.Failed !== undefined
                       ? UiSettings.themeDanger : UiSettings.themeTextDim
                font.pixelSize: 10
                elide: Text.ElideRight
                Layout.maximumWidth: 140
            }
        }
    }

    function statusText() {
        const load = panel.state.load;
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

    LoadDialog {
        id: loadDialog
        deckId: panel.deckId
        deckLabel: panel.deckLabel
        onPicked: function (path) { Bridge.load(panel.deckId, path) }
    }
}
