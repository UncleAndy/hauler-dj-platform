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


    // The window is counted in beats, not in fractions of the track.
    //
    // A fraction of the track is a scale nobody can compare: four minutes and eight minutes at the same
    // fraction show different spans of music, so two decks at one tempo scroll at two speeds and their bars
    // stand at two widths. Counted in beats, the window is `beats · 60 / BPM` seconds of music on both decks,
    // and two synced decks show one picture — the tempo is the whole scale (owner's decision, 2026-11-20).
    readonly property real beatsOnScreen: LayoutState.waveformBeats > 0 ? LayoutState.waveformBeats : 16

    // What the window would be if nothing were known about the music: the fallback the strip opens on
    // before a track is loaded, and the scale the check writes when it wants a known look.
    readonly property real defaultSpan: 1.0 / Math.max(1.0, UiSettings.waveformZoom)

    /// The tempo the deck is heard at: the measurement under the fader.
    readonly property real heardBpm: {
        const state = view.deckState;
        if (state === undefined || state === null || state.local_bpm === undefined
                || !(state.local_bpm > 0)) {
            return 0.0;
        }
        return state.local_bpm * view.tempoRatio;
    }

    /// Seconds of music on screen, and the same in frames of the track. The material walks
    /// `source_rate · tempo_ratio` frames a second, so that is what a wall second is worth here.
    readonly property real spanSeconds: view.heardBpm > 0
                                        ? view.beatsOnScreen * 60.0 / view.heardBpm : 0.0
    readonly property real spanFrames: view.spanSeconds * view.sourceRate * view.tempoRatio

    /// The visible fraction of the track: what the painter and every marker already speak in. It is derived,
    /// never set: the beats are the setting, and this is what they come to on this track.
    readonly property real zoomSpan: {
        if (view.spanFrames > 0 && view.totalFrames > 0) {
            return Math.min(1.0, Math.max(0.0001, view.spanFrames / view.totalFrames));
        }
        return view.defaultSpan;
    }

    // The zoom is a view setting, so it outlives the track: a deck opens at the number of beats it was left
    // at. A track with no measurement yet falls back to the old fraction of the track, which is what
    // `defaultSpan` carries.
    Component.onCompleted: {
        // Nothing to restore: the setting is read straight from the layout state, so a deck opened later
        // sees the same window as the one that was zoomed.
    }

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
    /// The beat grid the deck publishes, as segments of one tempo each (DJP-SYNC-003).
    readonly property var gridSegments: {
        const state = view.deckState;
        // A deck with no track holds `null`, not `undefined`: both mean "there is no grid yet".
        if (state === undefined || state === null || state.track === undefined || state.track === null
                || state.track.grid === undefined || state.track.grid === null
                || state.track.grid.segments === undefined) {
            return [];
        }
        return state.track.grid.segments;
    }
    /// The file's own rate: the grid is in frames of the track, not of the device.
    readonly property real sourceRate: view.deckState !== undefined
                                       && view.deckState.source_rate !== undefined
                                       && view.deckState.source_rate > 0
                                       ? view.deckState.source_rate : 48000

    /// Beats in a bar, as the measurement counted them: four unless the music counts otherwise.
    ///
    /// A waltz is counted in three, and a square of "four bars" is twelve beats there, not sixteen. The
    /// grid carries the number; a deck with no grid, or a grid from a file written before the metre was
    /// measured, is drawn in four.
    readonly property int beatsPerBar: {
        const track = view.deckState !== undefined && view.deckState !== null ? view.deckState.track : null;
        const grid = track !== undefined && track !== null ? track.grid : null;
        const beats = grid !== undefined && grid !== null ? grid.beats_per_bar : undefined;
        return beats !== undefined && beats !== null && beats > 0 ? beats : 4;
    }

    /// The grid as fractions of the track: one entry per beat, a bar every fourth beat. The strip paints
    /// in fractions and this is the only place that knows both the frames and the length of the track.
    readonly property var beatMarks: {
        const marks = [];
        const segments = view.gridSegments;
        const total = view.totalFrames;
        if (segments.length === 0 || total <= 0) {
            return marks;
        }
        const rate = view.sourceRate;
        // A guard, not a limit: a track is a few thousand beats, and a grid that produced more would be
        // a broken tempo rather than a long song.
        const limit = 4000;
        for (let index = 0; index < segments.length && marks.length < limit; ++index) {
            const segment = segments[index];
            const period = rate * 60 / segment.bpm;
            if (!(period > 0)) {
                continue;
            }
            const next = index + 1 < segments.length ? segments[index + 1] : null;
            const until = next !== null ? next.frame : total;
            let frame = segment.frame;
            let beat = segment.beat;
            // The lines run both ways from the anchor, out to the ends of the track: a grid whose first
            // beat sits half a second in still says where the beats before it are, and a grid shifted
            // past the start comes back rebased — the picture must not lose its left side either way.
            if (index === 0 && frame > 0) {
                const back = Math.ceil(frame / period);
                frame -= back * period;
                beat -= back;
            }
            while (frame < until && marks.length < limit) {
                if (frame >= 0 && frame <= total) {
                    marks.push({
                        "ratio": frame / total,
                        "bar": beat % view.beatsPerBar === 0,
                        // Four bars to a square: the count a DJ phrases in, and the only mark that moves
                        // when the grid is shifted by exactly one bar.
                        "square": beat % (4 * view.beatsPerBar) === 0
                    });
                }
                frame += period;
                beat += 1;
            }
        }
        return marks;
    }

    /// Where the tempo changes, as fractions of the track: a grid that changes under the hand has to say
    /// where it changed, or the DJ corrects the wrong part of the track.
    readonly property var anchorMarks: {
        const marks = [];
        const segments = view.gridSegments;
        const total = view.totalFrames;
        if (segments.length < 2 || total <= 0) {
            return marks;
        }
        for (let index = 1; index < segments.length; ++index) {
            const ratio = segments[index].frame / total;
            if (ratio >= 0 && ratio <= 1) {
                marks.push(ratio);
            }
        }
        return marks;
    }

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

    // A hand that stops moving stops making sound: the record stands still and the needle sits in a groove
    // that is not moving. Forty milliseconds is shorter than the gap between two moves of a deliberate
    // drag, so the deck only ever plays what the hand passes over.
    // The hand keeps saying it is still there: the deck treats a quarter of a second of silence as the
    // hand being taken away, and a DJ holds the record still for longer than that all the time.
    Timer {
        id: handBeat
        interval: 100
        repeat: true
        running: view.scrubHeld && view.deckId.length > 0
        onTriggered: Bridge.keepHand(view.deckId)
    }

    Timer {
        id: scrubIdle
        interval: 20
        repeat: false
        onTriggered: {
            if (view.scrubHeld && view.deckId.length > 0) {
                Bridge.setHandSpeed(view.deckId, 0.0);
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

    /// Where the virtual play head of slip stands, as a fraction of the track; negative when slip is off
    /// (DJP-DECK-007). The strip draws it as a dashed line, dimmer than the play head: it is where the
    /// sound will be when slip lets go, not where it is now.
    readonly property real slipRatio: {
        const frame = view.deckState !== undefined ? view.deckState.slip_frame : undefined;
        if (frame === undefined || frame === null) {
            return -1.0;
        }
        return view.ratioOfFrame(frame);
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

    // Double click returns to the default sixteen beats, not to the whole track: the whole track has its
    // own strip below.
    function resetZoom() {
        LayoutState.waveformBeats = 16;
    }

    // Zoom is always about the playhead: it is the centre, so only the number of beats changes.
    function zoomBy(factor) {
        LayoutState.waveformBeats = Math.min(128, Math.max(1, view.beatsOnScreen / factor));
    }

    /// Puts a known window on the strip: the number the settings hold, so a check can ask for a window that
    /// fits inside the track and read what the picture did with it.
    function zoomToBeats(count) {
        if (!(count > 0)) {
            return;
        }
        LayoutState.waveformBeats = Math.min(128, Math.max(1, count));
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
            slipRatio: view.slipRatio
            cues: view.cueMarks
            // The zoomed strip has room for a readable number.
            markFont: 9
            hasLoop: view.loopRatioFrom >= 0
            loopFrom: Math.max(0.0, view.loopRatioFrom)
            loopTo: Math.max(0.0, view.loopRatioTo)
            loopActive: view.loopActive
            pendingFrom: view.pendingRatio
            padLoops: view.padLoopMarks
            beats: view.beatMarks
            gridAnchors: view.anchorMarks

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
                // Named for the checks: the wheel over the strip zooms it, and that is the thing the settings window has to
                // stop while it is open.
                objectName: "waveformHand_" + view.deckId
                anchors.fill: parent
                acceptedButtons: Qt.LeftButton

                // Scrubbing: the strip is the record, the playhead stays in the middle, and the hand
                // moves the material under it. On its own that is a silent seek; with `scrubAudio` the
                // deck plays while the hand moves, so the move is heard as a scratch.
                property bool scrubbing: false
                property real lastX: 0.0
                property real lastMs: 0.0
                property bool wasPlaying: false
                // Where the hand has carried the record, for the silent scrub: the seek target, in source frames.
                property real scrubFrame: 0
                /// The hand takes the record only when it moves: a click is a click, and stopping the
                /// transport for one would be a sound nobody asked for.
                property bool holding: false

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
                    hand.lastMs = Date.now();
                    hand.wasPlaying = view.playing;
                    hand.scrubFrame = view.sourceFrame;
                    view.scrubHeld = true;
                    view.scrubWasPlaying = view.playing;
                    scrubIdle.restart();
                }
                onPositionChanged: function (mouse) {
                    if (!hand.scrubbing) {
                        return;
                    }
                    const now = Date.now();
                    const elapsed = Math.max(1.0, now - hand.lastMs);
                    const deltaPixels = mouse.x - hand.lastX;
                    hand.lastX = mouse.x;
                    hand.lastMs = now;
                    const deltaFrames = -deltaPixels * view.columnsPerPixel * view.framesPerBucket;
                    if (view.scrubAudio) {
                        // Audible: the hand takes the record, and what goes to the deck is a speed — source frames per
                        // second — because that is what a hand has, and the deck turns it into its own rate.
                        if (!hand.holding) {
                            if (deltaPixels === 0) {
                                hand.lastMs = now;
                                return;
                            }
                            // The first move is the touch: the deck stops its motor by its own rule and follows
                            // the hand from here.
                            hand.holding = true;
                            Bridge.putHandOn(view.deckId, true);
                        }
                        const speed = deltaFrames * 1000.0 / elapsed;
                        const limit = 4.0 * view.sourceRate;
                        Bridge.setHandSpeed(view.deckId, Math.max(-limit, Math.min(limit, speed)));
                    } else {
                        // Silent scrub: the playhead follows the hand but the transport is left alone (owner's report,
                        // 25.11). The seek moves the head without starting the deck.
                        hand.scrubFrame += deltaFrames;
                        const total = view.columns * view.framesPerBucket;
                        Bridge.seek(view.deckId,
                                    Math.max(0, Math.min(Math.max(0.0, total - 1),
                                                         Math.round(hand.scrubFrame))));
                    }
                    // The picture follows the hand; the sound is the deck's business now.
                    const total = view.columns * view.framesPerBucket;
                    view.shownFrame = Math.max(0.0, Math.min(Math.max(0.0, total - 1),
                                                             view.shownFrame + deltaFrames));
                    view.anchorFrame = view.shownFrame;
                    view.anchorMilliseconds = now;
                    scrubIdle.restart();
                }
                function endScrub() {
                    if (!hand.scrubbing) {
                        return;
                    }
                    hand.scrubbing = false;
                    view.scrubHeld = false;
                    scrubIdle.stop();
                    // The hand lets go: the motor takes the record back from wherever the hand left it. No
                    // seek — the position is already the one that was heard, and a jump there would be a
                    // sound nobody played.
                    if (hand.holding) {
                        hand.holding = false;
                        Bridge.putHandOn(view.deckId, false);
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
            // Beats are the unit the window is counted in: "16 beats" says more than "3,7×", and two decks
            // at one tempo show the same number here (owner's decision, 2026-11-20).
            text: view.atEnvelopeLimit
                  ? qsTr("%1 beats · envelope limit").arg(view.beatsOnScreen.toFixed(0))
                  : qsTr("%1 beats").arg(view.beatsOnScreen.toFixed(0))
            color: view.atEnvelopeLimit ? UiSettings.themeDanger : UiSettings.themeWaveOutline
            font.pixelSize: 10
        }
    }
}
