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
Item {
    id: window
    objectName: "settingsWindow"
    // The window covers the application, not the strip it was declared in: the gear stands in the global bar, and a
    // panel filling a bar 30 px tall is a panel with no height at all.
    anchors.fill: parent
    visible: false

    readonly property var themeNames: Settings ? Settings.themeSets() : []

    function open() {
        window.visible = true;
        window.forceActiveFocus();
    }
    function close() {
        window.visible = false;
    }

    Keys.onEscapePressed: window.close()

    // A dimmed field: the panels behind stay visible, so the DJ keeps his place.
    Rectangle {
        anchors.fill: parent
        color: "#99000000"
        MouseArea {
            anchors.fill: parent
            onClicked: window.close()
        }
    }

    Rectangle {
        id: panel
        objectName: "settingsPanel"
        anchors.centerIn: parent
        width: Math.min(parent.width - 60, 820)
        height: Math.min(parent.height - 60, 560)
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
                                            path: modelData.path
                                            kind: modelData.kind === undefined ? "text" : modelData.kind
                                            values: modelData.values === undefined ? [] : modelData.values
                                            gatePath: modelData.gatePath === undefined ? "" : modelData.gatePath
                                            gateValues: modelData.gateValues === undefined
                                                        ? [] : modelData.gateValues
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
                { label: qsTr("Device"), path: "engine:audio.device", kind: "device" },
                { label: qsTr("Device check"), path: "engine:audio.device", kind: "check" },
                { label: qsTr("Sample rate"), path: "engine:audio.sample_rate", kind: "choice",
                  values: ["44100", "48000", "96000"], numeric: true },
                { label: qsTr("Block, frames"), path: "engine:audio.block_frames", kind: "choice",
                  values: ["128", "256", "512", "1024"], numeric: true },
                { label: qsTr("Prefetch, seconds"), path: "engine:audio.prefetch_seconds", kind: "number",
                  from: 1, to: 30 },
                { label: qsTr("Headphones"), path: "engine:audio.headphones.mode", kind: "choice",
                  values: ["auto", "same_device", "device"] },
                // A device by name only in "device" mode: "auto" and "same_device" pick it themselves, and the line
                // says so instead of pretending the name is doing something.
                { label: qsTr("Headphone device"), path: "engine:audio.headphones.device", kind: "device",
                  gatePath: "engine:audio.headphones.mode", gateValues: ["device"] },
                { label: qsTr("Headphone channels"), path: "engine:audio.headphones.pair", kind: "choice",
                  values: ["[0, 1]", "[2, 3]"] },
                { label: qsTr("Headphone gain"), path: "engine:audio.headphones.gain", kind: "slider",
                  from: 0, to: 1, decimals: 2 },
                { label: qsTr("Headphone mix"), path: "engine:audio.headphones.mix", kind: "slider",
                  from: 0, to: 1, decimals: 2 }
            ]
        },
        {
            key: "analysis",
            title: qsTr("Analysis"),
            about: qsTr("How a track is measured when it is loaded."),
            rows: [
                { label: qsTr("Auto gain share"), path: "engine:analysis.auto_gain_share", kind: "slider",
                  from: 0, to: 1, decimals: 2 }
            ]
        },
        {
            key: "library",
            title: qsTr("Library"),
            about: qsTr("History and the PCM cache. The collection folders live in the browser, where they are added."),
            rows: [
                { label: qsTr("History, seconds"), path: "engine:library.history_min_seconds", kind: "number",
                  from: 0, to: 600 },
                { label: qsTr("Recently played, hours"), path: "engine:library.played_recently_hours", kind: "number",
                  from: 1, to: 720 },
                { label: qsTr("PCM cache"), path: "engine:pcm_cache.enabled", kind: "switch" },
                { label: qsTr("PCM cache, GB"), path: "engine:pcm_cache.max_size_gb", kind: "number",
                  from: 1, to: 500 },
                { label: qsTr("Long file, minutes"), path: "engine:pcm_cache.long_file_minutes", kind: "number",
                  from: 1, to: 240 }
            ]
        },
        {
            key: "mixer",
            title: qsTr("Mixer"),
            about: qsTr("The faders' own laws and the crossfader stay on the mixer: here are the defaults of the shell."),
            rows: [
                { label: qsTr("Deck rows"), path: "layout:deckRows", kind: "number", from: 1, to: 4 },
                { label: qsTr("Channel order"), path: "layout:channelOrder", kind: "text" }
            ]
        },
        {
            key: "effects",
            title: qsTr("Effects"),
            about: qsTr("The unit each deck comes up with. Changing it on the mixer changes it here too."),
            rows: [
                { label: qsTr("Deck A"), path: "session:deckFx.A", kind: "choice",
                  values: ["filter", "echo", "roll", "flanger", "phaser", "bitcrusher", "noise"] },
                { label: qsTr("Deck B"), path: "session:deckFx.B", kind: "choice",
                  values: ["filter", "echo", "roll", "flanger", "phaser", "bitcrusher", "noise"] },
                { label: qsTr("Deck C"), path: "session:deckFx.C", kind: "choice",
                  values: ["filter", "echo", "roll", "flanger", "phaser", "bitcrusher", "noise"] },
                { label: qsTr("Deck D"), path: "session:deckFx.D", kind: "choice",
                  values: ["filter", "echo", "roll", "flanger", "phaser", "bitcrusher", "noise"] }
            ]
        },
        {
            key: "interface",
            title: qsTr("Interface"),
            about: qsTr("Arrangement and the waveform. Everything here applies at once."),
            rows: [
                { label: qsTr("Global band, share"), path: "layout:globalPercent", kind: "slider", decimals: 2 },
                { label: qsTr("Browser width, share"), path: "layout:browserPercent", kind: "slider", decimals: 2 },
                { label: qsTr("Mixer width, share"), path: "layout:mixerPercent", kind: "slider", decimals: 2 },
                { label: qsTr("Waveform span, beats"), path: "layout:waveformSpan", kind: "slider",
                  from: 1, to: 8, decimals: 1 },
                { label: qsTr("Waveform window, beats"), path: "layout:waveformBeats", kind: "number",
                  from: 4, to: 128 },
                { label: qsTr("Browser font, px"), path: "layout:browserFont", kind: "number", from: 8, to: 24 },
                { label: qsTr("Browser rail, px"), path: "layout:browserRailWidth", kind: "number",
                  from: 40, to: 260 },
                { label: qsTr("Snap to beats"), path: "layout:snapGrid", kind: "switch" },
                { label: qsTr("Quantize on release"), path: "layout:quantizeGrid", kind: "switch" },
                { label: qsTr("Audible scrubbing"), path: "layout:scrubAudible", kind: "switch" },
                { label: qsTr("Theme"), path: "ui:themeSet", kind: "choice", values: window.themeNames },
                { label: qsTr("Frames per second"), path: "ui:frameHz", kind: "number", from: 5, to: 240 },
                { label: qsTr("Pitch range, percent"), path: "ui:pitchRangePercent", kind: "number",
                  from: 1, to: 50 },
                { label: qsTr("Level knob law"), path: "ui:gainCurve", kind: "choice",
                  values: ["linear", "cubic"] },
                { label: qsTr("Knob law, middle"), path: "ui:gainCurveMiddle", kind: "slider", decimals: 2 },
                { label: qsTr("Fader law"), path: "ui:faderCurve", kind: "choice",
                  values: ["linear", "logarithmic"] },
                { label: qsTr("Fader law, middle"), path: "ui:faderCurveMiddle", kind: "slider", decimals: 2 },
                { label: qsTr("Waveform style"), path: "ui:waveformStyle", kind: "choice",
                  values: ["coloured", "plain"] },
                { label: qsTr("Waveform zoom"), path: "ui:waveformZoom", kind: "number", from: 1, to: 400 },
                { label: qsTr("Waveform bar, px"), path: "ui:waveformBarWidth", kind: "number", from: 1, to: 8 },
                { label: qsTr("Waveform gap, px"), path: "ui:waveformBarGap", kind: "number", from: 0, to: 4 }
            ]
        },
        {
            key: "diagnostics",
            title: qsTr("Diagnostics"),
            about: qsTr("The logs and the thresholds that decide when a run is reported as struggling."),
            rows: [
                { label: qsTr("Log level"), path: "engine:diagnostics.log_level", kind: "choice",
                  values: ["error", "warn", "info", "debug", "trace"] },
                { label: qsTr("Require real time"), path: "engine:realtime.require", kind: "switch" },
                { label: qsTr("Stall threshold, µs"), path: "engine:diagnostics.stall_threshold_us", kind: "number",
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
