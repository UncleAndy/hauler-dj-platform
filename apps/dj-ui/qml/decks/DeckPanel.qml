// A deck panel driven entirely by the engine snapshot (DJP-ARCH-004).
//
// The transport controls are never optimistic: pressing Play sends a command and waits. The button
// only appears pressed once a snapshot says the deck is playing, and it is disabled while the engine
// is unreachable, so a dead engine can never look like a working one.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Pane {
    id: panel

    required property string deckId
    required property string deckLabel

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

    function positionLabel() {
        const frames = panel.state.source_frame;
        if (frames === undefined || frames === null) {
            return qsTr("—");
        }
        const seconds = Math.floor(frames / (Bridge.snapshot.sample_rate || 48000));
        const minutes = Math.floor(seconds / 60);
        return minutes + ":" + ("0" + (seconds % 60)).slice(-2);
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 8

        Label {
            text: panel.deckLabel
            font.pixelSize: 22
            font.bold: true
        }

        Label {
            text: panel.track !== null && panel.track.title !== undefined
                  ? panel.track.title : qsTr("no track loaded")
            opacity: panel.track !== null ? 0.9 : 0.45
            elide: Text.ElideRight
            Layout.fillWidth: true
        }

        RowLayout {
            Layout.alignment: Qt.AlignHCenter
            spacing: 8

            Button {
                text: panel.playing ? qsTr("Pause") : qsTr("Play")
                enabled: panel.live
                checkable: false
                highlighted: panel.playing
                onClicked: Bridge.play(panel.deckId, !panel.playing)
            }
            Button {
                text: qsTr("Cue")
                enabled: panel.live
                onClicked: Bridge.cue(panel.deckId, "Jump", 0)
            }
            Button {
                text: qsTr("Set cue")
                enabled: panel.live
                onClicked: Bridge.cue(panel.deckId, "Set", 0)
            }
            Button {
                text: panel.loop !== null ? qsTr("Loop on") : qsTr("Loop off")
                enabled: panel.live
                checkable: false
                onClicked: {
                    const from = panel.state.source_frame !== undefined ? panel.state.source_frame : 0;
                    Bridge.setLoop(panel.deckId, panel.loop === null, from, from + 48000);
                }
            }
        }

        Item { Layout.fillHeight: true }

        Label {
            Layout.alignment: Qt.AlignHCenter
            text: qsTr("at %1 · gain %2")
                      .arg(panel.positionLabel())
                      .arg(panel.state.linear_gain !== undefined
                           ? panel.state.linear_gain.toFixed(2) : "—")
            opacity: 0.6
        }
    }
}
