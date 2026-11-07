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


    // Visible fraction of the track. The default is a tenth — tenfold zoom — so a deck opens on the
    // seconds around the playhead rather than on the whole track; the overview strip below carries
    // the whole track.
    readonly property real defaultSpan: 1.0 / Math.max(1.0, UiSettings.waveformZoom)
    property real zoomSpan: view.defaultSpan

    readonly property real columnsPerPixel: (envelope.buckets * zoomSpan) / Math.max(1, width)
    readonly property bool atEnvelopeLimit: columnsPerPixel < 1.0 && envelope.buckets > 0

    // The deck's envelope, owned by the panel: the zoomed strip and the whole-track overview draw
    // the same columns, instead of parsing the file twice and disagreeing after a reload.
    property WaveformEnvelope envelope

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
    /// The track's length in frames, straight from the deck: a marker placed by frame over this is
    /// right before the envelope has arrived, and an envelope that changes resolution cannot move it.
    readonly property real totalFrames: {
        if (deckState === undefined || deckState.source_frame === undefined) {
            return 0;
        }
        const left = deckState.remaining_frames !== undefined ? deckState.remaining_frames : -1;
        return left >= 0 ? deckState.source_frame + left : 0;
    }

    /// Whether loop mode is watching this region: every saved loop is armed at once, so the strip has
    /// to show which ones repeat right now.
    function loopIsArmed(start, end) {
        const armed = deckState !== undefined && deckState.active_loops !== undefined
                      ? deckState.active_loops : [];
        for (let index = 0; index < armed.length; ++index) {
            const region = armed[index];
            if (region.start === start && region.end === end) {
                return true;
            }
        }
        return false;
    }

    /// Where a source frame lands along the whole track, as a fraction.
    function ratioOfFrame(frame) {
        if (frame === null || frame === undefined) {
            return -1;
        }
        // No total, no marker: an envelope that is still being built reports a growing length, and a
        // mark drawn against it lands somewhere the sound will not be. Nothing is better than wrong.
        return view.totalFrames > 0 ? frame / view.totalFrames : -1;
    }

    readonly property string deckId: deckState !== undefined && deckState.deck !== undefined
                                     ? String(deckState.deck) : ""
    /// Where a loop start has been marked but not yet closed, in source frames; negative means none.
    property real pendingLoopStart: -1

    /// Whether a hand on the strip is heard (a scratch) or only moves the playhead.
    property bool scrubAudio: false

    /// True while the button is down on the strip: the sound of a silent deck comes from the movement
    /// alone, so when the hand stops the deck has to be quiet again.
    property bool scrubHeld: false
    /// The deck was playing before the hand touched the strip.
    property bool scrubWasPlaying: false

    // A hand that stops moving stops making sound. Twenty-eight milliseconds is shorter than the gap
    // between two moves of a deliberate drag, so the deck only ever plays a grain of what the hand
    // passes over — a scratch, not a snippet running on under the playhead.
    Timer {
        id: scrubIdle
        interval: 28
        repeat: false
        onTriggered: {
            if (view.scrubHeld && !view.scrubWasPlaying && view.deckId.length > 0) {
                Bridge.play(view.deckId, false);
            }
        }
    }

    // The pads as the strip paints them: a fraction of the track, a colour and a name. The frame
    // belongs to the deck, the fraction to the strip — the conversion happens once, here.
    readonly property var cueMarks: {
        const marks = [];
        if (view.totalFrames <= 0 && (view.columns * view.framesPerBucket) <= 0) {
            return marks;
        }
        const colors = deckState !== undefined && deckState.cue_colors !== undefined
                       ? deckState.cue_colors : [];
        const labels = deckState !== undefined && deckState.cue_labels !== undefined
                       ? deckState.cue_labels : [];
        for (let slot = 0; slot < view.hotcues.length; ++slot) {
            const frame = view.hotcues[slot];
            if (frame === null || frame === undefined) {
                continue;
            }
            const ratio = view.ratioOfFrame(frame);
            if (ratio < 0) {
                continue;
            }
            marks.push({
                ratio: Math.max(0.0, Math.min(1.0, ratio)),
                color: slot < colors.length && colors[slot] ? String(colors[slot]) : "",
                label: slot < labels.length && labels[slot] ? String(labels[slot]) : ""
            });
        }
        return marks;
    }

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
    /// Every loop stored on a pad, as fractions of the whole track: drawn before anything is pressed,
    /// so the loops a DJ saved yesterday are on the track today.
    readonly property var padLoopMarks: {
        const marks = [];
        const loops = deckState !== undefined && deckState.pad_loops !== undefined
                      ? deckState.pad_loops : [];
        for (let slot = 0; slot < loops.length; ++slot) {
            const region = loops[slot];
            if (region === null || region === undefined || region.start === undefined) {
                continue;
            }
            const from = view.ratioOfFrame(region.start);
            const to = view.ratioOfFrame(region.end);
            if (from >= 0 && to >= 0) {
                marks.push({ "from": from, "to": to,
                             "active": view.loopIsArmed(region.start, region.end) });
            }
        }
        return marks;
    }

    /// The button's own loop, if the deck holds one. Saved loops are painted from the pads instead,
    /// so a deleted loop cannot live on as a ghost the UI draws.
    readonly property var paintedLoop: view.loopRegion
    /// A loop is "active" only while loop mode is watching it: its edges are drawn either way, the fill
    /// is what says whether it will repeat.
    readonly property bool loopActive: view.loopRegion !== null && view.loopRegion !== undefined
                                       && view.loopIsArmed(view.loopRegion.start, view.loopRegion.end)
    readonly property real pendingRatio: view.pendingLoopStart < 0
                                         ? -1 : view.ratioOfFrame(view.pendingLoopStart)
    readonly property real loopRatioFrom: {
        const region = view.paintedLoop;
        if (region === null || region === undefined || region.start === undefined) {
            return -1;
        }
        return view.ratioOfFrame(region.start);
    }
    readonly property real loopRatioTo: {
        const region = view.paintedLoop;
        if (region === null || region === undefined || region.end === undefined) {
            return -1;
        }
        return view.ratioOfFrame(region.end);
    }

    // Where a source frame lands on screen, in pixels; negative ratio means "not a frame".
    function xOfFrame(frame) {
        if (view.framesPerBucket <= 0 || view.columns <= 0 || frame === null || frame === undefined) {
            return -1;
        }
        const ratio = (frame / view.framesPerBucket) / view.columns;
        return ((ratio - view.windowStart) / view.zoomSpan) * width;
    }

    // Double click returns to the default, not to the whole track: the whole track has its own strip.
    function resetZoom() {
        view.zoomSpan = view.defaultSpan;
    }

    // Zoom is always about the playhead: it is the centre, so only the span changes.
    function zoomBy(factor) {
        view.zoomSpan = Math.min(1.0, Math.max(0.0025, view.zoomSpan / factor));
    }



    Rectangle {
        anchors.fill: parent
        color: UiSettings.waveformStyle === "coloured" ? UiSettings.themeWaveBackground : UiSettings.themeSurface
        radius: 3

        // The strip is drawn in C++ (WaveformItem): a fast scroll redraws it every frame, and a
        // QML Canvas cost a map per bar and a script call per rectangle, which is what made the
        // movement feel like a slideshow.
        WaveformStrip {
            id: strip
            objectName: "waveStrip_" + view.deckId
            anchors.fill: parent
            anchors.margins: 2

            waveform: envelope
            windowStart: view.windowStart
            span: view.zoomSpan
            playheadRatio: view.playheadRatio
            cues: view.cueMarks
            // The zoomed strip has room for a readable number.
            markFont: 9
            hasLoop: view.loopRatioFrom >= 0
            loopFrom: Math.max(0.0, view.loopRatioFrom)
            loopTo: Math.max(0.0, view.loopRatioTo)
            loopActive: view.loopActive
            pendingFrom: view.pendingRatio
            padLoops: view.padLoopMarks

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
                id: hand
                anchors.fill: parent
                acceptedButtons: Qt.LeftButton

                // Scrubbing: the strip is the record, the playhead stays in the middle, and the hand
                // moves the material under it. On its own that is a silent seek; with `scrubAudio` the
                // deck plays while the hand moves, so the move is heard as a scratch.
                property bool scrubbing: false
                property real lastX: 0.0
                property bool wasPlaying: false

                onWheel: function (wheel) {
                    view.zoomBy(wheel.angleDelta.y > 0 ? 1.25 : 0.8);
                }
                onDoubleClicked: view.resetZoom()
                onPressed: function (mouse) {
                    if (view.deckId.length === 0 || !Bridge.connected) {
                        return;
                    }
                    hand.scrubbing = true;
                    hand.lastX = mouse.x;
                    hand.wasPlaying = view.playing;
                    view.scrubHeld = true;
                    view.scrubWasPlaying = view.playing;
                    if (view.scrubAudio && !view.playing) {
                        // A held hand on a silent deck makes no sound until it moves; the move below
                        // starts the deck for as long as the movement lasts.
                        scrubIdle.restart();
                    }
                }
                onPositionChanged: function (mouse) {
                    if (!hand.scrubbing) {
                        return;
                    }
                    const deltaPixels = mouse.x - hand.lastX;
                    hand.lastX = mouse.x;
                    // A hand moving right takes the material back: the playhead does not move, the track
                    // does. Frames per pixel comes from the zoom, so the picture and the sound agree.
                    const deltaFrames = -deltaPixels * view.columnsPerPixel * view.framesPerBucket;
                    const total = view.columns * view.framesPerBucket;
                    const target = Math.max(0.0, Math.min(Math.max(0.0, total - 1),
                                                          view.shownFrame + deltaFrames));
                    view.shownFrame = target;
                    view.anchorFrame = target;
                    view.anchorMilliseconds = Date.now();
                    Bridge.seek(view.deckId, Math.floor(target));
                    if (view.scrubAudio && !view.scrubWasPlaying) {
                        // Sound from the hand: the deck plays while it moves, and falls silent when it
                        // stops. Without this a paused deck simply started playing on the first move and
                        // kept running after the hand let go mid-way.
                        if (!view.playing) {
                            Bridge.play(view.deckId, true);
                        }
                        scrubIdle.restart();
                    }
                }
                function endScrub() {
                    if (!hand.scrubbing) {
                        return;
                    }
                    hand.scrubbing = false;
                    view.scrubHeld = false;
                    scrubIdle.stop();
                    if (view.scrubAudio && !hand.wasPlaying) {
                        // The playhead ends where the hand left it: a grain of audio played while the hand
                        // moved, and the position must not keep the distance the grain covered.
                        Bridge.seek(view.deckId, Math.floor(view.shownFrame));
                        Bridge.play(view.deckId, false);
                    }
                }
                onReleased: endScrub()
                onCanceled: endScrub()
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
