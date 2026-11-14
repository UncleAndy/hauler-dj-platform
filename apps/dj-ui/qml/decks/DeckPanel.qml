// A deck panel driven entirely by the engine snapshot (DJP-ARCH-004).
//
// The arrangement follows the reference: the deck's letter and the track's title share one line, the
// zoomed strip takes the room, the whole track sits under it as a thin overview, and the transport
// controls are a row at the bottom. There is no load button: tracks come from the browser, into the
// deck's own button there, which is also the only place that has to say "this deck is playing". Nothing is optimistic: pressing Play sends a command and waits,
// and the button only reads "playing" once a snapshot says so.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import "../views"
import "../mixer"
import "../controls"

Pane {
    id: panel
    // Named so a check can press what the DJ presses: the panel owns the tap→grid arithmetic.
    objectName: "deckPanel_" + panel.deckId

    required property string deckId
    required property string deckLabel
    // The deck the browser will load into; the panel shows it instead of leaving the user to guess.
    property bool targeted: false
    signal chosen()

    background: Rectangle {
        objectName: "deckFrame_" + panel.deckId
        color: UiSettings.themePanel
        // The frame answers one question — is this deck playing? A border that marks the browser's
        // target instead sits on one deck for ever and says nothing about the sound.
        border.color: panel.playing ? UiSettings.themeAccent : UiSettings.themeBorder
        border.width: panel.playing ? 2 : 1
        radius: 2

        MouseArea {
            anchors.fill: parent
            // Behind the controls: a click on empty panel space chooses this deck as the browser's
            // target, a click on a control does what the control says.
            onClicked: panel.chosen()
        }
    }

    readonly property bool live: Bridge.connected
    /// Whether a hand on the waveform is heard: a scratch. Silent by default — a booth is not a place
    /// to discover that dragging a strip makes noise.
    property bool scrubAudio: false
    readonly property var deck: {
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

    readonly property bool playing: panel.deck.playing === true
    /// True while the engine is measuring this deck's tempo again (B02).
    readonly property bool analyzing: panel.deck.analyzing === true
    readonly property var track: panel.deck.track !== undefined ? panel.deck.track : null
    /// The grid this deck carries, or null. A track can arrive without one, and a grid that is missing is
    /// not a grid with no segments: reading `segments` off `undefined` throws, and a binding that throws
    /// answers nothing for the rest of the run.
    readonly property var grid: panel.track !== null && panel.track.grid !== undefined ? panel.track.grid : null
    /// Whether there is a grid to edit at all.
    readonly property bool gridReady: panel.grid !== null
    /// The DJ locked this track's grid: no measurement and no edit may move it.
    readonly property bool analysisLock: panel.deck ? panel.deck.analysis_lock === true : false
    // The collection draws the lock in a column that comes from the index, so a press has to come back as
    // rows: the deck's own snapshot is the witness that the write landed, and the list is asked once it does.
    property bool lockPending: false
    property bool lockWanted: false
    /// Grid edits are open only while there is a grid to edit and nobody has locked it.
    readonly property bool gridEditable: panel.gridReady && !panel.analysisLock
                                      && panel.grid.segments !== undefined
                                      && panel.grid.segments.length > 0
    /// The tools that can change the grid at all. They stay live while there is nothing to read yet — a
    /// track without a grid needs its creators — and the lock takes every one of them, creators included.
    readonly property bool gridToolsLive: panel.track !== null && !panel.analysisLock
    /// The file's own rate: a millisecond is `rate / 1000` frames of the track, not of the device.
    readonly property int gridRate: panel.deck.source_rate > 0 ? panel.deck.source_rate : 48000
    /// Taps in the last few seconds, for the tempo the DJ beats out by hand.
    property var taps: []

    /// Move the grid by ten milliseconds: one press is one step, and the ear is the judge.
    /// The tempo the buttons last asked for and the engine has not published yet.
    ///
    /// A snapshot arrives at display rate, so two presses in the same frame both read the tempo from
    /// before the first one — and the second press then asks for the same number, so the tempo moves by
    /// one tenth instead of two. This is not a copy of the state for display: nothing draws it, and it is
    /// dropped the moment the published tempo agrees with it.
    property real pendingBpm: 0
    onLocalBpmChanged: {
        if (panel.pendingBpm > 0 && Math.abs(panel.localBpm - panel.pendingBpm) < 0.05) {
            panel.pendingBpm = 0;
        }
    }
    /// Trim the tempo by a fraction of a beat per minute.
    ///
    /// The base is the tempo under the play head, rounded to a tenth first: a measured 128.00037 plus a
    /// tenth has to read 128.1 and then 128.2, not 128.10037 and 128.20037. A track with no grid has no
    /// tempo at all, and the nudge starts one from the number a deck shows when nothing is measured —
    /// the same 120 the beat button uses.
    function nudgeTempo(delta) {
        const shown = panel.pendingBpm > 0 ? panel.pendingBpm : panel.localBpm;
        const base = shown > 0 ? Math.round(shown * 10) / 10 : 120.0;
        const wanted = Math.round((base + delta) * 10) / 10;
        if (wanted < 20 || wanted > 400) {
            return;
        }
        panel.pendingBpm = wanted;
        Bridge.setGridTempo(panel.deckId, wanted);
    }

    function shiftGrid(direction) {
        Bridge.shiftGrid(panel.deckId, direction * Math.round(panel.gridRate * 0.010));
    }

    /// Move the grid by one beat of the tempo under the play head. Ten milliseconds solve a late kick;
    /// a whole beat solves a grid that starts on the wrong one, and that is a different mistake with a
    /// different button.
    function shiftBeat(direction) {
        if (!(panel.localBpm > 0)) {
            return;
        }
        const beat = Math.round(panel.gridRate * 60 / panel.localBpm);
        Bridge.shiftGrid(panel.deckId, direction * beat);
    }

    /// Put a beat of the grid on the play head. The two mistakes a grid makes are "a few milliseconds
    /// out" and "started on the wrong beat"; this is the third — "right tempo, wrong place here" — and it
    /// is answered by the head itself rather than by counting presses of the arrows.
    function gridHere() {
        const frame = panel.deck.source_frame !== undefined ? panel.deck.source_frame : -1;
        if (frame < 0) {
            return;
        }
        Bridge.snapGrid(panel.deckId, frame);
    }

    /// One tap, kept with the place the music was at when it landed: a tap is a moment of the deck, not
    /// only a moment of the clock, and the grid has to be put where the DJ's hand said it is.
    function tap() {
        const now = Date.now();
        if (panel.taps.length > 0 && now - panel.taps[panel.taps.length - 1].ms > 2000) {
            panel.taps = [];
        }
        const taps = panel.taps.concat([
            { "ms": now, "frame": panel.deck.source_frame !== undefined ? panel.deck.source_frame : 0 }
        ]);
        panel.taps = taps;
        if (taps.length >= 4) {
            panel.applyTaps(taps);
        }
    }

    /// The grid a run of taps describes, or null when they describe nothing.
    ///
    /// The beats per minute are the slope of a line through the tap moments, not a median of gaps: four
    /// taps are four points, and a line through all four is what the DJ actually beat out. The first
    /// beat is where that line says it is — `frame` of each tap, counted back by whole beats — so the
    /// grid lands on the taps instead of next to them.
    function gridFromTaps(taps) {
        const count = taps.length;
        if (count < 2) {
            return null;
        }
        let sumX = 0;
        let sumY = 0;
        for (let index = 0; index < count; ++index) {
            sumX += index;
            sumY += taps[index].ms;
        }
        const meanX = sumX / count;
        const meanY = sumY / count;
        let num = 0;
        let den = 0;
        for (let index = 0; index < count; ++index) {
            const dx = index - meanX;
            num += dx * (taps[index].ms - meanY);
            den += dx * dx;
        }
        if (!(den > 0)) {
            return null;
        }
        const periodMs = num / den;
        if (!(periodMs > 0)) {
            return null;
        }
        const periodFrames = periodMs * panel.gridRate / 1000;
        if (!(periodFrames > 0)) {
            return null;
        }
        // The frame of the first beat: every tap is a beat, so each one back-projects to beat zero, and
        // the average of those is steadier than any single tap.
        let anchor = 0;
        for (let index = 0; index < count; ++index) {
            anchor += taps[index].frame - index * periodFrames;
        }
        anchor /= count;
        return { "periodFrames": periodFrames, "anchor": anchor };
    }

    /// Put the grid where the taps were: the tempo from their slope, the first beat on their line.
    function applyTaps(taps) {
        const fit = panel.gridFromTaps(taps);
        panel.taps = [];
        if (fit === null) {
            return false;
        }
        const bpm = panel.gridRate * 60 / fit.periodFrames;
        Bridge.setGridTempo(panel.deckId, bpm);
        // Then slide it by the shortest way round, so a beat lands on the fitted first tap.
        const first = panel.gridReady ? panel.track.grid.segments[0].frame : 0;
        const period = fit.periodFrames;
        let delta = ((fit.anchor - first) % period + period) % period;
        if (delta > period / 2) {
            delta -= period;
        }
        Bridge.shiftGrid(panel.deckId, Math.round(delta));
        return true;
    }

    /// What this deck follows (DJP-SYNC-002), exactly as the engine reports it: `off`, `tempo` or `beat`.
    readonly property string syncMode: panel.deck !== null && panel.deck.sync !== undefined
                                       ? panel.deck.sync : "off"

    /// Whether this deck is the one the others follow (DJP-SYNC-001).
    readonly property bool isMaster: panel.deck !== null && panel.deck.master === true

    /// Distance from the master's beat in milliseconds: positive means this deck is behind and has to
    /// move forward. Null when the deck follows nothing, which is not the same as being on the beat.
    readonly property bool hasPhase: panel.deck !== null
                                     && panel.deck.phase_error_ms !== undefined
                                     && panel.deck.phase_error_ms !== null
    readonly property real phaseMs: panel.hasPhase ? panel.deck.phase_error_ms : 0.0

    /// How far the needle may swing: half a bar either side. The meter draws the bar, not the beat —
    /// a beat-level offset cannot tell "on the beat" from "on the wrong beat", and on a bar of four that
    /// is a quarter of the music.
    readonly property int barWindowBeats: 2
    readonly property real barErrorBeats: panel.hasPhase && panel.deck.bar_error_beats !== undefined
                                          ? panel.deck.bar_error_beats
                                          : 0.0

    /// The tempo under the play head, falling back to the track's own number for a client that talks to
    /// an engine which does not publish one (DJP-SYNC-003).
    readonly property real localBpm: {
        if (panel.deck.local_bpm !== undefined && panel.deck.local_bpm !== null) {
            return panel.deck.local_bpm;
        }
        return panel.track !== null && panel.track.bpm !== undefined && panel.track.bpm !== null
               ? panel.track.bpm : 0;
    }
    readonly property var loop: panel.deck.loop_region !== undefined ? panel.deck.loop_region : null

    /// Whether this deck's metronome clicks its grid. The state is the engine's: the button draws it and
    /// keeps no copy of its own, or a click the engine refused would still look lit.
    readonly property bool metronome: panel.deck !== undefined && panel.deck !== null
                                      && panel.deck.metronome === true

    /// Beats in a bar, as the measurement counted them: four for almost everything, three for a waltz.
    readonly property int beatsPerBar: {
        const grid = panel.track !== null && panel.track !== undefined ? panel.track.grid : null;
        const beats = grid !== undefined && grid !== null ? grid.beats_per_bar : undefined;
        return beats !== undefined && beats !== null && beats > 0 ? beats : 4;
    }

    /// Which bar of the four-bar square the play head is in. A grid shifted by exactly one bar leaves
    /// every bar line where it was — the lines are periodic — so the count is what tells the DJ that the
    /// shift happened at all.
    readonly property string barInSquare: {
        const beat = panel.deck.beat;
        if (beat === undefined || beat === null) {
            return "";
        }
        const bar = Math.floor(beat / panel.beatsPerBar);
        return "bar " + ((((bar % 4) + 4) % 4) + 1) + "/" + panel.beatsPerBar;
    }

    // One envelope per deck, shared by the zoomed strip and the overview: loading into A can never
    // repaint B, and the two strips cannot disagree about how long the track is.
    WaveformEnvelope { id: envelope }

    function timeLabel(frames) {
        const rate = envelope.sampleRate > 0 ? envelope.sampleRate : 48000;
        if (frames === undefined || frames === null || frames < 0) {
            return "—";
        }
        const seconds = Math.floor(frames / rate);
        const minutes = Math.floor(seconds / 60);
        return minutes + ":" + ("0" + (seconds % 60)).slice(-2);
    }

    function positionLabel() {
        return panel.timeLabel(panel.deck.source_frame);
    }

    /// The point Cue returns to: hotcue 0 when it is set, the start of the file otherwise.
    function cueFrame() {
        const slots = panel.deck.hotcues !== undefined ? panel.deck.hotcues : null;
        if (slots !== null && slots.length > 0 && slots[0] !== null && slots[0] !== undefined) {
            return slots[0];
        }
        return 0;
    }

    // ---- the eight pads (DJP-DECK-003) ----------------------------------------------------
    // Eight colours, readable on a dark panel and told apart from each other in a booth. Not called
    // `palette`: that name belongs to the control's own theme palette, and shadowing it is a warning.
    readonly property var cuePalette: ["#ff4d4d", "#ff9f1a", "#7cf03d", "#00e0c6",
                                    "#3d9bff", "#b07bff", "#ff5cc8", "#ffffff"]
    /// Loops wear yellow and points never do, so a glance tells a loop from a cue point.
    readonly property string loopColor: "#ffd400"

    function hotcueFrame(slot) {
        const cues = panel.deck !== null && panel.deck.hotcues !== undefined ? panel.deck.hotcues : [];
        return slot < cues.length && cues[slot] !== null && cues[slot] !== undefined ? cues[slot] : null;
    }

    function hotcueLabel(slot) {
        const list = panel.deck !== null && panel.deck.cue_labels !== undefined
                     ? panel.deck.cue_labels : [];
        return slot < list.length && list[slot] !== null && list[slot] !== undefined ? list[slot] : "";
    }

    function padLoop(slot) {
        const loops = panel.deck !== null && panel.deck.pad_loops !== undefined
                      ? panel.deck.pad_loops : [];
        const region = slot < loops.length ? loops[slot] : null;
        return region === null || region === undefined ? null : region;
    }

    /// Setting a loop takes two presses: the first marks the start, the second closes it, switches it
    /// on and puts the playhead at its beginning, because a loop that is only drawn is not a loop.
    property bool loopPending: false
    property real loopStartFrame: 0
    function seekInto(region) {
        if (region === null || region === undefined || region.start === undefined) {
            return;
        }
        Bridge.seek(panel.deckId, region.start);
    }

    function loopContains(region, frame) {
        if (region === null || region === undefined || region.start === undefined) {
            return false;
        }
        return frame >= region.start && frame < region.end;
    }

    /// Every loop the deck is watching in loop mode: the saved ones plus the button's own. A track
    /// holds many loops and one switch arms them all.
    readonly property var activeLoops: panel.deck !== null && panel.deck.active_loops !== undefined
                                       ? panel.deck.active_loops : []

    /// Whether loop mode is on. The engine's own flag, not a count of loops: every loop is the same kind
    /// of loop here, and the button has to be lit for the one the size buttons made as well as for a pad's.
    readonly property bool loopModeOn: panel.deck !== null && panel.deck.loop_mode === true

    /// The region the playhead stands in, out of everything the deck knows: armed loops first, then
    /// the last loop the button made.
    function loopAt(frame) {
        // Every loop the deck holds counts, in force or not: one of its pads, or the button's own.
        const known = panel.activeLoops.slice();
        const saved = panel.deck !== null && panel.deck.pad_loops !== undefined
                      ? panel.deck.pad_loops : [];
        for (let slot = 0; slot < saved.length; ++slot) {
            if (saved[slot] !== null && saved[slot] !== undefined) {
                known.push(saved[slot]);
            }
        }
        if (panel.loop !== null && panel.loop !== undefined) {
            known.push(panel.loop);
        }
        for (let index = 0; index < known.length; ++index) {
            const region = known[index];
            if (region.start !== undefined && frame >= region.start && frame < region.end) {
                return region;
            }
        }
        return null;
    }

    /// A line in the deck's status when a gesture was refused, so a press that does nothing says why.
    property string loopNotice: ""

    /// Whether a region would sit on a loop that already exists. Loops never overlap: a nested loop
    /// would never play, because the outer one wraps first.
    function loopClashes(from, to) {
        const loops = panel.deck !== null && panel.deck.pad_loops !== undefined
                      ? panel.deck.pad_loops : [];
        const candidates = [];
        for (let slot = 0; slot < loops.length; ++slot) {
            if (loops[slot] !== null && loops[slot] !== undefined) {
                candidates.push(loops[slot]);
            }
        }
        if (panel.loop !== null && panel.loop !== undefined) {
            candidates.push(panel.loop);
        }
        for (let index = 0; index < candidates.length; ++index) {
            const region = candidates[index];
            if (region.start === from && region.end === to) {
                continue;
            }
            if (from < region.end && region.start < to) {
                return true;
            }
        }
        return false;
    }

    /// The strip of loop sizes and beatjumps is folded to begin with: it costs a row, and a row costs
    /// the wave. The button in the transport row decides (owner's decision, 14.11). Snap and quantize are
    /// not here any more: the two modes are global, and they live in `UiSettings`, where the mixer's
    /// switches and every deck read the one value (owner's decision, 14.11).
    property bool stripOpen: false

    /// The nearest line of this deck's grid, or the frame itself when there is no grid to snap to.
    function onTheGrid(frame) {
        if (!panel.gridReady || !(panel.localBpm > 0)) {
            return frame;
        }
        const period = panel.gridRate * 60 / panel.localBpm;
        const anchor = panel.track.grid.segments[0].frame;
        return Math.round(anchor + Math.round((frame - anchor) / period) * period);
    }

    /// The nearest whole number of beats from `from` to `frame`, in frames.
    function wholeBeatsBetween(from, frame) {
        if (!panel.gridReady || !(panel.localBpm > 0)) {
            return frame;
        }
        const period = panel.gridRate * 60 / panel.localBpm;
        return from + Math.max(1, Math.round((frame - from) / period)) * period;
    }

    /// The Loop button: inside a loop it switches the mode on where the playhead stands; outside it
    /// starts marking another loop. Shift only switches the mode, and never moves anything.
    function pressLoop(cancel) {
        const frame = panel.deck !== null && panel.deck.source_frame !== undefined
                      ? panel.deck.source_frame : 0;

        if (cancel) {
            if (panel.loopPending) {
                panel.loopPending = false;
                return;
            }
            Bridge.setLoopMode(panel.deckId, !panel.loopModeOn);
            return;
        }

        // A pair that is waiting for its second press is always closed by the next press, wherever the
        // playhead stands: leaving it hanging because the playhead happened to be inside another loop is
        // how a press ends up doing nothing at all.
        if (panel.loopPending) {
            const closed = UiSettings.snapGrid ? panel.onTheGrid(frame) : frame;
            const start = UiSettings.snapGrid ? panel.onTheGrid(panel.loopStartFrame) : panel.loopStartFrame;
            const end = UiSettings.quantizeGrid ? panel.wholeBeatsBetween(Math.min(start, closed),
                                                                 Math.max(start, closed))
                                       : Math.max(start, closed);
            const from = Math.round(Math.min(start, closed));
            const to = Math.round(Math.max(end, from + 1));
            panel.loopPending = false;
            if (to - from < 480) {
                panel.loopNotice = qsTr("too short to loop");
                return;
            }
            if (panel.loopClashes(from, to)) {
                panel.loopNotice = qsTr("it would cross another loop");
                return;
            }
            panel.loopNotice = "";
            Bridge.setLoop(panel.deckId, true, from, to);
            panel.seekInto({ "start": from, "end": to });
            return;
        }

        // Inside a loop the button does what the button says: lit, it turns the mode off — that is how a
        // DJ leaves a loop; dark, it arms it again.
        if (panel.loopAt(frame) !== null) {
            Bridge.setLoopMode(panel.deckId, !panel.loopModeOn);
            return;
        }

        // Outside them: this press marks the start of another loop, so the mode steps aside while the
        // pair is being made — otherwise an old loop would wrap the playhead mid-gesture.
        if (panel.loopModeOn) {
            Bridge.setLoopMode(panel.deckId, false);
        }
        panel.loopPending = true;
        panel.loopStartFrame = frame;
    }

    function hotcueColor(slot, fallback) {
        const list = panel.deck !== null && panel.deck.cue_colors !== undefined
                     ? panel.deck.cue_colors : [];
        const hex = slot < list.length ? list[slot] : null;
        return hex !== null && hex !== undefined && String(hex).length > 0 ? String(hex) : fallback;
    }

    /// The measured level correction, in decibels: shown, never applied to a knob.
    ///
    /// `null` means the file's level was never measured, which is a different thing from a correction
    /// of zero and has to read differently too.
    readonly property var autoGainDb: {
        const value = panel.deck.auto_gain_db;
        return value === undefined || value === null ? null : Number(value);
    }

    /// The correction as a person reads it, or an empty string when there is nothing measured.
    // How much of the cache budget this deck's track takes. A size alone says nothing about whether
    // the next track fits; a fraction of the whole does.
    readonly property real cacheBudget: Bridge.snapshot !== undefined
                                        && Bridge.snapshot.cache_budget_bytes !== undefined
                                        ? Number(Bridge.snapshot.cache_budget_bytes) : 0

    function cachedText() {
        const bytes = panel.deck !== null ? Number(panel.deck.cached_bytes || 0) : 0;
        if (bytes <= 0) {
            return "";
        }
        if (panel.cacheBudget > 0) {
            return "RAM " + (100.0 * bytes / panel.cacheBudget).toFixed(1) + "%";
        }
        return "RAM " + (bytes / 1048576).toFixed(0) + " MB";
    }

    // The same number in words, for the tooltip: a percentage without the megabytes behind it is
    // half an answer.
    function cachedHint() {
        const bytes = panel.deck !== null ? Number(panel.deck.cached_bytes || 0) : 0;
        const mb = bytes / 1048576;
        if (panel.cacheBudget > 0) {
            return (mb).toFixed(0) + " MB of " + (panel.cacheBudget / 1048576).toFixed(0)
                   + " MB in memory";
        }
        return mb.toFixed(0) + " MB in memory";
    }

    function autoGainText() {
        if (panel.autoGainDb === null) {
            return "";
        }
        const db = panel.autoGainDb;
        const sign = db > 0 ? "+" : (db < 0 ? "\u2212" : "");
        return sign + Math.abs(db).toFixed(1) + qsTr(" dB");
    }

    function remainingLabel() {
        return panel.timeLabel(panel.deck.remaining_frames);
    }

    function lengthLabel() {
        if (panel.columns <= 0 || panel.framesPerBucket <= 0) {
            return "—";
        }
        return panel.timeLabel(panel.columns * panel.framesPerBucket);
    }

    readonly property int columns: panel.deck.waveform !== undefined
                                   && panel.deck.waveform.buckets !== undefined
                                   ? panel.deck.waveform.buckets : 0
    readonly property real framesPerBucket: panel.deck.waveform !== undefined
                                            && panel.deck.waveform.frames_per_bucket !== undefined
                                            ? panel.deck.waveform.frames_per_bucket : 0

    // Load once per reference: the snapshot repeats the same path twenty times a second.
    function reload() {
        const reference = panel.deck.waveform !== undefined && panel.deck.waveform.path !== undefined
                          ? String(panel.deck.waveform.path) : "";
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

    // The property is `deck`; `onStateChanged` would listen to every Item's built-in state and
    // fire never, which is how the strip lost its picture.
    onDeckChanged: {
        // A new deck means a new track: a tempo asked for on the old one means nothing here.
        panel.pendingBpm = 0;
        // A lock press is confirmed by the engine, not by the button: the collection is asked again the
        // moment the deck carries the flag that was asked for, so the row wears the lock at once.
        // `undefined === false` is false, and an unlocked deck does not carry the field at all: the flag
        // is compared as a boolean, not as a value that may be missing.
        const landed = (panel.deck !== null && panel.deck.analysis_lock === true) === panel.lockWanted;
        if (panel.lockPending && landed) {
            panel.lockPending = false;
            Browser.refreshCollection();
        }
        reload();
    }
    Component.onCompleted: reload()

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 4
        spacing: 4

        // One line: which deck it is, what is loaded, where the playhead is in the track.
        RowLayout {
            Layout.fillWidth: true
            spacing: 6

            Label {
                text: panel.deckId
                font.pixelSize: 15
                font.bold: true
                color: panel.playing ? UiSettings.themeAccent : UiSettings.themeText
            }
            Label {
                Layout.fillWidth: true
                text: panel.track !== null && panel.track.title !== undefined
                      ? panel.track.title : qsTr("no track loaded")
                color: panel.track !== null ? UiSettings.themeText : UiSettings.themeTextDim
                font.pixelSize: 12
                elide: Text.ElideRight
            }
            RowLayout {
                spacing: 6
                Label {
                    objectName: "clock_" + panel.deckId
                    // Elapsed first, then what is left: a DJ reads the time that is running out, not
                    // the time on the clock (DJP-DECK-002).
                    text: panel.positionLabel() + " / −" + panel.remainingLabel()
                    color: UiSettings.themeTextDim
                    font.pixelSize: 11
                }
                Label {
                    objectName: "bpm_" + panel.deckId
                    // The tempo under the play head, one decimal: on a track that holds one tempo this is
                    // the measured number, and on a track that changes tempo it follows the play head
                    // instead of lying about the whole file (B02, DJP-SYNC-003). A grid nobody can see is
                    // a grid nobody trusts, and the deck is where the DJ looks while it is playing.
                    visible: panel.localBpm > 0
                    text: panel.localBpm > 0 ? panel.localBpm.toFixed(1) + " BPM" : ""
                    color: UiSettings.themeAccent
                    font.pixelSize: 10
                }
                Label {
                    objectName: "key_" + panel.deckId
                    // The key sits beside the tempo because those are the two numbers a DJ reads to
                    // pick the next track. Nothing shows until something measured it, and a track
                    // with no key shows nothing rather than an empty slot (DJP-MEDIA-006).
                    visible: panel.track !== null && panel.track.key !== undefined
                             && panel.track.key !== null
                    text: panel.track !== null && panel.track.key !== undefined
                          && panel.track.key !== null ? panel.track.key : ""
                    color: UiSettings.themeText
                    font.pixelSize: 10
                }
                Label {
                    objectName: "bar_" + panel.deckId
                    visible: panel.localBpm > 0
                    text: panel.barInSquare
                    color: UiSettings.themeTextDim
                    font.pixelSize: 10
                }
                // Master and the two ways of following it. The buttons say which mode is in force
                // rather than folding three states into one: a lit SYNC that only matched the tempo
                // would be a button lying about what it does (DJP-SYNC-001/002).
                ToggleButton {
                    objectName: "syncMaster_" + panel.deckId
                    text: qsTr("MASTER")
                    font.pixelSize: 8
                    Layout.preferredWidth: 50
                    Layout.preferredHeight: 16
                    enabled: panel.live && panel.localBpm > 0
                    focusPolicy: Qt.NoFocus
                    lit: panel.isMaster
                    ToolTip.visible: hovered
                    ToolTip.text: panel.isMaster
                                  ? qsTr("Press again to leave the others without a leader")
                                  : qsTr("The deck the others follow")
                    // A master is a state, not a latch: pressing the button that lit it puts it down.
                    // The decks that followed stop following and keep playing as they were.
                    onClicked: panel.isMaster ? Bridge.clearMaster() : Bridge.setMaster(panel.deckId)
                }
                ToggleButton {
                    objectName: "syncBeat_" + panel.deckId
                    text: qsTr("SYNC")
                    font.pixelSize: 8
                    Layout.preferredWidth: 40
                    Layout.preferredHeight: 16
                    enabled: panel.live && panel.localBpm > 0
                    focusPolicy: Qt.NoFocus
                    lit: panel.syncMode === "beat"
                    ToolTip.visible: hovered
                    ToolTip.text: qsTr("Follow the master's beat: same tempo and the same beat")
                    onClicked: Bridge.setSync(panel.deckId,
                                              panel.syncMode === "beat" ? "off" : "beat")
                }
                ToggleButton {
                    objectName: "syncTempo_" + panel.deckId
                    text: qsTr("TEMPO")
                    font.pixelSize: 8
                    Layout.preferredWidth: 44
                    Layout.preferredHeight: 16
                    enabled: panel.live && panel.localBpm > 0
                    focusPolicy: Qt.NoFocus
                    lit: panel.syncMode === "tempo"
                    ToolTip.visible: hovered
                    ToolTip.text: qsTr("Follow the master's tempo and keep this deck's beat where it is")
                    onClicked: Bridge.setSync(panel.deckId,
                                              panel.syncMode === "tempo" ? "off" : "tempo")
                }
                // The phase meter: the button says sync is on, the meter says where the deck stands in
                // the bar. Beat ticks on it, because a bar of four is read as four beats and not as a
                // distance in pixels.
                Item {
                    objectName: "phaseMeter_" + panel.deckId
                    // Shown while a mode is on. A deck that stands still still has a distance to the
                    // master's beat, and it moves with the master: the needle says where this head would
                    // land if the deck were started now.
                    visible: panel.deck !== null && panel.syncMode !== "off"
                    Layout.preferredWidth: 72
                    Layout.preferredHeight: 12
                    Rectangle {
                        anchors.fill: parent
                        radius: 2
                        color: "#20242b"
                        border.color: UiSettings.themeBorder
                        border.width: 1
                    }
                    // The beats of the bar: centre, both quarters and both edges.
                    Repeater {
                        model: 5
                        Rectangle {
                            width: 1
                            height: parent.height
                            opacity: index === 2 ? 0.6 : 0.3
                            color: UiSettings.themeTextDim
                            x: Math.round(index * (parent.width - 1) / 4)
                        }
                    }
                    Rectangle {
                        objectName: "phaseNeedle_" + panel.deckId
                        width: 2
                        height: parent.height - 2
                        y: 1
                        // Clamped to the window: a needle drawn outside the bar would say the size of
                        // the error, and the bar is there to say whether there is one.
                        x: Math.max(1, Math.min(parent.width - 3,
                                                Math.round(parent.width / 2
                                                           + (panel.barErrorBeats / panel.barWindowBeats)
                                                             * (parent.width / 2 - 2))))
                        // Amber while it is off the beat, accent while it is on it: the same language the
                        // loops use for "this is a decision about the beat". The bar says where it is
                        // standing; the colour says whether the beats themselves line up.
                        color: Math.abs(panel.phaseMs) <= 2.0 ? UiSettings.themeAccent : "#ffb300"
                    }
                    ToolTip.visible: hovered
                    // Sync holds the beat; which beat of the bar the deck stands on is the DJ's call,
                    // so the bar is shown and not forced (owner's decision, 2026-11-10).
                    ToolTip.text: panel.playing
                                  ? qsTr("%1 ms from the master's beat, %2 beats along the bar — sync holds the beat, the bar is yours")
                                  : qsTr("This head would land %1 ms from the master's beat, %2 beats along the bar — start it to take the beat")
                                      .arg(panel.phaseMs.toFixed(1))
                                      .arg(panel.barErrorBeats.toFixed(2))
                }
                Label {
                    objectName: "cached_" + panel.deckId
                    // A track the engine keeps in memory can lose its file and keep playing. That is a
                    // property of the deck, not of the file, so it is said here and only when true.
                    visible: panel.deck !== null && panel.deck.cached === true
                    text: panel.cachedText()
                    ToolTip.visible: hovered
                    ToolTip.text: panel.cachedHint()
                    color: UiSettings.themeAccent
                    opacity: 0.7
                    font.pixelSize: 9
                }
                Label {
                    objectName: "autogain_" + panel.deckId
                    // Every measured track says what was done to its level: a DJ who cannot see the
                    // correction cannot trust the fader either. A track nobody measured says nothing.
                    visible: panel.autoGainDb !== null
                    text: panel.autoGainText()
                    // A correction worth a glance is bright; one under half a decibel is a footnote.
                    opacity: panel.autoGainDb !== null && Math.abs(panel.autoGainDb) >= 0.5 ? 1.0 : 0.55
                    color: UiSettings.themeTextDim
                    font.pixelSize: 9
                }
            }
        }

        // The sticks and the fader in one row: the tempo belongs to the deck, and a hand riding the
        // fader has to see the playhead at the same time.
        RowLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            spacing: 4

            PitchFader {
                deckId: panel.deckId
                ratio: panel.deck !== null && panel.deck.tempo_ratio !== undefined
                       ? panel.deck.tempo_ratio : 1.0
                Layout.fillHeight: true
                Layout.preferredWidth: 24
                Layout.minimumWidth: 18
            }

            ColumnLayout {
                Layout.fillWidth: true
                Layout.fillHeight: true
                spacing: 4

                // The zoomed strip: what is around the playhead right now.
                WaveformView {
                    id: detail
                    objectName: "deckDetail_" + panel.deckId
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    Layout.minimumHeight: 40
                    deckState: panel.deck
                    envelope: envelope
                    scrubAudio: panel.scrubAudio
                    pendingLoopStart: panel.loopPending ? panel.loopStartFrame : -1
                }

                // The whole track, with a marker that slides along it; a click here seeks.
                TrackOverview {
                    objectName: "deckOverview_" + panel.deckId
                    Layout.fillWidth: true
                    // Twice the detail strip's old overview height: the whole track is easier to aim at for
                    // a seek than a hairline, and the zoomed strip still keeps the most room.
                    Layout.preferredHeight: 44
                    Layout.minimumHeight: 28
                    deckState: panel.deck
                    envelope: envelope
                    pendingLoopStart: panel.loopPending ? panel.loopStartFrame : -1
                }
            }
        }

        // Transport, at the bottom where the hand already is.
        RowLayout {
            objectName: "transport_" + panel.deckId
            Layout.fillWidth: true
            spacing: 4

            ToggleButton {
                objectName: "play_" + panel.deckId
                // One caption, whatever the transport is doing: the lit face says "this deck is running",
                // and a word that swaps under the hand reads as a different button (owner's decision).
                text: qsTr("Play")
                font.pixelSize: 11
                enabled: panel.live
                // Buttons must not take keyboard focus: Space belongs to the browser, not to
                // whichever control happened to be focused.
                focusPolicy: Qt.NoFocus
                lit: panel.playing
                onClicked: Bridge.play(panel.deckId, !panel.playing)
            }
            // The cue button of a transport, not a hotcue button. A tap keeps the gesture every DJ
            // knows: the head goes back to the cue point and the deck plays on from there. A hold of
            // 400 ms sets the point where the hand landed and leaves the deck standing there — that is
            // what the Set button used to do, and there is no Set button any more (owner's decision).
            Button {
                id: cueButton
                objectName: "cue_" + panel.deckId
                text: qsTr("Cue")
                font.pixelSize: 11
                enabled: panel.live
                focusPolicy: Qt.NoFocus
                // Where the hand landed: the hold sets the point there and not where the deck has run
                // to while the timer waited.
                property double pressFrame: 0
                onPressed: {
                    pressFrame = panel.deck.source_frame !== undefined ? panel.deck.source_frame : 0;
                    cueHold.restart();
                }
                onReleased: {
                    // A quick release: the head goes back to the point and the deck plays on.
                    if (cueHold.running) {
                        cueHold.stop();
                        Bridge.seek(panel.deckId, panel.cueFrame());
                        Bridge.play(panel.deckId, true);
                    }
                    // After a hold the deck stands where the point was set: nothing to do on release.
                }
                Timer {
                    id: cueHold
                    interval: 400
                    repeat: false
                    onTriggered: {
                        Bridge.play(panel.deckId, false);
                        Bridge.seek(panel.deckId, cueButton.pressFrame);
                        Bridge.cue(panel.deckId, "Set", 0);
                    }
                }
            }
            // The scrub mode of this deck, where the hand already is: silent, or heard as a scratch.
            ToggleButton {
                objectName: "scrubMode_" + panel.deckId
                // One caption here too: the lit face says the scrub is heard, and the word stays put.
                text: qsTr("scrub")
                font.pixelSize: 10
                enabled: panel.live
                focusPolicy: Qt.NoFocus
                lit: panel.scrubAudio
                onClicked: panel.scrubAudio = !panel.scrubAudio
            }
            // A Button's clicked signal carries no mouse, so the shift modifier has to be read where
            // the click happens: this is a flat button drawn like its neighbours.
            Rectangle {
                objectName: "loop_" + panel.deckId
                readonly property bool pending: panel.loopPending
                // The button is lit while loop mode is on, and only then: a loop that is merely
                // remembered has edges on the strip, not a lit button.
                readonly property bool active: panel.loopModeOn
                Layout.preferredWidth: loopLabel.implicitWidth + 14
                Layout.preferredHeight: 22
                radius: 2
                color: panel.loopPending || active ? UiSettings.themeAccent : UiSettings.themeRaised
                border.width: 1
                border.color: UiSettings.themeBorder

                Text {
                    id: loopLabel
                    anchors.centerIn: parent
                    // The caption stays put while the press is in flight: the face already lights for
                    // a pending or an active loop, and dots that arrive under the hand widen the button.
                    text: qsTr("Loop")
                    font.pixelSize: 11
                    // The text follows the face: a loop that is only remembered has edges on the strip,
                    // not a lit button, and text meant for the accent reads as a smudge on the raised one.
                    // On the raised face the accent reads as "the loop will land on the grid": the mode is
                    // global now, and its switch is in the mixer, so the deck says what it is doing.
                    color: panel.loopPending || active
                           ? UiSettings.themeTextOnAccent
                           : (UiSettings.snapGrid ? UiSettings.themeAccent : UiSettings.themeText)
                }

                MouseArea {
                    anchors.fill: parent
                    enabled: panel.live
                    acceptedButtons: Qt.LeftButton
                    onClicked: function (mouse) {
                        panel.pressLoop((mouse.modifiers & Qt.ShiftModifier) !== 0);
                    }
                }
            }
            // One button for the strip that holds the loop sizes and the beatjumps: folded there is no
            // row at all and the wave keeps its height; unfolded the strip takes that height and gives
            // it back when it folds. The words stay put, the sign carries the state.
            ToggleButton {
                objectName: "loopJumpToggle_" + panel.deckId
                text: qsTr("Loop/Jump") + (panel.stripOpen ? " ▾" : " ▸")
                font.pixelSize: 10
                enabled: panel.gridReady
                opacity: panel.gridReady ? 1.0 : 0.45
                focusPolicy: Qt.NoFocus
                lit: panel.stripOpen
                ToolTip.visible: hovered
                ToolTip.text: qsTr("Show the loop sizes and the beatjumps of this deck")
                onClicked: panel.stripOpen = !panel.stripOpen
            }
            // Slip wears the same colour as Loop when it is on: a mode has to look like a mode.
            ToggleButton {
                objectName: "slip_" + panel.deckId
                text: qsTr("Slip")
                font.pixelSize: 11
                width: 52
                height: 20
                enabled: panel.live
                focusPolicy: Qt.NoFocus
                lit: panel.deck.slip === true
                ToolTip.visible: hovered
                ToolTip.text: qsTr("The sound may jump or repeat while the track under it keeps "
                                   + "running; leaving a loop puts the sound back on the track's clock")
                onClicked: Bridge.setSlip(panel.deckId, panel.deck.slip !== true)
            }
            // Reverse walks the material backwards from the playhead. It lights like the other modes,
            // and it refuses on a track that is still streaming: the engine says so instead of stalling.
            ToggleButton {
                objectName: "reverse_" + panel.deckId
                text: qsTr("Rev")
                font.pixelSize: 11
                width: 46
                height: 20
                enabled: panel.live
                focusPolicy: Qt.NoFocus
                lit: panel.deck.reverse === true
                ToolTip.visible: hovered
                ToolTip.text: qsTr("Plays the material backwards from the playhead; a track held in "
                                   + "the cache can do it, one still reading from disk cannot")
                onClicked: Bridge.setReverse(panel.deckId, panel.deck.reverse !== true)
            }
            Label {
                objectName: "status_" + panel.deckId
                text: panel.statusText()
                color: panel.deck.load !== undefined && panel.deck.load.Failed !== undefined
                       ? UiSettings.themeDanger : UiSettings.themeTextDim
                font.pixelSize: 10
                elide: Text.ElideRight
                Layout.maximumWidth: 140
            }
            // The pads of this deck, in the row where the hand already is: the wave keeps the height a
            // separate row used to take, and the lock of the grid row stands to their right.
            RowLayout {
                objectName: "cuepads_" + panel.deckId
                Layout.fillWidth: true
                spacing: 3

                Repeater {
                    model: 8
                    delegate: padDelegate
                }
            }
            // The lock of the grid, in the first row and to the right of the pads: it is the one grid control
            // that has to survive the folded panel, because from a locked deck it is the way back (owner's decision).
            ToggleButton {
                objectName: "gridLock_" + panel.deckId
                text: "\uD83D\uDD12"
                lit: panel.analysisLock
                enabled: panel.track !== null
                font.pixelSize: 11
                Layout.preferredWidth: 24
                Layout.preferredHeight: 18
                ToolTip.visible: hovered
                ToolTip.text: qsTr("Lock the grid: no measurement and no edit may move it")
                onClicked: {
                    panel.lockWanted = !panel.analysisLock;
                    panel.lockPending = true;
                    Bridge.setAnalysisLock(panel.deckId, panel.lockWanted);
                }
            }
        }

        // A loop of the music rather than of the hand: the sizes make a loop of whole beats and the arrows
        // move it (DJP-DECK-005). Only a grid knows where a beat is, so without one the row is not there.
        RowLayout {
            objectName: "loopsize_" + panel.deckId
            Layout.fillWidth: true
            // Folded there is no row at all; a track without a grid has no sizes to offer either.
            visible: panel.gridReady && panel.stripOpen
            spacing: 3
            Label {
                objectName: "stripLabel_" + panel.deckId
                text: qsTr("Loop")
                font.pixelSize: 10
                color: UiSettings.themeTextDim
            }
            Repeater {
                model: [1, 2, 4, 8, 16, 32, 64]
                Button {
                    objectName: "loopSize_" + panel.deckId + "_" + modelData
                    text: qsTr("%1").arg(modelData)
                    font.pixelSize: 10
                    Layout.preferredWidth: 26
                    Layout.preferredHeight: 20
                    enabled: panel.live
                    focusPolicy: Qt.NoFocus
                    ToolTip.visible: hovered
                    ToolTip.text: qsTr("Loop %1 beat(s) from the nearest beat").arg(modelData)
                    onClicked: Bridge.loopBeats(panel.deckId, modelData)
                }
            }
            Button {
                objectName: "loopBack_" + panel.deckId
                text: qsTr("‹")
                font.pixelSize: 11
                Layout.preferredWidth: 22
                Layout.preferredHeight: 20
                enabled: panel.live && panel.loop !== null
                focusPolicy: Qt.NoFocus
                ToolTip.visible: hovered
                ToolTip.text: qsTr("Move the loop one beat back")
                onClicked: Bridge.moveLoop(panel.deckId, -1)
            }
            Button {
                objectName: "loopOn_" + panel.deckId
                text: qsTr("›")
                font.pixelSize: 11
                Layout.preferredWidth: 22
                Layout.preferredHeight: 20
                enabled: panel.live && panel.loop !== null
                focusPolicy: Qt.NoFocus
                ToolTip.visible: hovered
                ToolTip.text: qsTr("Move the loop one beat on")
                onClicked: Bridge.moveLoop(panel.deckId, 1)
            }
            Item { Layout.fillWidth: true }
            // Snap puts a loop point on the nearest line of the grid; quantize makes the loop end a whole
            // number of beats after its start. Both are the DJ's switches, and they wear the same colour
            // The beatjumps of the same strip, at the right edge: a jump is measured in beats, so it
            // belongs on the grid the loop sizes do. Twelve lengths do not fit one line at every panel
            // width, so this group wraps instead of squeezing the buttons into unreadable slivers.
            Flow {
                objectName: "jumpGroup_" + panel.deckId
                Layout.preferredWidth: implicitWidth
                Layout.preferredHeight: implicitHeight
                spacing: 3

                Repeater {
                    model: [-64, -32, -16, -8, -4, -1, 1, 4, 8, 16, 32, 64]
                    Button {
                        required property int modelData
                        objectName: "jump_" + panel.deckId + "_" + (modelData > 0 ? "p" : "m")
                                    + Math.abs(modelData)
                        text: (modelData > 0 ? "+" : "−") + Math.abs(modelData)
                        font.pixelSize: 10
                        width: 30
                        height: 20
                        enabled: panel.live
                        focusPolicy: Qt.NoFocus
                        ToolTip.visible: hovered
                        ToolTip.text: UiSettings.quantizeGrid
                                      ? qsTr("%1 beat(s), on the grid").arg(Math.abs(modelData))
                                      : qsTr("%1 beat(s), by time").arg(Math.abs(modelData))
                        onClicked: Bridge.beatJump(panel.deckId, modelData, UiSettings.quantizeGrid)
                    }
                }
            }
        }

        // The grid editor: the measurement is a first answer, and this is where the DJ corrects it. Each
        // press is one edit, written into the track's metadata at once and locked against the next
        // automatic measurement (DJP-SYNC-003).

        RowLayout {
            objectName: "gridedit_" + panel.deckId
            Layout.fillWidth: true
            // A track with no grid keeps the row: the tools that CREATE a grid live here (⌖, BPM…, Tap),
            // and taking them away leaves nothing to measure with; the tools that only read a grid are
            // greyed instead (DJP-MEDIA-006). Under the lock the row is gone entirely — there is nothing
            // to edit, and thirteen dead buttons cost the wave a row (owner's decision, 14.11).
            visible: panel.track !== null && !panel.analysisLock
            Item { Layout.fillWidth: true }
            // The tools of the grid, centred on the deck: the row is a full-width one, and a block of
            // buttons pushed to the left edge leaves the middle of the panel empty (owner's decision).
            RowLayout {
                objectName: "gridBlock_" + panel.deckId
                spacing: 3

                // A beat a press, on the outside of the ten-millisecond arrows: the two mistakes a grid makes
                // are "a few milliseconds late" and "started on the wrong beat", and they need different
                // buttons.
                // The click, first in the row: it is the tool the ear judges everything after it with, and a
                // grid compared against itself proves nothing. Only this deck's grid clicks, so two decks are
                // compared one at a time.
                ToggleButton {
                    objectName: "metronome_" + panel.deckId
                    enabled: panel.gridEditable
                    text: qsTr("♪")
                    font.pixelSize: 9
                    Layout.preferredWidth: 24
                    Layout.preferredHeight: 18
                    focusPolicy: Qt.NoFocus
                    // Lit while it clicks: a reference the DJ cannot see is one they cannot trust.
                    lit: panel.metronome
                    ToolTip.visible: hovered
                    ToolTip.text: qsTr("Click on every beat of this deck's grid")
                    onClicked: Bridge.metronome(panel.deckId, !panel.metronome)
                }
                GridTool {
                    objectName: "gridBeatLeft_" + panel.deckId
                    enabled: panel.gridEditable
                    text: qsTr("⇤")
                    font.pixelSize: 9
                    Layout.preferredWidth: 22
                    Layout.preferredHeight: 18
                    focusPolicy: Qt.NoFocus
                    ToolTip.visible: hovered
                    ToolTip.text: qsTr("Move the grid one beat earlier")
                    onClicked: panel.shiftBeat(-1)
                }
                // Ten milliseconds a press: a grid a whole beat out is rare, and a few milliseconds is
                // what the ear actually hears.
                GridTool {
                    objectName: "gridLeft_" + panel.deckId
                    enabled: panel.gridEditable
                    text: qsTr("◀")
                    font.pixelSize: 9
                    Layout.preferredWidth: 22
                    Layout.preferredHeight: 18
                    focusPolicy: Qt.NoFocus
                    ToolTip.visible: hovered
                    ToolTip.text: qsTr("Move the grid 10 ms earlier")
                    onClicked: panel.shiftGrid(-1)
                }
                // Between the arrows: the grid is put where the play head is, by the head itself.
                // The mark is drawn, not typed: the glyph for a target — ⌖, ⊕ — is set as a dot in most fonts
                // and at nine pixels it reads as one. A ring and a cross at thirteen pixels read as a target.
                GridTool {
                    id: gridHereButton
                    objectName: "gridHere_" + panel.deckId
                    text: ""
                    Layout.preferredWidth: 22
                    Layout.preferredHeight: 18
                    // No padding: the mark is the whole button, and the eighteen pixels of its height are
                    // what makes it a target rather than a dot.
                    padding: 0
                    topPadding: 0
                    bottomPadding: 0
                    leftPadding: 0
                    rightPadding: 0
                    focusPolicy: Qt.NoFocus
                    ToolTip.visible: hovered
                    enabled: panel.gridToolsLive
                    ToolTip.text: qsTr("Put a beat of the grid under the play head")
                    onClicked: panel.gridHere()
                    contentItem: Item {
                        objectName: "gridHereIcon_" + panel.deckId
                        implicitWidth: 10
                        implicitHeight: 10
                        Rectangle {
                            objectName: "gridHereRing_" + panel.deckId
                            anchors.centerIn: parent
                            width: 9
                            height: 9
                            radius: width / 2
                            color: "transparent"
                            border.width: 1
                            border.color: panel.gridToolsLive ? UiSettings.themeText
                                                              : UiSettings.themeTextDim
                        }
                        Rectangle {
                            anchors.centerIn: parent
                            width: 11
                            height: 1
                            color: panel.gridToolsLive ? UiSettings.themeText : UiSettings.themeTextDim
                        }
                        Rectangle {
                            anchors.centerIn: parent
                            width: 1
                            height: 11
                            color: panel.gridToolsLive ? UiSettings.themeText : UiSettings.themeTextDim
                        }
                    }
                }
                GridTool {
                    objectName: "gridRight_" + panel.deckId
                    enabled: panel.gridEditable
                    text: qsTr("▶")
                    font.pixelSize: 9
                    Layout.preferredWidth: 22
                    Layout.preferredHeight: 18
                    focusPolicy: Qt.NoFocus
                    ToolTip.visible: hovered
                    ToolTip.text: qsTr("Move the grid 10 ms later")
                    onClicked: panel.shiftGrid(1)
                }
                GridTool {
                    objectName: "gridBeatRight_" + panel.deckId
                    enabled: panel.gridEditable
                    text: qsTr("⇥")
                    font.pixelSize: 9
                    Layout.preferredWidth: 22
                    Layout.preferredHeight: 18
                    focusPolicy: Qt.NoFocus
                    ToolTip.visible: hovered
                    ToolTip.text: qsTr("Move the grid one beat later")
                    onClicked: panel.shiftBeat(1)
                }
                GridTool {
                    objectName: "gridHalf_" + panel.deckId
                    enabled: panel.gridEditable
                    text: qsTr("½")
                    font.pixelSize: 9
                    Layout.preferredHeight: 18
                    focusPolicy: Qt.NoFocus
                    ToolTip.visible: hovered
                    ToolTip.text: qsTr("Read the grid half as fast")
                    onClicked: Bridge.halveGrid(panel.deckId)
                }
                GridTool {
                    objectName: "gridDouble_" + panel.deckId
                    enabled: panel.gridEditable
                    text: qsTr("×2")
                    font.pixelSize: 9
                    Layout.preferredHeight: 18
                    focusPolicy: Qt.NoFocus
                    ToolTip.visible: hovered
                    ToolTip.text: qsTr("Read the grid twice as fast")
                    onClicked: Bridge.doubleGrid(panel.deckId)
                }
                GridTool {
                    objectName: "gridTempo_" + panel.deckId
                    enabled: panel.gridToolsLive
                    text: qsTr("BPM…")
                    font.pixelSize: 9
                    Layout.preferredHeight: 18
                    focusPolicy: Qt.NoFocus
                    ToolTip.visible: hovered
                    ToolTip.text: qsTr("Type the tempo")
                    onClicked: tempoEdit.open()
                }
                // A tenth of a beat per minute either way: the number a DJ trims by ear against the other
                // deck, where a whole beat is far too much and the arrows below are a different job (they
                // move the grid, these move the tempo).
                GridTool {
                    objectName: "gridTempoDown_" + panel.deckId
                    enabled: panel.gridToolsLive
                    text: qsTr("▼")
                    font.pixelSize: 9
                    Layout.preferredWidth: 18
                    Layout.preferredHeight: 18
                    focusPolicy: Qt.NoFocus
                    ToolTip.visible: hovered
                    ToolTip.text: qsTr("Tempo down by 0.1 BPM")
                    onClicked: panel.nudgeTempo(-0.1)
                }
                GridTool {
                    objectName: "gridTempoUp_" + panel.deckId
                    enabled: panel.gridToolsLive
                    text: qsTr("▲")
                    font.pixelSize: 9
                    Layout.preferredWidth: 18
                    Layout.preferredHeight: 18
                    focusPolicy: Qt.NoFocus
                    ToolTip.visible: hovered
                    ToolTip.text: qsTr("Tempo up by 0.1 BPM")
                    onClicked: panel.nudgeTempo(0.1)
                }
                GridTool {
                    objectName: "gridTap_" + panel.deckId
                    enabled: panel.gridToolsLive
                    text: qsTr("Tap")
                    font.pixelSize: 9
                    Layout.preferredHeight: 18
                    focusPolicy: Qt.NoFocus
                    ToolTip.visible: hovered
                    ToolTip.text: qsTr("Tap the tempo: the grid is put on your taps, from the fourth one")
                    onClicked: panel.tap()
                }
                // The tempo the deck shows was measured once, at load. When the answer is wrong the DJ
                // asks again — a full decode, so it is asked for, never done behind their back, and a
                // confirmation stands between the click and the new grid (B02).
                GridTool {
                    objectName: "tempo_" + panel.deckId
                    text: qsTr("Analyze")
                    font.pixelSize: 10
                    // A second question while the first is being answered would queue another decode of
                    // the same file for the same answer; a locked grid is the DJ's decision, and a
                    // measurement would rewrite it. The face goes with the state: a tool that only refuses
                    // clicks is a tool the hand keeps trying.
                    enabled: panel.live && !panel.analyzing && !panel.analysisLock
                    focusPolicy: Qt.NoFocus
                    ToolTip.visible: hovered
                    ToolTip.text: panel.analysisLock
                                  ? qsTr("The grid is locked: unlock it to measure this track again")
                                  : qsTr("Analyse this track again: measure its tempo and rebuild the grid")
                    onClicked: tempoConfirm.open()
                }
                Label {
                    objectName: "tapHint_" + panel.deckId
                    visible: panel.taps.length > 0
                    text: qsTr("%1 tap(s)").arg(panel.taps.length)
                    color: UiSettings.themeTextDim
                    font.pixelSize: 9
                }
            }
            Item { Layout.fillWidth: true }
        }


    }

    // A small window over the deck while its tempo is measured again: a decode of a five-minute file
    // takes seconds, and a button that answers nothing reads as a button that did nothing. Not modal —
    // the music keeps playing and the hand keeps working (B02).
    Rectangle {
        objectName: "analyzing_" + panel.deckId
        visible: panel.analyzing
        anchors.top: parent.top
        anchors.right: parent.right
        anchors.margins: 10
        z: 100
        width: analyzeRow.implicitWidth + 18
        height: 26
        radius: 3
        color: UiSettings.themeRaised
        border.width: 1
        border.color: UiSettings.themeAccent

        RowLayout {
            id: analyzeRow
            anchors.centerIn: parent
            spacing: 6

            BusyIndicator {
                running: panel.analyzing
                visible: panel.analyzing
                Layout.preferredWidth: 16
                Layout.preferredHeight: 16
            }
            Label {
                text: qsTr("Analyze…")
                color: UiSettings.themeText
                font.pixelSize: 11
            }
        }
    }

    // What a right click offers: the eight colours, a name and a way to clear the pad.
    Menu {
        id: cueMenu
        property int slot: 0

        MenuEntry {
            text: qsTr("Name…")
            onTriggered: {
                rename.slot = cueMenu.slot;
                rename.field.text = panel.hotcueLabel(cueMenu.slot);
                rename.open();
            }
        }
        MenuSeparator {}
        Repeater {
            model: 8
            delegate: MenuEntry {
                required property int index
                text: qsTr("Colour %1").arg(index + 1)
                icon.color: panel.cuePalette[index % panel.cuePalette.length]
                onTriggered: Bridge.setCueMeta(panel.deckId, cueMenu.slot, "",
                                               panel.cuePalette[index % panel.cuePalette.length])
            }
        }
        MenuSeparator {}
        MenuEntry {
            text: qsTr("Clear")
            onTriggered: Bridge.cue(panel.deckId, "Delete", cueMenu.slot)
        }
    }

    // The second answer to the same question: measure this track again and replace its grid.
    Popup {
        id: tempoConfirm
        objectName: "tempoConfirm_" + panel.deckId
        anchors.centerIn: Overlay.overlay
        modal: true
        focus: true
        padding: 10
        // A question, not a paragraph: the width is fixed and the sentence wraps inside it, instead of
        // the window growing to the length of the text.
        width: Math.min(300, Overlay.overlay !== null ? Overlay.overlay.width - 40 : 300)
        contentWidth: width - 2 * padding

        ColumnLayout {
            spacing: 6
            Layout.fillWidth: true
            Label {
                objectName: "tempoText_" + panel.deckId
                Layout.fillWidth: true
                Layout.preferredWidth: tempoConfirm.contentWidth
                wrapMode: Text.WordWrap
                text: panel.track !== null && panel.track.bpm
                      ? qsTr("Measure the tempo again? Deck %1 now says %2 BPM; the new measurement "
                             + "replaces it and is written into the track's metadata.")
                            .arg(panel.deckId).arg(panel.track.bpm.toFixed(1))
                      : qsTr("Measure the tempo again? Deck %1 has no tempo yet; the measurement "
                             + "will be written into the track's metadata.").arg(panel.deckId)
                color: UiSettings.themeText
                font.pixelSize: 11
            }
            RowLayout {
                spacing: 6
                Item { Layout.fillWidth: true }
                Button {
                    objectName: "tempoCancel_" + panel.deckId
                    text: qsTr("Cancel")
                    font.pixelSize: 11
                    focusPolicy: Qt.NoFocus
                    onClicked: tempoConfirm.close()
                }
                Button {
                    objectName: "tempoGo_" + panel.deckId
                    text: qsTr("Measure")
                    font.pixelSize: 11
                    focusPolicy: Qt.NoFocus
                    onClicked: {
                        Bridge.analyzeTempo(panel.deckId);
                        tempoConfirm.close();
                    }
                }
            }
        }
    }

    // The tempo typed by hand: the number the DJ knows, instead of the one the detector guessed.
    Popup {
        id: tempoEdit
        objectName: "tempoEdit_" + panel.deckId
        anchors.centerIn: Overlay.overlay
        modal: true
        focus: true
        padding: 10
        // Measured from the panel, not from the overlay: the overlay is not laid out when the popup is
        // created, and `overlay.width - 40` was a negative width for that instant — the content was then
        // laid out at its own width and ran out of the window on the right. The panel is laid out.
        width: Math.max(220, Math.min(280, panel.width - 20))
        contentWidth: width - 2 * padding

        onOpened: {
            tempoField.text = panel.localBpm > 0 ? panel.localBpm.toFixed(1) : "";
            tempoField.forceActiveFocus();
            tempoField.selectAll();
        }

        ColumnLayout {
            // The column is as wide as the popup, and the text wraps inside it: `Layout.fillWidth` on a
            // column that has no width of its own does nothing, and the label kept its one-line width.
            width: tempoEdit.availableWidth
            spacing: 6
            Label {
                objectName: "tempoEditLabel_" + panel.deckId
                Layout.fillWidth: true
                Layout.maximumWidth: tempoEdit.availableWidth
                wrapMode: Text.WordWrap
                text: qsTr("Tempo of deck %1, in beats per minute. The first beat stays where it is.")
                        .arg(panel.deckId)
                color: UiSettings.themeText
                font.pixelSize: 11
            }
            TextField {
                id: tempoField
                objectName: "tempoField_" + panel.deckId
                Layout.fillWidth: true
                font.pixelSize: 11
                selectByMouse: true
                // Both separators, because the field is filled with a dot (`128.0`) and a `DoubleValidator`
                // accepts only the one its locale uses — in a comma locale the number the dialog itself
                // wrote was refused. The range is checked on apply, where it can say no.
                validator: RegularExpressionValidator {
                    regularExpression: /^\d{0,3}([.,]\d{0,3})?$/
                }
                onAccepted: {
                    // Either separator: a DJ types 128.0 or 128,0 and means the same thing.
                    Bridge.setGridTempo(panel.deckId, parseFloat(text.replace(",", ".")));
                    tempoEdit.close();
                }
            }
            RowLayout {
                spacing: 6
                Item { Layout.fillWidth: true }
                Button {
                    text: qsTr("Cancel")
                    font.pixelSize: 11
                    focusPolicy: Qt.NoFocus
                    onClicked: tempoEdit.close()
                }
                Button {
                    objectName: "tempoApply_" + panel.deckId
                    text: qsTr("Apply")
                    font.pixelSize: 11
                    focusPolicy: Qt.NoFocus
                    onClicked: {
                        const typed = parseFloat(tempoField.text.replace(",", "."));
                        if (typed >= 20 && typed <= 400) {
                            Bridge.setGridTempo(panel.deckId, typed);
                        }
                        tempoEdit.close();
                    }
                }
            }
        }
    }

    // A pad's name, typed once and then carried with the file.
    Popup {
        id: rename
        property int slot: 0
        anchors.centerIn: Overlay.overlay
        modal: true
        focus: true
        padding: 8
        width: Math.min(320, Overlay.overlay !== null ? Overlay.overlay.width * 0.6 : 320)

        ColumnLayout {
            spacing: 6
            Label {
                text: qsTr("Name for pad %1").arg(rename.slot + 1)
                color: UiSettings.themeText
                font.pixelSize: 11
            }
            TextField {
                id: field
                Layout.fillWidth: true
                font.pixelSize: 11
                selectByMouse: true
                onAccepted: {
                    Bridge.setCueMeta(panel.deckId, rename.slot, field.text, "");
                    rename.close();
                }
            }
            RowLayout {
                spacing: 6
                Item { Layout.fillWidth: true }
                Button {
                    text: qsTr("Cancel")
                    font.pixelSize: 11
                    focusPolicy: Qt.NoFocus
                    onClicked: rename.close()
                }
                Button {
                    text: qsTr("Save")
                    font.pixelSize: 11
                    focusPolicy: Qt.NoFocus
                    onClicked: {
                        Bridge.setCueMeta(panel.deckId, rename.slot, field.text, "");
                        rename.close();
                    }
                }
            }
        }
    }

    // Eight pads, one row: click sets an empty pad or jumps to a set one, shift-click or the menu
    // clears it, and the menu names and colours it. A pad's colour and name travel with the file, so
    // the same cue looks the same tomorrow.
    Component {
        id: padDelegate
        Rectangle {
            id: pad
            required property int index
            objectName: "cuepad_" + panel.deckId + "_" + pad.index
            readonly property var frame: panel.hotcueFrame(pad.index)
            readonly property var loop: panel.padLoop(pad.index)
            readonly property string label: panel.hotcueLabel(pad.index)
            Layout.fillWidth: true
            Layout.preferredHeight: 20
            Layout.minimumWidth: 14
            radius: 2
            // A loop pad is amber and says so with ⟲: the colour is the only thing a glance reads.
            color: pad.loop !== null
                   ? panel.loopColor
                   : (pad.frame !== null
                      ? panel.hotcueColor(pad.index, panel.cuePalette[pad.index % panel.cuePalette.length])
                      : UiSettings.themeRaised)
            border.width: 1
            border.color: UiSettings.themeBorder
            opacity: pad.frame !== null || pad.loop !== null ? 1.0 : 0.7

            Text {
                anchors.fill: parent
                anchors.margins: 1
                text: pad.loop !== null ? "⟲" : (pad.label.length > 0 ? pad.label
                                                                   : String(pad.index + 1))
                font.pixelSize: 8
                font.bold: true
                horizontalAlignment: Text.AlignHCenter
                verticalAlignment: Text.AlignVCenter
                elide: Text.ElideRight
                // Dark on a lit pad, dim on an empty one: a bright pad with white text is a blank.
                color: pad.frame !== null || pad.loop !== null ? "#101010" : UiSettings.themeTextDim
            }

            MouseArea {
                anchors.fill: parent
                acceptedButtons: Qt.LeftButton | Qt.RightButton
                onClicked: function (mouse) {
                    if (!panel.live) {
                        return;
                    }
                    if (mouse.button === Qt.RightButton) {
                        cueMenu.slot = pad.index;
                        cueMenu.popup();
                        return;
                    }
                    if (mouse.modifiers & Qt.ShiftModifier) {
                        Bridge.cue(panel.deckId, "Delete", pad.index);
                    } else if (pad.loop !== null || pad.frame !== null) {
                        Bridge.cue(panel.deckId, "Jump", pad.index);
                    } else if (panel.loop !== null && panel.loop !== undefined) {
                        // An empty pad while a loop is active stores that loop: the four bars become part
                        // of the set, which is what a DJ does between two mixes. The loop lives on the
                        // pad from now on, so the deck's own copy — and its remembered outline — go away.
                        Bridge.setPadLoop(panel.deckId, pad.index);
                    } else {
                        Bridge.cue(panel.deckId, "Set", pad.index);
                    }
                }
            }
        }
    }

    function statusText() {
        if (panel.loopNotice.length > 0) {
            return panel.loopNotice;
        }
        const load = panel.deck.load;
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

}
