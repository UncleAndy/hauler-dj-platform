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
                text: qsTr("Load…")
                enabled: panel.live
                onClicked: loadDialog.open()
            }
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

        // Level and cue indicators (DJP-UI-002): driven by the snapshot, never by the click.
        ColumnLayout {
            Layout.fillWidth: true
            spacing: 2

            RowLayout {
                Layout.fillWidth: true
                spacing: 6
                Label { text: qsTr("L"); opacity: 0.5; font.pixelSize: 10 }
                Rectangle {
                    Layout.fillWidth: true
                    height: 6
                    radius: 2
                    color: "#232830"
                    Rectangle {
                        height: parent.height
                        radius: 2
                        color: panel.playing ? "#7fd67f" : "#4a5a4a"
                        width: parent.width * Math.min(1.0, panel.state.levels !== undefined
                                                        ? panel.state.levels.peak_left : 0.0)
                    }
                }
            }
            RowLayout {
                Layout.fillWidth: true
                spacing: 6
                Label { text: qsTr("R"); opacity: 0.5; font.pixelSize: 10 }
                Rectangle {
                    Layout.fillWidth: true
                    height: 6
                    radius: 2
                    color: "#232830"
                    Rectangle {
                        height: parent.height
                        radius: 2
                        color: panel.playing ? "#7fd67f" : "#4a5a4a"
                        width: parent.width * Math.min(1.0, panel.state.levels !== undefined
                                                        ? panel.state.levels.peak_right : 0.0)
                    }
                }
            }
        }

        RowLayout {
            Layout.fillWidth: true
            spacing: 8

            Label {
                text: qsTr("play")
                font.pixelSize: 10
                color: panel.playing ? "#7fd67f" : "#3a4048"
            }
            Label {
                text: qsTr("cue")
                font.pixelSize: 10
                color: panel.state.hotcues !== undefined
                       && panel.state.hotcues[0] !== null
                       && panel.state.hotcues[0] !== undefined ? "#e2c07f" : "#3a4048"
            }
            Label {
                text: qsTr("loop")
                font.pixelSize: 10
                color: panel.loop !== null && panel.loop !== undefined ? "#7fb0e2" : "#3a4048"
            }
        }

        Label {
            Layout.alignment: Qt.AlignHCenter
            text: {
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
            color: panel.state.load !== undefined && panel.state.load.Failed !== undefined
                   ? "#e2a09b" : "#8a929c"
            font.pixelSize: 11
            elide: Text.ElideRight
            Layout.fillWidth: true
            horizontalAlignment: Text.AlignHCenter
        }

        Label {
            Layout.alignment: Qt.AlignHCenter
            text: qsTr("at %1 · gain %2")
                      .arg(panel.positionLabel())
                      .arg(panel.state.linear_gain !== undefined
                           ? panel.state.linear_gain.toFixed(2) : "—")
            opacity: 0.6
        }
    }

    LoadDialog {
        id: loadDialog
        deckId: panel.deckId
        deckLabel: panel.deckLabel
        onPicked: function (path) { Bridge.load(panel.deckId, path) }
    }
}
