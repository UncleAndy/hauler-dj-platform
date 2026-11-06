// The waveform of a deck, drawn from the cached envelope (DJP-MEDIA-003, DJP-UI-004).
//
// The timebase is source frames: column n covers frames [n*F, (n+1)*F). Positions are computed as
// frame / F / columns, the same formula Rust uses, so a marker cannot drift away from the sound.
//
// The view is a scrolling strip: the playback position sits at the horizontal centre and the
// waveform moves under it. Zoom therefore has only one pivot — the playhead — and the regions before
// the start and after the end of the track are filled with their own colour, so a half-empty strip
// reads as "outside the track" rather than as silence in it.
import QtQuick
import QtQuick.Controls

Item {
    id: view

    required property var deckState

    // Visible fraction of the track: 1.0 is the whole track, 0.02 is fiftyfold zoom.
    property real zoomSpan: 1.0

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

    // Fraction of the track the playhead sits at; the strip is always centred on it.
    readonly property real playheadRatio: {
        if (view.framesPerBucket <= 0 || view.columns <= 0) {
            return 0.0;
        }
        return Math.min(1.0, (view.sourceFrame / view.framesPerBucket) / view.columns);
    }
    readonly property real windowStart: playheadRatio - zoomSpan / 2
    readonly property real windowEnd: playheadRatio + zoomSpan / 2

    // Where a source frame lands on screen, in pixels; negative ratio means "not a frame".
    function xOfFrame(frame) {
        if (view.framesPerBucket <= 0 || view.columns <= 0 || frame === null || frame === undefined) {
            return -1;
        }
        const ratio = (frame / view.framesPerBucket) / view.columns;
        return ((ratio - view.windowStart) / view.zoomSpan) * width;
    }

    function resetZoom() {
        view.zoomSpan = 1.0;
    }

    // Zoom is always about the playhead: it is the centre, so only the span changes.
    function zoomBy(factor) {
        view.zoomSpan = Math.min(1.0, Math.max(0.02, view.zoomSpan / factor));
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
        color: UiSettings.waveformStyle === "coloured" ? "#141414" : "#101317"
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
                const total = envelope.buckets;

                // Everything outside the track is its own colour, so it cannot be mistaken for a
                // quiet passage: before the first frame and after the last one.
                const startX = ((0 - view.windowStart) / view.zoomSpan) * width;
                const endX = ((1 - view.windowStart) / view.zoomSpan) * width;
                const outside = UiSettings.waveformOutsideColor;
                if (startX > 0) {
                    context.fillStyle = outside;
                    context.fillRect(0, 0, Math.min(width, startX), height);
                }
                if (endX < width) {
                    context.fillStyle = outside;
                    context.fillRect(Math.max(0, endX), 0, width - Math.max(0, endX), height);
                }

                context.strokeStyle = "#242a33";
                context.beginPath();
                context.moveTo(0, middle);
                context.lineTo(width, middle);
                context.stroke();

                if (total === 0 || width <= 0) {
                    return;
                }

                // The window is split into bars of a fixed width, which is the segmentation step of
                // the reference algorithm: block i covers an equal slice of time and gets one RMS
                // per band, one amplitude and one colour. Integer positions mean no two bars can
                // land on the same pixel, which is what used to produce moiré.
                const barWidth = Math.max(1, UiSettings.waveformBarWidth);
                const barGap = Math.max(0, UiSettings.waveformBarGap);
                const step = barWidth + barGap;
                const barCount = Math.max(1, Math.floor(width / step));
                const bars = envelope.profile(barCount, view.windowStart, view.zoomSpan);
                const coloured = UiSettings.waveformStyle === "coloured";
                const flatColour = UiSettings.waveformMidColor;
                for (let index = 0; index < bars.length; ++index) {
                    const bar = bars[index];
                    // Amplitude is already normalised against the loudest block of the track, so a
                    // quiet track still fills the strip instead of drawing a thin line.
                    // Never skip a bar: silence still draws the centre line, so the waveform stays
                    // one continuous shape instead of splitting into separate vertical strokes.
                    const half = Math.max(0.5, bar.amplitude * middle);
                    const x = index * step;
                    if (!coloured) {
                        context.fillStyle = flatColour;
                        context.fillRect(x, middle - half, barWidth, Math.max(1, half * 2));
                    } else {
                        // Full brightness at the centre axis, about half of it at the tips: the
                        // reference waveform glows from the middle outwards instead of being a flat
                        // block of colour. Three stacked slices give that for the cost of two
                        // extra rectangles per bar.
                        const glow = [0.42, 1.0, 0.42];
                        const base = Qt.color(bar.rgb);
                        for (let slice = 0; slice < 3; ++slice) {
                            const from = middle - half + (2 * half) * slice / 3.0;
                            const to = middle - half + (2 * half) * (slice + 1) / 3.0;
                            context.fillStyle = Qt.rgba(base.r * glow[slice],
                                                        base.g * glow[slice],
                                                        base.b * glow[slice], 1.0);
                            context.fillRect(x, from, barWidth, Math.max(1, to - from));
                        }
                    }
                }
                // The loop, if one is set: a tinted band across the whole height.
                if (view.loopRegion !== null && view.loopRegion.start !== undefined) {
                    const from = view.xOfFrame(view.loopRegion.start);
                    const to = view.xOfFrame(view.loopRegion.end);
                    if (to > from) {
                        const left = Math.max(0, from);
                        const right = Math.min(width, to);
                        if (right > left) {
                            context.fillStyle = "rgba(127, 176, 226, 0.14)";
                            context.fillRect(left, 0, right - left, height);
                        }
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
                context.font = "9px sans-serif";
                for (let slot = 0; slot < view.hotcues.length; ++slot) {
                    const x = view.xOfFrame(view.hotcues[slot]);
                    if (x < 0 || x > width) {
                        continue;
                    }
                    context.fillStyle = "#e2c07f";
                    context.fillRect(x - 1, 0, 2, 6);
                    context.fillText(String(slot + 1), x + 3, 9);
                }

                // The playhead is the centre of the strip by construction; drawn from the engine's
                // own frame count, never from a local timer.
                const playhead = ((view.playheadRatio - view.windowStart) / view.zoomSpan) * width;
                context.strokeStyle = "#f0f3f6";
                context.beginPath();
                context.moveTo(playhead, 0);
                context.lineTo(playhead, height);
                context.stroke();
            }

            Connections {
                target: envelope
                function onChanged() { canvas.requestPaint() }
            }
            Connections {
                target: UiSettings
                function onChanged() { canvas.requestPaint() }
            }
            Connections {
                target: view
                function onSourceFrameChanged() { canvas.requestPaint() }
                function onZoomSpanChanged() { canvas.requestPaint() }
                function onHotcuesChanged() { canvas.requestPaint() }
            }
            onWidthChanged: requestPaint()
            onHeightChanged: requestPaint()

            MouseArea {
                anchors.fill: parent
                acceptedButtons: Qt.LeftButton
                onWheel: function (wheel) {
                    view.zoomBy(wheel.angleDelta.y > 0 ? 1.25 : 0.8);
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
