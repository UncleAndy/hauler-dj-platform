// The whole track in one strip, with a marker that slides along it (DJP-UI-004).
//
// This is the second waveform of a deck, as in the reference: the strip above it shows the seconds
// around the playhead, this one shows where those seconds are in the track. A click seeks, which is
// the one thing a full-track strip is for.
import QtQuick
import QtQuick.Controls

Item {
    id: overview

    required property var deckState
    property WaveformEnvelope envelope
    /// Where a loop start has been marked but not yet closed, in source frames; negative means none.
    property real pendingLoopStart: -1

    readonly property real framesPerBucket: deckState !== undefined
                                           && deckState.waveform !== undefined
                                           && deckState.waveform.frames_per_bucket !== undefined
                                           ? deckState.waveform.frames_per_bucket : 0
    readonly property int columns: deckState !== undefined && deckState.waveform !== undefined
                                   && deckState.waveform.buckets !== undefined
                                   ? deckState.waveform.buckets : 0
    readonly property real sourceFrame: deckState !== undefined && deckState.source_frame !== undefined
                                        ? deckState.source_frame : 0
    readonly property real ratio: overview.columns > 0 && overview.framesPerBucket > 0
                                  ? Math.min(1.0, (overview.sourceFrame / overview.framesPerBucket)
                                                  / overview.columns) : 0.0
    /// The track's length in frames, from the deck: markers stay put while the envelope loads.
    readonly property real totalFrames: {
        if (overview.deckState === undefined || overview.deckState.source_frame === undefined) {
            return 0;
        }
        const left = overview.deckState.remaining_frames !== undefined
                     ? overview.deckState.remaining_frames : -1;
        return left >= 0 ? overview.deckState.source_frame + left : 0;
    }
    function loopIsArmed(start, end) {
        const armed = overview.deckState !== undefined && overview.deckState.active_loops !== undefined
                      ? overview.deckState.active_loops : [];
        for (let index = 0; index < armed.length; ++index) {
            const region = armed[index];
            if (region.start === start && region.end === end) {
                return true;
            }
        }
        return false;
    }

    function ratioOfFrame(frame) {
        if (frame === null || frame === undefined) {
            return -1.0;
        }
        return overview.totalFrames > 0 ? frame / overview.totalFrames : -1.0;
    }

    // The loop, if one is set: the same region the strip above draws, in miniature.
    /// Loops stored on pads, as fractions of the whole track, drawn whether or not they are in force.
    readonly property var padLoopMarks: {
        const marks = [];
        const loops = overview.deckState !== undefined && overview.deckState.pad_loops !== undefined
                      ? overview.deckState.pad_loops : [];
        for (let slot = 0; slot < loops.length; ++slot) {
            const region = loops[slot];
            if (region === null || region === undefined || region.start === undefined) {
                continue;
            }
            const from = overview.ratioOfFrame(region.start);
            const to = overview.ratioOfFrame(region.end);
            if (from >= 0.0 && to >= 0.0) {
                marks.push({ "from": from, "to": to,
                             "active": overview.loopIsArmed(region.start, region.end) });
            }
        }
        return marks;
    }

    /// An active loop wins over a remembered one.
    readonly property var paintedLoop: overview.deckState !== undefined
                                       && overview.deckState.loop_region !== undefined
                                       && overview.deckState.loop_region !== null
                                       ? overview.deckState.loop_region : null
    readonly property bool loopActive: overview.deckState !== undefined
                                       && overview.deckState.loop_region !== undefined
                                       && overview.deckState.loop_region !== null
                                       && overview.loopIsArmed(overview.deckState.loop_region.start,
                                                               overview.deckState.loop_region.end)
    readonly property real pendingRatio: overview.pendingLoopStart < 0
                                         ? -1.0 : overview.ratioOfFrame(overview.pendingLoopStart)
    readonly property real loopFrom: overview.deckState !== undefined
                                     && overview.deckState.loop_region !== undefined
                                     && overview.paintedLoop !== null
                                     && overview.paintedLoop !== undefined
                                     && overview.paintedLoop.start !== undefined
                                     ? overview.ratioOfFrame(overview.paintedLoop.start) : -1.0
    readonly property real loopTo: overview.deckState !== undefined
                                   && overview.deckState.loop_region !== undefined
                                   && overview.paintedLoop !== null
                                   && overview.paintedLoop !== undefined
                                   && overview.paintedLoop.end !== undefined
                                   ? overview.ratioOfFrame(overview.paintedLoop.end) : -1.0

    // The pads of the whole track, at the same fractions the strip above uses.
    readonly property var cueMarks: {
        const marks = [];
        const total = overview.columns * overview.framesPerBucket;
        if (total <= 0 || overview.deckState === undefined || overview.deckState.hotcues === undefined) {
            return marks;
        }
        const frames = overview.deckState.hotcues;
        const colors = overview.deckState.cue_colors !== undefined ? overview.deckState.cue_colors : [];
        const labels = overview.deckState.cue_labels !== undefined ? overview.deckState.cue_labels : [];
        for (let slot = 0; slot < frames.length; ++slot) {
            const frame = frames[slot];
            if (frame === null || frame === undefined) {
                continue;
            }
            marks.push({
                ratio: Math.max(0.0, Math.min(1.0, frame / total)),
                color: slot < colors.length && colors[slot] ? String(colors[slot]) : "",
                label: slot < labels.length && labels[slot] ? String(labels[slot]) : ""
            });
        }
        return marks;
    }

    WaveformStrip {
        id: strip
        anchors.fill: parent
        waveform: overview.envelope
        windowStart: 0.0
        span: 1.0
        playheadRatio: overview.ratio
        cues: overview.cueMarks
        // A whole track in a hundred pixels: the number has to be half the size, or it covers the
        // strip instead of naming a point on it.
        markFont: 5
        hasLoop: overview.loopFrom >= 0.0
        loopFrom: Math.max(0.0, overview.loopFrom)
        loopTo: Math.max(0.0, overview.loopTo)
        loopActive: overview.loopActive
        pendingFrom: overview.pendingRatio
        padLoops: overview.padLoopMarks

        coloured: UiSettings.waveformStyle === "coloured"
        barWidth: 1
        barGap: 0
        background: UiSettings.themeWaveBackground
        outside: UiSettings.waveformOutsideColor
        flat: UiSettings.waveformMidColor
        border: UiSettings.themeBorder
        accent: UiSettings.themeAccent
        marker: UiSettings.themeMarker
        playhead: UiSettings.themePlayhead
    }

    // Scrubbing, not just a click: the hand takes the playhead and moves it, and the engine follows while
    // the button is down (DJP-DECK-002). A click is the shortest scrub, so both are the same code.
    MouseArea {
        anchors.fill: parent
        acceptedButtons: Qt.LeftButton
        property bool scrubbing: false

        function moveTo(x) {
            if (overview.columns <= 0 || overview.framesPerBucket <= 0
                    || !Bridge.connected || overview.deckState === undefined) {
                return;
            }
            const target = Math.max(0.0, Math.min(1.0, x / Math.max(1, width)));
            const frame = Math.floor(target * overview.columns * overview.framesPerBucket);
            Bridge.seek(overview.deckState.deck, frame);
        }

        onPressed: function (mouse) {
            scrubbing = true;
            moveTo(mouse.x);
        }
        onPositionChanged: function (mouse) {
            if (scrubbing) {
                moveTo(mouse.x);
            }
        }
        onReleased: scrubbing = false
        onCanceled: scrubbing = false
        // A double click returns the playhead to the start of the track, the second thing a hand wants
        // from a scrubbed bar.
        onDoubleClicked: moveTo(0)
    }
}
