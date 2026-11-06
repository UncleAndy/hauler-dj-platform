// The waveform of a deck, drawn from the cached envelope (DJP-MEDIA-003, DJP-UI-004).
//
// The timebase is source frames: column n covers frames [n*F, (n+1)*F), so the playhead, the hotcue
// markers and the loop edges are all placed as frame / F / columns — no fudge factor that would move
// the picture away from the sound.
//
// Zoom re-cuts the same envelope: out, several buckets collapse into a pixel; in, one bucket spans
// several pixels. It never invents detail — a finer envelope is the engine's job, and the deck says
// when the picture is at its limit instead of pretending.
import QtQuick
import QtQuick.Controls

Item {
    id: view

    required property var deckState

    // Visible window over the track, in fractions of its length.
    property real zoomStart: 0.0
    property real zoomSpan: 1.0
    // Columns drawn per pixel before the envelope runs out of detail.
    readonly property real columnsPerPixel: (envelope.buckets * zoomSpan) / Math.max(1, width)
    readonly property bool atEnvelopeLimit: columnsPerPixel < 1.0 && envelope.buckets > 0

    // This deck's envelope, and only this deck's: each view owns one, so loading into A cannot
    // repaint B.
    WaveformEnvelope { id: envelope }

    readonly property var waveform: deckState !== undefined && deckState.waveform !== undefined
                                    ? deckState.waveform : null
    readonly property int columns: waveform !== null && waveform.buckets !== undefined
                                   ? waveform.buckets : 0
    readonly property real framesPerBucket: waveform !== null && waveform.frames_per_bucket !== undefined
                                            ? waveform.frames_per_bucket : 0
    readonly property real sourceFrame: deckState !== undefined && deckState.source_frame !== undefined
                                        ? deckState.source_frame : 0
    readonly property var hotcues: deckState !== undefined && deckState.hotcues !== undefined
                                   ? deckState.hotcues : []
    readonly property var loopRegion: deckState !== undefined && deckState.loop_region !== undefined
                                      ? deckState.loop_region : null

    function resetZoom() {
        view.zoomStart = 0.0;
        view.zoomSpan = 1.0;
    }

    function zoomAt(factor, pivotRatio) {
        const pivot = view.zoomStart + view.zoomSpan * pivotRatio;
        let span = Math.min(1.0, Math.max(0.02, view.zoomSpan / factor));
        let start = pivot - span * pivotRatio;
        start = Math.min(Math.max(0.0, start), 1.0 - span);
        view.zoomSpan = span;
        view.zoomStart = start;
    }

    // Fraction of the track a source frame sits at.
    function ratioOf(frame) {
        if (view.framesPerBucket <= 0 || view.columns <= 0 || frame === null || frame === undefined) {
            return -1;
        }
        return (frame / view.framesPerBucket) / view.columns;
    }

    function xOf(ratio) {
        if (ratio < 0) {
            return -1;
        }
        return ((ratio - view.zoomStart) / view.zoomSpan) * width;
    }

    // Load once per reference: the snapshot repeats the same path twenty times a second.
    Connections {
        target: Bridge
        function onSnapshotChanged() { view.reload() }
    }

    function reload() {
        const reference = view.waveform !== null && view.waveform.path !== undefined
                          ? String(view.waveform.path) : "";
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

                const total = envelope.buckets;
                if (total === 0 || width <= 0) {
                    return;
                }

                // The visible slice of the envelope, in columns. Out, the extremes of a group are
                // kept so a peak never disappears; in, a column becomes a block of pixels with its
                // true extent, so the picture stops short of inventing detail.
                const firstColumn = Math.max(0, Math.floor(total * view.zoomStart));
                const lastColumn = Math.min(total, Math.ceil(total * (view.zoomStart + view.zoomSpan)));
                const visible = Math.max(1, lastColumn - firstColumn);
                const group = Math.max(1, Math.ceil(visible / width));

                context.strokeStyle = "#5fa8d3";
                context.beginPath();
                for (let start = firstColumn; start < lastColumn; start += group) {
                    let low = 0.0;
                    let high = 0.0;
                    for (let index = start; index < Math.min(start + group, lastColumn); ++index) {
                        const column = envelope.columns[index];
                        if (column === undefined) {
                            continue;
                        }
                        low = Math.min(low, column.min);
                        high = Math.max(high, column.max);
                    }
                    const ratio = start / total;
                    const x = view.xOf(ratio) + 0.5;
                    if (x < -1 || x > width + 1) {
                        continue;
                    }
                    context.moveTo(x, middle - high * middle);
                    context.lineTo(x, middle - low * middle);
                }
                context.stroke();

                // The loop, if one is set: a tinted band across the whole height.
                if (view.loopRegion !== null && view.loopRegion.start !== undefined) {
                    const from = view.xOf(view.ratioOf(view.loopRegion.start));
                    const to = view.xOf(view.ratioOf(view.loopRegion.end));
                    if (from >= 0 && to > from) {
                        context.fillStyle = "rgba(127, 176, 226, 0.14)";
                        context.fillRect(from, 0, to - from, height);
                        context.strokeStyle = "#7fb0e2";
                        context.beginPath();
                        context.moveTo(from, 0);
                        context.lineTo(from, height);
                        context.moveTo(to, 0);
                        context.lineTo(to, height);
                        context.stroke();
                    }
                }

                // Hotcues: the engine's frames, so a marker cannot drift from the sound.
                for (let slot = 0; slot < view.hotcues.length; ++slot) {
                    const x = view.xOf(view.ratioOf(view.hotcues[slot]));
                    if (x < 0 || x > width) {
                        continue;
                    }
                    context.fillStyle = "#e2c07f";
                    context.fillRect(x - 1, 0, 2, 6);
                    context.font = "9px sans-serif";
                    context.fillText(String(slot + 1), x + 3, 9);
                }

                // Playhead from the engine's own frame count, never from a local timer.
                const playhead = view.xOf(view.ratioOf(view.sourceFrame));
                if (playhead >= 0 && playhead <= width) {
                    context.strokeStyle = "#f0f3f6";
                    context.beginPath();
                    context.moveTo(playhead, 0);
                    context.lineTo(playhead, height);
                    context.stroke();
                }
            }

            Connections {
                target: envelope
                function onChanged() { canvas.requestPaint() }
            }
            Connections {
                target: view
                function onSourceFrameChanged() { canvas.requestPaint() }
                function onZoomSpanChanged() { canvas.requestPaint() }
                function onZoomStartChanged() { canvas.requestPaint() }
                function onHotcuesChanged() { canvas.requestPaint() }
            }
            onWidthChanged: requestPaint()
            onHeightChanged: requestPaint()

            MouseArea {
                anchors.fill: parent
                acceptedButtons: Qt.LeftButton
                onWheel: function (wheel) {
                    const pivot = Math.min(1.0, Math.max(0.0, wheel.x / Math.max(1, canvas.width)));
                    view.zoomAt(wheel.angleDelta.y > 0 ? 1.25 : 0.8, pivot);
                }
                onDoubleClicked: view.resetZoom()
            }
        }

        Label {
            anchors.centerIn: parent
            visible: envelope.buckets === 0
            text: envelope.error.length > 0 ? envelope.error : qsTr("no waveform")
            color: "#5b636d"
            font.pixelSize: 11
        }

        Label {
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.margins: 4
            visible: view.zoomSpan < 1.0
            text: view.atEnvelopeLimit
                  ? qsTr("zoom %1× · envelope limit").arg((1 / view.zoomSpan).toFixed(1))
                  : qsTr("zoom %1×").arg((1 / view.zoomSpan).toFixed(1))
            color: view.atEnvelopeLimit ? "#e2a09b" : "#5b636d"
            font.pixelSize: 10
        }
    }
}
