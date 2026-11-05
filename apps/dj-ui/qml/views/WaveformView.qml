// The waveform of a deck, drawn from the cached envelope (DJP-MEDIA-003, DJP-UI-004).
//
// The timebase is source frames: column n covers frames [n*F, (n+1)*F), so the playhead is placed at
// sourceFrame / F / columns * width — no fudge factor, no smoothing that would move the picture away
// from the sound.
import QtQuick
import QtQuick.Controls

Item {
    id: view

    required property var deckState

    readonly property var waveform: deckState !== undefined && deckState.waveform !== undefined
                                    ? deckState.waveform : null
    readonly property int columns: waveform !== null && waveform.buckets !== undefined
                                   ? waveform.buckets : 0
    readonly property real framesPerBucket: waveform !== null && waveform.frames_per_bucket !== undefined
                                            ? waveform.frames_per_bucket : 0
    readonly property real sourceFrame: deckState !== undefined && deckState.source_frame !== undefined
                                        ? deckState.source_frame : 0

    // Load once per reference: the snapshot repeats the same path twenty times a second.
    Connections {
        target: Bridge
        function onSnapshotChanged() { view.reload() }
    }

    function reload() {
        const reference = view.waveform !== null && view.waveform.path !== undefined
                          ? String(view.waveform.path) : "";
        if (reference.length === 0) {
            if (Waveform.buckets > 0) {
                Waveform.clear();
            }
            return;
        }
        if (Waveform.source === reference) {
            return;
        }
        if (!Waveform.load(reference)) {
            console.warn("waveform:", Waveform.error);
        }
    }

    Component.onCompleted: reload()

    Rectangle {
        anchors.fill: parent
        color: "#101317"
        radius: 3

        Canvas {
            id: canvas
            anchors.fill: parent
            anchors.margins: 2

            onPaint: {
                const context = getContext("2d");
                context.reset();
                context.fillStyle = "#101317";
                context.fillRect(0, 0, width, height);

                const middle = height / 2;
                context.strokeStyle = "#242a33";
                context.beginPath();
                context.moveTo(0, middle);
                context.lineTo(width, middle);
                context.stroke();

                const total = Waveform.buckets;
                if (total === 0 || width <= 0) {
                    return;
                }

                // One pixel per column of screen: take the extremes of the buckets it covers, so a
                // three-minute track stays readable without dropping the loudest peaks.
                context.strokeStyle = "#5fa8d3";
                context.beginPath();
                const step = width / total;
                const group = Math.max(1, Math.ceil(total / width));
                for (let start = 0; start < total; start += group) {
                    let low = 0.0;
                    let high = 0.0;
                    for (let index = start; index < Math.min(start + group, total); ++index) {
                        const column = Waveform.columns[index];
                        if (column === undefined) {
                            continue;
                        }
                        low = Math.min(low, column.min);
                        high = Math.max(high, column.max);
                    }
                    const x = start * step + 0.5;
                    context.moveTo(x, middle - high * middle);
                    context.lineTo(x, middle - low * middle);
                }
                context.stroke();

                // Playhead from the engine's own frame count, never from a local timer.
                if (view.framesPerBucket > 0 && view.sourceFrame > 0) {
                    const column = view.sourceFrame / view.framesPerBucket;
                    const x = Math.min(width, column / total * width);
                    context.strokeStyle = "#e2c07f";
                    context.beginPath();
                    context.moveTo(x, 0);
                    context.lineTo(x, height);
                    context.stroke();
                }
            }

            Connections {
                target: Waveform
                function onChanged() { canvas.requestPaint() }
            }
            Connections {
                target: view
                function onSourceFrameChanged() { canvas.requestPaint() }
            }
            onWidthChanged: requestPaint()
            onHeightChanged: requestPaint()
        }

        Label {
            anchors.centerIn: parent
            visible: Waveform.buckets === 0
            text: Waveform.error.length > 0 ? Waveform.error : qsTr("no waveform")
            color: "#5b636d"
            font.pixelSize: 11
        }
    }
}
