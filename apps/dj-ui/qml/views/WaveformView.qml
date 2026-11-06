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
        return Math.min(1.0, (view.shownFrame / view.framesPerBucket) / view.columns);
    }

    // The engine publishes twenty states a second — the right rate for numbers, far too coarse for
    // a strip that scrolls past the playhead. Between snapshots the position is projected from the
    // last one with the track's own sample rate and tempo, and every new snapshot puts it back on
    // the engine's number. The picture therefore moves on every frame, and it still cannot drift.
    readonly property bool playing: deckState !== undefined && deckState.playing === true
    readonly property real tempoRatio: deckState !== undefined && deckState.tempo_ratio !== undefined
                                       ? deckState.tempo_ratio : 1.0
    property real shownFrame: sourceFrame
    property real anchorFrame: sourceFrame
    property double anchorMilliseconds: 0

    onSourceFrameChanged: {
        view.anchorFrame = view.sourceFrame;
        view.anchorMilliseconds = Date.now();
        view.shownFrame = view.sourceFrame;
    }
    onPlayingChanged: {
        view.anchorFrame = view.sourceFrame;
        view.anchorMilliseconds = Date.now();
    }

    function advance() {
        if (!view.playing || view.tempoRatio <= 0) {
            return;
        }
        const seconds = (Date.now() - view.anchorMilliseconds) / 1000.0;
        const travelled = seconds * envelope.sampleRate * view.tempoRatio;
        const limit = view.columns * view.framesPerBucket;
        view.shownFrame = Math.min(view.anchorFrame + travelled, Math.max(0, limit - 1));
    }

    Timer {
        // The frame rate is a setting (display.frame_hz, default 20): twenty is smooth enough with
        // the projection, and a booth laptop does not have to draw at the display rate.
        interval: UiSettings.frameInterval
        repeat: true
        running: view.playing
        onTriggered: view.advance()
    }
    readonly property real windowStart: playheadRatio - zoomSpan / 2
    readonly property real windowEnd: playheadRatio + zoomSpan / 2

    // The loop as fractions of the track: the strip paints in fractions, so the conversion happens
    // once here instead of per frame inside the painter.
    readonly property real loopRatioFrom: {
        const region = view.loopRegion;
        if (region === null || region.start === undefined || view.framesPerBucket <= 0
                || view.columns <= 0) {
            return -1;
        }
        return (region.start / view.framesPerBucket) / view.columns;
    }
    readonly property real loopRatioTo: {
        const region = view.loopRegion;
        if (region === null || region.end === undefined || view.framesPerBucket <= 0
                || view.columns <= 0) {
            return -1;
        }
        return (region.end / view.framesPerBucket) / view.columns;
    }

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
        color: UiSettings.waveformStyle === "coloured" ? UiSettings.themeWaveBackground : UiSettings.themeSurface
        radius: 3

        // The strip is drawn in C++ (WaveformItem): a fast scroll redraws it every frame, and a
        // QML Canvas cost a map per bar and a script call per rectangle, which is what made the
        // movement feel like a slideshow.
        WaveformStrip {
            id: strip
            anchors.fill: parent
            anchors.margins: 2

            waveform: envelope
            windowStart: view.windowStart
            span: view.zoomSpan
            playheadRatio: view.playheadRatio
            hotcues: view.hotcues
            hasLoop: view.loopRatioFrom >= 0
            loopFrom: Math.max(0.0, view.loopRatioFrom)
            loopTo: Math.max(0.0, view.loopRatioTo)

            coloured: UiSettings.waveformStyle === "coloured"
            barWidth: UiSettings.waveformBarWidth
            barGap: UiSettings.waveformBarGap
            background: UiSettings.waveformStyle === "coloured" ? UiSettings.themeWaveBackground
                                                                : UiSettings.themeSurface
            outside: UiSettings.waveformOutsideColor
            flat: UiSettings.waveformMidColor
            border: UiSettings.themeBorder
            accent: UiSettings.themeAccent
            marker: UiSettings.themeMarker
            playhead: UiSettings.themePlayhead

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
            color: UiSettings.themeWaveOutline
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
            color: view.atEnvelopeLimit ? UiSettings.themeDanger : UiSettings.themeWaveOutline
            font.pixelSize: 10
        }
    }
}
