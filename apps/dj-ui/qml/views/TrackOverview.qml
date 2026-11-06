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
    // The loop, if one is set: the same region the strip above draws, in miniature.
    readonly property real loopFrom: overview.deckState !== undefined
                                     && overview.deckState.loop_region !== undefined
                                     && overview.deckState.loop_region !== null
                                     && overview.deckState.loop_region.start !== undefined
                                     && overview.framesPerBucket > 0 && overview.columns > 0
                                     ? (overview.deckState.loop_region.start / overview.framesPerBucket)
                                       / overview.columns : -1.0
    readonly property real loopTo: overview.deckState !== undefined
                                   && overview.deckState.loop_region !== undefined
                                   && overview.deckState.loop_region !== null
                                   && overview.deckState.loop_region.end !== undefined
                                   && overview.framesPerBucket > 0 && overview.columns > 0
                                   ? (overview.deckState.loop_region.end / overview.framesPerBucket)
                                     / overview.columns : -1.0

    WaveformStrip {
        id: strip
        anchors.fill: parent
        waveform: overview.envelope
        windowStart: 0.0
        span: 1.0
        playheadRatio: overview.ratio
        hotcues: []
        hasLoop: overview.loopFrom >= 0.0
        loopFrom: Math.max(0.0, overview.loopFrom)
        loopTo: Math.max(0.0, overview.loopTo)

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

    MouseArea {
        anchors.fill: parent
        acceptedButtons: Qt.LeftButton
        onClicked: function (mouse) {
            if (overview.columns <= 0 || overview.framesPerBucket <= 0
                    || !Bridge.connected || overview.deckState === undefined) {
                return;
            }
            const target = Math.max(0.0, Math.min(1.0, mouse.x / Math.max(1, width)));
            const frame = Math.floor(target * overview.columns * overview.framesPerBucket);
            Bridge.seek(overview.deckState.deck, frame);
        }
    }
}
