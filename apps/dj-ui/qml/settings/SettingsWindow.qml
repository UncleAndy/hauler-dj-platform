import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import DjPlatform.Ui 1.0

// The settings window (plan `.hermes/plans/2026-11-21_120622-settings-window.md`).
//
// The window is a view over SettingsModel and nothing else: every line names a path, the model says which file owns
// it, and the control writes through that owner. Lines that only the engine reads are marked "after restart" — that
// marker is part of the interface, not a footnote, because a switch that changes nothing until the next start has to
// say so where it stands.
// A modal popup, not an overlay item: modality is what stops the mouse wheel over this window from reaching the
// waveform behind it and zooming the deck under the DJ's hand (owner's report, 21.11).
Popup {
    id: window
    objectName: "settingsWindow"
    modal: true
    focus: true
    closePolicy: Popup.CloseOnEscape
    anchors.centerIn: Overlay.overlay
    width: Overlay.overlay ? Overlay.overlay.width : 0
    height: Overlay.overlay ? Overlay.overlay.height : 0
    padding: 0

    // The theme sets belong to UiSettings, which owns the palette; the model only routes writes.
    readonly property var themeNames: UiSettings.themeSets()

    // A dimmed field: the panels behind stay visible, so the DJ keeps his place, and a click on it closes the window.
    background: Rectangle {
        color: "#99000000"
        MouseArea {
            anchors.fill: parent
            onClicked: window.close()
        }
    }

    contentItem: Item {
        Rectangle {
            id: panel
            objectName: "settingsPanel"
            anchors.centerIn: parent
            width: Math.max(320, Math.min(parent.width - 60, 820))
            height: Math.max(240, Math.min(parent.height - 60, 560))
        color: UiSettings.themeBackground
        border.color: UiSettings.themeTextDim
        border.width: 1
        radius: 4

        ColumnLayout {
            anchors.fill: parent
            anchors.margins: 12
            spacing: 8

            RowLayout {
                Layout.fillWidth: true
                Label {
                    text: qsTr("Settings")
                    color: UiSettings.themeText
                    font.pixelSize: 14
                    font.bold: true
                }
                Item { Layout.fillWidth: true }
                Button {
                    objectName: "settingsClose"
                    text: qsTr("Close")
                    font.pixelSize: 11
                    onClicked: window.close()
                }
            }

            RowLayout {
                Layout.fillWidth: true
                Layout.fillHeight: true
                spacing: 12

                // The sections, on the left.
                ColumnLayout {
                    objectName: "settingsNav"
                    // A set width, not a preference: the list of sections is a margin, and a margin that can grow
                    // takes the room the parameters need (owner's report, 21.11).
                    Layout.preferredWidth: 176
                    Layout.minimumWidth: 176
                    Layout.maximumWidth: 176
                    Layout.fillWidth: false
                    Layout.fillHeight: true
                    spacing: 2
                    Repeater {
                        model: window.sections
                        Button {
                            required property var modelData
                            required property int index
                            objectName: "settingsSection_" + modelData.key
                            Layout.fillWidth: true
                            text: modelData.title
                            font.pixelSize: 11
                            padding: 4
                            checkable: true
                            checked: pages.currentIndex === index
                            onClicked: pages.currentIndex = index
                        }
                    }
                    Item { Layout.fillHeight: true }
                }

                Rectangle {
                    objectName: "settingsContent"
                    Layout.fillWidth: true
                    Layout.minimumWidth: 320
                    Layout.fillHeight: true
                    color: "transparent"
                    border.color: UiSettings.themeTextDim
                    border.width: 1

                    StackLayout {
                        id: pages
                        objectName: "settingsPages"
                        anchors.fill: parent
                        anchors.margins: 8
                        currentIndex: 0

                        Repeater {
                            model: window.sections

                            Flickable {
                                required property var modelData
                                contentWidth: width
                                contentHeight: sectionColumn.implicitHeight
                                clip: true
                                ScrollBar.vertical: ScrollBar {}

                                ColumnLayout {
                                    id: sectionColumn
                                    width: pages.width - 8
                                    spacing: 2

                                    Label {
                                        text: modelData.about
                                        visible: modelData.about !== ""
                                        wrapMode: Text.WordWrap
                                        Layout.fillWidth: true
                                        color: UiSettings.themeTextDim
                                        font.pixelSize: 10
                                    }

                                    Repeater {
                                        model: modelData.rows

                                        SettingRow {
                                            required property var modelData
                                            Layout.fillWidth: true
                                            label: modelData.label
                                            hint: modelData.hint === undefined ? "" : modelData.hint
                                            path: modelData.path
                                            kind: modelData.kind === undefined ? "text" : modelData.kind
                                            values: modelData.values === undefined ? [] : modelData.values
                                            fallback: modelData.fallback === undefined ? "" : String(modelData.fallback)
                                            pairSide: modelData.pairSide === undefined ? -1 : modelData.pairSide
                                            allowNone: modelData.allowNone === true
                                            channelDevicePath: modelData.channelDevicePath === undefined
                                                               ? "engine:audio.device" : modelData.channelDevicePath
                                            gatePath: modelData.gatePath === undefined ? "" : modelData.gatePath
                                            gateValues: modelData.gateValues === undefined
                                                        ? [] : modelData.gateValues
                                            itemName: modelData.name === undefined ? "" : modelData.name
                                            onlyWhenDistinct: modelData.onlyWhenDistinct === true
                                            alsoPath: modelData.alsoPath === undefined ? "" : modelData.alsoPath
                                            alsoValue: modelData.alsoValue === undefined
                                                       ? "" : modelData.alsoValue
                                            onEdited: function (value) {
                                                // The headphones are a decision about other lines: the mode, and in
                                                // "device" mode the device itself, decide which channels are free. The
                                                // engine says what they mean, and the window stores that answer — a
                                                // device chosen without asking left the channels greyed out (owner's
                                                // report, 21.11).
                                                const changed = modelData.path;
                                                if (changed === "engine:audio.headphones.mode"
                                                    || changed === "engine:audio.headphones.device") {
                                                    const mode = String(Settings.value(
                                                                        "engine:audio.headphones.mode"));
                                                    Settings.applyHeadphoneMode(mode);
                                                }
                                            }
                                            numeric: modelData.numeric === true
                                            from: modelData.from === undefined ? 0 : modelData.from
                                            to: modelData.to === undefined ? 1 : modelData.to
                                            decimals: modelData.decimals === undefined ? 2 : modelData.decimals
                                        }
                                    }

                                    // The pages that show rather than set: versions, files, and the report button.
                                    Repeater {
                                        model: modelData.info === undefined ? [] : modelData.info
                                        Label {
                                            required property var modelData
                                            Layout.fillWidth: true
                                            text: modelData
                                            color: UiSettings.themeTextDim
                                            font.pixelSize: 10
                                            wrapMode: Text.WordWrap
                                        }
                                    }

                                    Button {
                                        visible: modelData.key === "about"
                                        text: qsTr("Copy report to the clipboard")
                                        font.pixelSize: 11
                                        onClicked: Settings.copyReport()
                                    }

                                    Item { Layout.fillHeight: true; Layout.preferredHeight: 4 }
                                }
                            }
                        }
                    }
                }
            }
        }
        }
    }

    // What the window holds. Every line is something the code reads: a control without a reader is a promise the
    // program does not keep.
    readonly property var sections: [
        {
            key: "sound",
            title: qsTr("Sound"),
            about: qsTr("The engine reads these when it starts: the marks say \"after restart\"."),
            rows: [
                // The outputs the engine can open, asked for when the window opens. Text only when the engine named
                // none at all, and then the row says why.
                { label: qsTr("Device"),
                  hint: qsTr("The output the engine opens. The list is what the engine names on this machine; a machine it cannot ask shows a field for a name instead."), path: "engine:audio.device", kind: "device" },
                { label: qsTr("Device check"),
                  hint: qsTr("Asks the engine about the current device and shows its format — sample rate, sample format and channel count — without opening a stream."), path: "engine:audio.device", kind: "check" },
                // One channel per row: a booth with an odd interface numbers its sockets, not pairs, and a main output
                // that can only be chosen in twos cannot express [0, 2] (owner's decision, 21.11).
                { label: qsTr("Main channel L"),
                  hint: qsTr("Which hardware channel the main output leaves on. A pair the device does not have is refused; swapping L and R inverts the stereo image."), path: "engine:audio.master", kind: "choice", pairSide: 0 },
                { label: qsTr("Main channel R"),
                  hint: qsTr("The right half of the main pair. Both halves are stored together, as one pair of channels."), path: "engine:audio.master", kind: "choice", pairSide: 1 },
                { label: qsTr("Sample rate"),
                  hint: qsTr("The rate the engine runs at. Tracks are resampled to it, and both the device and the engine must accept it. Takes effect on the next start."), path: "engine:audio.sample_rate", kind: "choice",
                  values: ["44100", "48000", "96000"], numeric: true },
                { label: qsTr("Block, frames"),
                  hint: qsTr("Size of one audio callback. Smaller is lower latency and more wake-ups: 256 frames is about 5 ms at 48 kHz."), path: "engine:audio.block_frames", kind: "choice",
                  values: ["128", "256", "512", "1024"], numeric: true },
                { label: qsTr("Prefetch, seconds"),
                  hint: qsTr("How far ahead of the playhead the decoder reads. More is safer on a slow disk and costs memory."), path: "engine:audio.prefetch_seconds", kind: "number",
                  from: 1, to: 30 },
                { label: qsTr("Headphones"),
                  hint: qsTr("Where the cue bus goes. auto takes spare channels of the main device; same_device keeps the main device; device opens a second one with its own clock."), path: "engine:audio.headphones.mode", kind: "choice",
                  values: ["auto", "same_device", "device"] },
                // A device by name only in "device" mode: "auto" and "same_device" pick it themselves, and the line
                // says so instead of pretending the name is doing something.
                { label: qsTr("Headphone device"),
                  hint: qsTr("The second device, when the mode is device. In the other modes the engine decides, and this line is locked."), path: "engine:audio.headphones.device", kind: "device",
                  gatePath: "engine:audio.headphones.mode", gateValues: ["device"] },
                // The cue takes one channel per row too, and it may have none: an interface whose every socket carries
                // the main output has nothing left for monitoring, and "None" says so instead of offering a lie.
                // The same characteristics for the interface that carries the cue, shown only when it is a device of its
                // own: on the main interface the line above has already answered.
                { label: qsTr("Headphone device check"),
                  hint: qsTr("Asks the engine about the headphone device and shows its format, the way the main device check does."), path: "engine:audio.headphones.device", kind: "check",
                  name: "settingRow_headphoneDeviceCheck",
                  gatePath: "engine:audio.headphones.mode", gateValues: ["device"],
                  onlyWhenDistinct: true },
                // Editable where the interface has spare channels and the DJ is the one choosing; locked to "None" when
                // it does not, because monitoring that cannot exist must not offer a choice.
                { label: qsTr("Headphone channel L"),
                  hint: qsTr("Which channel of the headphone device carries the left side of the cue. Only channels the device really has are offered."), path: "engine:audio.headphones.pair", kind: "choice",
                  pairSide: 0, allowNone: true, channelDevicePath: "engine:audio.headphones.device",
                  gatePath: "engine:audio.headphones.mode", gateValues: ["same_device", "device"],
                  alsoPath: "engine:audio.headphones.enabled", alsoValue: "true" },
                { label: qsTr("Headphone channel R"),
                  hint: qsTr("The right half of the cue pair."), path: "engine:audio.headphones.pair", kind: "choice",
                  pairSide: 1, allowNone: true, channelDevicePath: "engine:audio.headphones.device",
                  gatePath: "engine:audio.headphones.mode", gateValues: ["same_device", "device"],
                  alsoPath: "engine:audio.headphones.enabled", alsoValue: "true" },
                { label: qsTr("Headphone gain"),
                  hint: qsTr("Level of the cue bus before it leaves for the headphones."), path: "engine:audio.headphones.gain", kind: "slider",
                  from: 0, to: 1, decimals: 2 },
                { label: qsTr("Headphone mix"),
                  hint: qsTr("Blend in the headphones between the master mix and the cued channel: at 0 the DJ hears the cue alone."), path: "engine:audio.headphones.mix", kind: "slider",
                  from: 0, to: 1, decimals: 2 }
            ]
        },
        {
            key: "analysis",
            title: qsTr("Analysis"),
            about: qsTr("How a track is measured when it is loaded."),
            rows: [
                { label: qsTr("Warn before the end"),
                  hint: qsTr("How long before a track ends the decks start flashing: the deck's frame, the jog's ring and the panel's screen. Nothing plays a loop inside a warning, and 0 is off."),
                  path: "engine:display.end_warning_seconds", kind: "slider",
                  from: 0, to: 120, decimals: 0 },
                { label: qsTr("Auto gain share"),
                  hint: qsTr("How much of the automatic gain is applied to the track itself rather than left to the channel fader."), path: "engine:analysis.auto_gain_share", kind: "slider",
                  from: 0, to: 1, decimals: 2 }
            ]
        },
        {
            key: "library",
            title: qsTr("Library"),
            about: qsTr("Where the music is, and how much of it the engine remembers."),
            rows: [
                // The collection folders, editable here: a list with a way to add and take away, not a path typed into
                // a file (owner's request, 22.11).
                { label: qsTr("Collection folders"),
                  hint: qsTr("Every folder the collection is built from, scanned recursively. A folder that is gone is marked in the warning colour; nothing is dropped from the list behind your back."), path: "layout:collectionRoots", kind: "folders" },
                { label: qsTr("History, seconds"),
                  hint: qsTr("How much play history is kept, for the marks that show what has been played."), path: "engine:library.history_min_seconds", kind: "number",
                  from: 0, to: 600 },
                { label: qsTr("Recently played, hours"),
                  hint: qsTr("A track played within this window is marked as recently played in the browser."), path: "engine:library.played_recently_hours", kind: "number",
                  from: 1, to: 720 },
                { label: qsTr("PCM cache"),
                  hint: qsTr("Keeps decoded audio of lossy files on disk, so loading the same track again does not decode it again."), path: "engine:pcm_cache.enabled", kind: "switch" },
                { label: qsTr("PCM cache, GB"),
                  hint: qsTr("Upper bound for the cache. When it is reached the oldest entries are dropped."), path: "engine:pcm_cache.max_size_gb", kind: "number",
                  from: 1, to: 500 },
                { label: qsTr("Long file, minutes"),
                  hint: qsTr("Files longer than this are not cached: decoding them again is cheaper than holding them."), path: "engine:pcm_cache.long_file_minutes", kind: "number",
                  from: 1, to: 240 }
            ]
        },
        {
            key: "mixer",
            title: qsTr("Mixer"),
            about: qsTr("The faders' own laws and the crossfader stay on the mixer: here are the defaults of the shell."),
            rows: [
                { label: qsTr("Deck rows"),
                  hint: qsTr("How many rows of decks the mixer shows: one row puts every deck side by side, two rows stack them."), path: "layout:deckRows", kind: "number", from: 1, to: 4 },
                { label: qsTr("Channel order"),
                  hint: qsTr("Left-to-right order of the channel strips on the mixer, by deck letter."), path: "layout:channelOrder", kind: "text" }
            ]
        },
        {
            key: "effects",
            title: qsTr("Effects"),
            about: qsTr("The unit each deck comes up with. Changing it on the mixer changes it here too."),
            rows: [
                { label: qsTr("Deck A"),
                  hint: qsTr("Which effect deck A's unit holds. The amount knob and the on/off button stay on the mixer."), path: "session:deckFx.A", kind: "choice",
                  values: ["filter", "echo", "roll", "flanger", "phaser", "bitcrusher", "noise"] },
                { label: qsTr("Deck B"),
                  hint: qsTr("Which effect deck B's unit holds."), path: "session:deckFx.B", kind: "choice",
                  values: ["filter", "echo", "roll", "flanger", "phaser", "bitcrusher", "noise"] },
                { label: qsTr("Deck C"),
                  hint: qsTr("Which effect deck C's unit holds."), path: "session:deckFx.C", kind: "choice",
                  values: ["filter", "echo", "roll", "flanger", "phaser", "bitcrusher", "noise"] },
                { label: qsTr("Deck D"),
                  hint: qsTr("Which effect deck D's unit holds."), path: "session:deckFx.D", kind: "choice",
                  values: ["filter", "echo", "roll", "flanger", "phaser", "bitcrusher", "noise"] }
            ]
        },
        {
            key: "interface",
            title: qsTr("Interface"),
            about: qsTr("Arrangement and the waveform. Everything here applies at once."),
            rows: [
                { label: qsTr("Global band, share"),
                  hint: qsTr("Share of the window height taken by the band above the panels."), path: "layout:globalPercent", kind: "slider",
                  from: 3, to: 25, decimals: 1 },
                { label: qsTr("Browser width, share"),
                  hint: qsTr("Share of the middle row taken by the browser."), path: "layout:browserPercent", kind: "slider",
                  from: 8, to: 77, decimals: 1 },
                { label: qsTr("Mixer width, share"),
                  hint: qsTr("Share of the middle row taken by the mixer; the waveform takes what is left."), path: "layout:mixerPercent", kind: "slider",
                  from: 8, to: 40, decimals: 1 },
                { label: qsTr("Waveform window, beats"),
                  hint: qsTr("Length of the beat window used when the strip is zoomed in."), path: "layout:waveformBeats", kind: "number",
                  from: 4, to: 128 },
                { label: qsTr("Browser font, px"),
                  hint: qsTr("Text size in the browser list."), path: "layout:browserFont", kind: "number", from: 8, to: 24 },
                { label: qsTr("Browser rail, px"),
                  hint: qsTr("Width of the browser's section rail."), path: "layout:browserRailWidth", kind: "number",
                  from: 40, to: 260 },
                { label: qsTr("Snap to beats"),
                  hint: qsTr("Dragging a marker or the playhead snaps to the beat grid."), path: "layout:snapGrid", kind: "switch" },
                { label: qsTr("Quantize on release"),
                  hint: qsTr("Releasing a hotcue or a loop jump moves the playhead to the nearest beat."), path: "layout:quantizeGrid", kind: "switch" },
                { label: qsTr("Audible scrubbing"),
                  hint: qsTr("Dragging the waveform plays the audio under the hand instead of staying silent."), path: "layout:scrubAudible", kind: "switch" },
                { label: qsTr("Theme"),
                  hint: qsTr("Colour set for the whole interface. The choice is stored with the layout, not with the engine."), path: "ui:themeSet", kind: "choice", values: window.themeNames },
                { label: qsTr("Frames per second"),
                  hint: qsTr("How often the interface redraws. Higher is smoother and costs more CPU; audio never depends on it."), path: "ui:frameHz", kind: "number", from: 5, to: 240 },
                { label: qsTr("Pitch range, percent"),
                  hint: qsTr("Range of the tempo fader, in percent either side of the track's own tempo."), path: "ui:pitchRangePercent", kind: "number",
                  from: 1, to: 50 },
                { label: qsTr("Level knob law"),
                  hint: qsTr("The curve a level knob follows between its ends."), path: "ui:gainCurve", kind: "choice",
                  values: ["linear", "cubic"] },
                { label: qsTr("Knob law, middle"),
                  hint: qsTr("Where the middle of the knob law sits, as a share of the travel."), path: "ui:gainCurveMiddle", kind: "slider", decimals: 2 },
                { label: qsTr("Fader law"),
                  hint: qsTr("The curve a channel fader follows between its ends."), path: "ui:faderCurve", kind: "choice",
                  values: ["linear", "logarithmic"] },
                { label: qsTr("Fader law, middle"),
                  hint: qsTr("Where the middle of the fader law sits, as a share of the travel."), path: "ui:faderCurveMiddle", kind: "slider", decimals: 2 },
                { label: qsTr("Waveform style"),
                  hint: qsTr("How the waveform is drawn: bars, an outline or filled."), path: "ui:waveformStyle", kind: "choice",
                  values: ["coloured", "plain"] },
                { label: qsTr("Waveform zoom"),
                  hint: qsTr("Magnification of the waveform strip, from the whole track to a few beats."), path: "ui:waveformZoom", kind: "number", from: 1, to: 400 },
                { label: qsTr("Waveform bar, px"),
                  hint: qsTr("Width of one bar when the waveform is drawn as bars."), path: "ui:waveformBarWidth", kind: "number", from: 1, to: 8 },
                { label: qsTr("Waveform gap, px"),
                  hint: qsTr("Space between bars."), path: "ui:waveformBarGap", kind: "number", from: 0, to: 4 }
            ]
        },
        {
            key: "controller",
            title: qsTr("Controller"),
            about: qsTr("A DJ controller is optional. With the switch off nothing else starts: no service, no device "
                        + "opened, and the program is the mouse-and-keyboard one you already know."),
            rows: [
                { label: qsTr("Use a DJ controller"),
                  hint: qsTr("Whether the controller service runs at all. Off is the way a fresh install behaves: no extra process is started and no HID node is opened. Turning it on takes effect the next time the program starts."),
                  path: "engine:controller.enabled", kind: "switch" },
                { label: qsTr("Driver"),
                  hint: qsTr("Which device the service should read. None means the switch is on but nothing is claimed, and the log says so instead of failing quietly."),
                  path: "engine:controller.driver", kind: "choice", values: ["none", "s4mk3"] },
                { label: qsTr("Device"),
                  hint: qsTr("The HID node to open, when the driver should not search for it by itself. Empty means search: the service looks for the vendor and product the driver knows."),
                  path: "engine:controller.device", kind: "text" },
                { label: qsTr("Headphone button"),
                  hint: qsTr("How the channel's headphone (CUE) button behaves. Hold keeps the channel in the headphones only while the button is pressed; Toggle switches monitoring on with one press and off with the next."),
                  path: "engine:controller.pfl", kind: "choice", values: ["hold", "toggle"] },
                // The panel's colours are settings, not taste baked into code: the same eighteen names the device's own
                // protocol uses, and whichever a person picks is what the panel shows.
                { label: qsTr("Light the whole panel"),
                  hint: qsTr("Night mode: instead of leaving inactive controls dark, glow them faintly in the resting colour so every button can be found without a lamp. Active controls keep their own colours, one brightness step brighter."),
                  path: "engine:controller.lights.night", kind: "switch" },
                { label: qsTr("Resting colour"),
                  hint: qsTr("The colour of everything that is doing nothing, in night mode. White shows the panel best in the dark; a colour matching the booth's lighting is easier on the eyes."),
                  path: "engine:controller.lights.resting", kind: "choice",
                  values: ["off", "red", "carrot", "orange", "honey", "yellow", "lime", "green",
                           "aqua", "celeste", "sky", "blue", "purple", "fuscia", "magenta",
                           "azalea", "salmon", "white"] },
                { label: qsTr("Jog ring"),
                  hint: qsTr("The marker on a playing deck's platter: it goes round once per record revolution, like the label on a turning record. A colour that stands out from the booth's lighting is easiest to find at a glance."),
                  path: "engine:controller.lights.jog", kind: "choice", fallback: "white",
                  values: ["off", "red", "carrot", "orange", "honey", "yellow", "lime", "green",
                           "aqua", "celeste", "sky", "blue", "purple", "fuscia", "magenta",
                           "azalea", "salmon", "white"] },
                { label: qsTr("Playing deck"),
                  hint: qsTr("The PLAY button of a deck that is playing. Lit only while the deck runs, so a look at the panel answers 'what is playing'."),
                  path: "engine:controller.lights.play", kind: "choice",
                  values: ["off", "red", "carrot", "orange", "honey", "yellow", "lime", "green",
                           "aqua", "celeste", "sky", "blue", "purple", "fuscia", "magenta",
                           "azalea", "salmon", "white"] },
                { label: qsTr("Headphone monitoring"),
                  hint: qsTr("The CUE button of a channel that feeds the headphones. With the S4 MK3 the headphone volume and mix are turned on the controller itself; this light says which channel is being listened to."),
                  path: "engine:controller.lights.cue", kind: "choice",
                  values: ["off", "red", "carrot", "orange", "honey", "yellow", "lime", "green",
                           "aqua", "celeste", "sky", "blue", "purple", "fuscia", "magenta",
                           "azalea", "salmon", "white"] },
                { label: qsTr("Sync"),
                  hint: qsTr("The Sync/Lock button of a deck that follows another. Lit while the deck follows, dark when it runs on its own tempo."),
                  path: "engine:controller.lights.sync", kind: "choice",
                  values: ["off", "red", "carrot", "orange", "honey", "yellow", "lime", "green",
                           "aqua", "celeste", "sky", "blue", "purple", "fuscia", "magenta",
                           "azalea", "salmon", "white"] },
                { label: qsTr("Pitch-zero lamp: at zero"),
                  hint: qsTr("The LED beside each tempo fader has two states, and each wears its own colour. This is the colour of the state that says the deck runs at the track's own pitch (within one percent); `off` means nothing lights there at all."),
                  path: "engine:controller.lights.pitch_zero_on", kind: "choice", fallback: "white",
                  values: ["off", "red", "carrot", "orange", "honey", "yellow", "lime", "green",
                           "aqua", "celeste", "sky", "blue", "purple", "fuscia", "magenta",
                           "azalea", "salmon", "white"] },
                { label: qsTr("Pitch-zero lamp: away from zero"),
                  hint: qsTr("The same LED while the deck's pitch is somewhere else. `off` keeps the panel quiet and is the ordinary rule; a colour turns the lamp into a two-colour signal — red away from zero, green at it, for instance."),
                  path: "engine:controller.lights.pitch_zero_off", kind: "choice", fallback: "off",
                  values: ["off", "red", "carrot", "orange", "honey", "yellow", "lime", "green",
                           "aqua", "celeste", "sky", "blue", "purple", "fuscia", "magenta",
                           "azalea", "salmon", "white"] },
                { label: qsTr("Pitch-zero lamp: at the fader's zero point"),
                  hint: qsTr("The fader is relative — its position means 'this much pitch from where the hand took it' — so the place that makes the pitch zero moves with the hand. This is the colour of the LED while the fader stands on that place and the deck has not arrived there yet: the engine glides for about 30 ms after a command, and a sync can hold the pitch elsewhere for as long as it likes. `honey` by default; `off` leaves the lamp with the two states it had before."),
                  path: "engine:controller.lights.pitch_zero_target", kind: "choice", fallback: "honey",
                  values: ["off", "red", "carrot", "orange", "honey", "yellow", "lime", "green",
                           "aqua", "celeste", "sky", "blue", "purple", "fuscia", "magenta",
                           "azalea", "salmon", "white"] },
                { label: qsTr("Invert the pitch-zero lamp"),
                  hint: qsTr("The LED beside each tempo fader is lit while the deck runs at the track's own pitch (within one percent) and dark otherwise. Inverted, it goes out at zero and lights while the pitch is somewhere else — for a hand that reads a lit lamp as 'something is on'. Takes effect the next time the controller service starts."),
                  path: "engine:controller.lights.pitch_zero_inverted", kind: "switch" },
                { label: qsTr("Hide deck buttons with a controller"),
                  hint: qsTr("While a controller is connected and driving, the deck's transport row (PLAY, CUE, LOOP, jump, REV, FLX) folds away: the same buttons are under your hand on the panel. Off keeps both, for a desk with a screen and a controller side by side."),
                  path: "layout:hideDeckButtonsWithController", kind: "switch" },
                { label: qsTr("Master"),
                  hint: qsTr("The MASTER button of the deck the others follow. A colour of its own, not the sync lamp's, so with both lit the leader is told from the follower at a glance; a colour that stands out from the booth's lighting is easiest."),
                  path: "engine:controller.lights.master", kind: "choice", fallback: "white",
                  values: ["off", "red", "carrot", "orange", "honey", "yellow", "lime", "green",
                           "aqua", "celeste", "sky", "blue", "purple", "fuscia", "magenta",
                           "azalea", "salmon", "white"] },
                { label: qsTr("Reverse (REV)"),
                  hint: qsTr("A deck playing its material backwards. Lit while it runs backwards, dark when it runs forwards again, so the panel answers 'which way is this deck going' without a look at the screen."),
                  path: "engine:controller.lights.reverse", kind: "choice", fallback: "red",
                  values: ["off", "red", "carrot", "orange", "honey", "yellow", "lime", "green",
                           "aqua", "celeste", "sky", "blue", "purple", "fuscia", "magenta",
                           "azalea", "salmon", "white"] },
                { label: qsTr("Slip (FLX)"),
                  hint: qsTr("A deck whose sound is slipped away from its head: a jump or a loop moves what is heard while the track's own head runs on. The lamp says the deck will come back to where it would have been."),
                  path: "engine:controller.lights.slip", kind: "choice", fallback: "sky",
                  values: ["off", "red", "carrot", "orange", "honey", "yellow", "lime", "green",
                           "aqua", "celeste", "sky", "blue", "purple", "fuscia", "magenta",
                           "azalea", "salmon", "white"] },
                { label: qsTr("Hotcues"),
                  hint: qsTr("The cue pads that hold a saved hotcue. A pad with nothing stored stays dark, so the panel shows which slots the track actually uses."),
                  path: "engine:controller.lights.hotcue", kind: "choice",
                  values: ["off", "red", "carrot", "orange", "honey", "yellow", "lime", "green",
                           "aqua", "celeste", "sky", "blue", "purple", "fuscia", "magenta",
                           "azalea", "salmon", "white"] },
                { label: qsTr("Loop pads"),
                  hint: qsTr("The pads that hold a loop instead of a cue. The screen draws a loop amber, and yellow is the panel's nearest colour: the same thing under the hand and on the screen. A pad with a cue in it shows the cue's colour instead."),
                  path: "engine:controller.lights.loop", kind: "choice",
                  values: ["off", "red", "carrot", "orange", "honey", "yellow", "lime", "green",
                           "aqua", "celeste", "sky", "blue", "purple", "fuscia", "magenta",
                           "azalea", "salmon", "white"] },
                { label: qsTr("Effect on"),
                  hint: qsTr("The FX button of a deck whose effect is switched in. On the S4 MK3 the mixer section has one for each of the four decks."),
                  path: "engine:controller.lights.fx", kind: "choice",
                  values: ["off", "red", "carrot", "orange", "honey", "yellow", "lime", "green",
                           "aqua", "celeste", "sky", "blue", "purple", "fuscia", "magenta",
                           "azalea", "salmon", "white"] },
                // What each of the four numbered FX select keys loads and how it glows. The dedicated Filter key beside
                // them always loads the filter and is not a row here (owner's request, 25.11).
                { label: qsTr("FX key 1 effect"),
                  hint: qsTr("Which effect the first FX select key loads when pressed. The dedicated Filter key beside the four numbered ones always loads the filter."),
                  path: "engine:controller.fx.key1_effect", kind: "choice", fallback: "echo",
                  values: ["filter", "echo", "roll", "flanger", "phaser", "bitcrusher", "noise"] },
                { label: qsTr("FX key 1 colour"),
                  hint: qsTr("The colour the first FX select key glows, and the colour a deck running that effect shows on its FX button."),
                  path: "engine:controller.fx.key1_colour", kind: "choice", fallback: "blue",
                  values: ["off", "red", "carrot", "orange", "honey", "yellow", "lime", "green",
                           "aqua", "celeste", "sky", "blue", "purple", "fuscia", "magenta",
                           "azalea", "salmon", "white"] },
                { label: qsTr("FX key 2 effect"),
                  hint: qsTr("Which effect the second FX select key loads when pressed."),
                  path: "engine:controller.fx.key2_effect", kind: "choice", fallback: "roll",
                  values: ["filter", "echo", "roll", "flanger", "phaser", "bitcrusher", "noise"] },
                { label: qsTr("FX key 2 colour"),
                  hint: qsTr("The colour the second FX select key glows, and the colour a deck running that effect shows on its FX button."),
                  path: "engine:controller.fx.key2_colour", kind: "choice", fallback: "green",
                  values: ["off", "red", "carrot", "orange", "honey", "yellow", "lime", "green",
                           "aqua", "celeste", "sky", "blue", "purple", "fuscia", "magenta",
                           "azalea", "salmon", "white"] },
                { label: qsTr("FX key 3 effect"),
                  hint: qsTr("Which effect the third FX select key loads when pressed."),
                  path: "engine:controller.fx.key3_effect", kind: "choice", fallback: "flanger",
                  values: ["filter", "echo", "roll", "flanger", "phaser", "bitcrusher", "noise"] },
                { label: qsTr("FX key 3 colour"),
                  hint: qsTr("The colour the third FX select key glows, and the colour a deck running that effect shows on its FX button."),
                  path: "engine:controller.fx.key3_colour", kind: "choice", fallback: "purple",
                  values: ["off", "red", "carrot", "orange", "honey", "yellow", "lime", "green",
                           "aqua", "celeste", "sky", "blue", "purple", "fuscia", "magenta",
                           "azalea", "salmon", "white"] },
                { label: qsTr("FX key 4 effect"),
                  hint: qsTr("Which effect the fourth FX select key loads when pressed."),
                  path: "engine:controller.fx.key4_effect", kind: "choice", fallback: "phaser",
                  values: ["filter", "echo", "roll", "flanger", "phaser", "bitcrusher", "noise"] },
                { label: qsTr("FX key 4 colour"),
                  hint: qsTr("The colour the fourth FX select key glows, and the colour a deck running that effect shows on its FX button."),
                  path: "engine:controller.fx.key4_colour", kind: "choice", fallback: "aqua",
                  values: ["off", "red", "carrot", "orange", "honey", "yellow", "lime", "green",
                           "aqua", "celeste", "sky", "blue", "purple", "fuscia", "magenta",
                           "azalea", "salmon", "white"] }
            ]
        },
        {
            key: "diagnostics",
            title: qsTr("Diagnostics"),
            about: qsTr("The logs and the thresholds that decide when a run is reported as struggling."),
            rows: [
                { label: qsTr("Log level"),
                  hint: qsTr("How much the engine writes to its log. The environment variable HAULER_LOG, when it is set, wins over this line."), path: "engine:diagnostics.log_level", kind: "choice",
                  values: ["error", "warn", "info", "debug", "trace"] },
                { label: qsTr("Require real time"),
                  hint: qsTr("Refuse to start when the process cannot get real-time priority, instead of running with the risk of dropouts."), path: "engine:realtime.require", kind: "switch" },
                { label: qsTr("Stall threshold, µs"),
                  hint: qsTr("A callback that takes longer than this is counted as a stall and written to the log."), path: "engine:diagnostics.stall_threshold_us", kind: "number",
                  from: 100, to: 5000 }
            ]
        },
        {
            key: "about",
            title: qsTr("About"),
            about: "",
            rows: [],
            info: [
                qsTr("hauler-dj %1").arg(Qt.application.version),
                qsTr("protocol bridge-v1 / state-v1"),
                qsTr("settings: %1").arg(Settings.fileOf("engine:audio.device")),
                qsTr("layout: %1").arg(Settings.fileOf("layout:deckRows")),
                qsTr("session: %1").arg(Settings.fileOf("session:deckFx.A")),
                qsTr("Licences: LICENSES/ and THIRD_PARTY.md beside the program. The beat model is MIT; "
                     + "ONNX Runtime is MIT; the C++ runtime is shipped beside the program.")
            ]
        }
    ]
}
