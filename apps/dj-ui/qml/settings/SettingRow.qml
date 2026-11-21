import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import DjPlatform.Ui 1.0

// One line of the settings window: a name, a control, and where the value lands.
//
// The value is never copied into the window as its own truth: it is read from the owner every time the model says
// something changed, and written through the owner when the DJ moves the control. A control that keeps its own copy
// drifts from the file the engine reads, and the drift is only noticed at the next start.
Item {
    id: row

    property string label: ""
    property string path: ""
    // "switch" | "choice" | "slider" | "number" | "text" | "check" | "device" | "pair"
    property string kind: "text"
    // What the engine answered about the device (kind "check").
    property string answer: ""
    property var values: []
    property string hint: ""
    property real from: 0
    property real to: 1
    property int decimals: 2
    property bool restart: Settings ? Settings.needsRestart(path) : false
    // A line that only makes sense under one setting of another line: the headphone device is chosen by name only in
    // the "device" mode, and in the other modes the engine picks it. The control stays visible and locked — hiding it
    // would leave the DJ wondering where the device went (owner's report, 21.11).
    property string gatePath: ""
    // A second condition, for a line whose openness depends on two things at once: the cue has channels only where the
    // mode lets the DJ choose and the interface has a spare pair to give.
    property string alsoPath: ""
    property string alsoValue: ""
    // Which line names the device the pairs belong to.
    property string pairDevicePath: "engine:audio.device"
    // Which half of a pair this row is: 0 or 1. A pair is written by two rows, one channel each — a DJ in a booth with
    // an odd interface numbers his sockets, not pairs (owner's decision, 21.11).
    property int pairSide: -1
    // Which device's channel count the list is built from.
    property string channelDevicePath: "engine:audio.device"
    // "None" is offered for the cue, never for the main output: silence on the main output is not a route.
    property bool allowNone: false
    property var channelChoices: []
    property var gateValues: []
    // The gate's value is read in reload(), not inside the binding: a binding cannot see the inside of an invokable, so
    // `Settings.value()` in a binding would answer once and never again — and the line stayed locked in every mode.
    property string gateValue: ""
    property string alsoCurrent: ""
    readonly property bool open: (row.gatePath === "" ? true : row.gateValues.indexOf(row.gateValue) >= 0)
                                 && (row.alsoPath === "" ? true : row.alsoCurrent === row.alsoValue)
    // A list of numbers: the file holds `48000`, and a control that carried the text would write it as a string the
    // engine then refuses to read.
    property bool numeric: false

    signal edited(var value)

    implicitHeight: 30
    implicitWidth: 600

    // `engine:audio.block_frames` reads as `settingRow_engine_audio_block_frames`: a check has to find the very line
    // the hand would use, not a copy of it.
    objectName: "settingRow_" + path.replace(/[:.]/g, "_")

    // What the list offers. The engine is asked in reload(), never inside a binding: a binding cannot see the inside of
    // an invokable, so a list read in one would be a list from the first moment and never again.
    property var engineDevices: []
    property var enginePairs: []
    readonly property var choices: row.pairSide >= 0 ? row.channelChoices
                                   : row.kind === "device" ? row.engineDevices
                                   : row.kind === "pair" ? row.enginePairs
                                   : row.values
    // The value stored in the file stays visible even when the device it names is no longer plugged in: hiding it
    // would look like the setting had been lost.
    readonly property var offered: {
        const list = [];
        for (let index = 0; index < row.choices.length; ++index) {
            list.push(String(row.choices[index]));
        }
        if (row.currentText !== "" && list.indexOf(row.currentText) < 0) {
            list.push(row.currentText);
        }
        return list;
    }
    readonly property bool asList: row.pairSide >= 0 ? row.channelChoices.length > 0
                                   : row.kind === "device" || row.kind === "pair"
                                   ? row.choices.length > 0
                                   : row.kind === "choice"
    // A row that shows neither a list nor a field is a row the DJ cannot see: this is what "the setting is not there"
    // looks like when the engine had no answer (owner's report, 21.11).
    readonly property bool asField: row.kind === "text"
                                    || ((row.kind === "device" || row.kind === "pair") && !row.asList)
                                    // A channel row with no device to ask still takes a number by hand: an organ that
                                    // shows neither a list nor a field is a row that is not there at all.
                                    || (row.pairSide >= 0 && !row.asList)

    property var current: undefined
    readonly property string currentText: row.current === undefined || row.current === null
                                       ? "" : String(row.current)

    // A key the file does not have is "nothing", not the word "undefined": `String(undefined)` in QML is a string, and
    // it went to the engine as a device name (found by the check, 21.11).
    function text(path) {
        const value = Settings.value(path);
        return value === undefined || value === null ? "" : String(value);
    }

    function reload() {
        if (row.pairSide >= 0) {
            // One channel of a pair: the number the file holds, or "None" when there is none.
            const channel = Settings.channelAt(row.path, row.pairSide);
            row.current = channel < 0 ? "None" : channel;
            const shape = Settings.deviceShape(row.text(row.channelDevicePath));
            const count = shape.channels === undefined ? 0 : Number(shape.channels);
            const list = [];
            for (let index = 0; index < count; ++index) {
                list.push(String(index));
            }
            if (row.allowNone) {
                list.push("None");
            }
            row.channelChoices = list;
        } else if (row.path !== "") {
            row.current = Settings.value(row.path);
        }
        if (row.gatePath !== "") {
            row.gateValue = row.text(row.gatePath);
        }
        if (row.alsoPath !== "") {
            // Read here, not in the binding: an invokable called from a binding answers once and never again.
            row.alsoCurrent = row.text(row.alsoPath);
        }
        if (row.kind === "device") {
            row.engineDevices = Settings.devices();
        } else if (row.kind === "pair") {
            // The pairs belong to the device the DJ picked above: a pair list that ignores the device is a list of
            // channel numbers that may not exist on the card.
            const device = row.pairDevicePath === "" ? "" : row.text(row.pairDevicePath);
            row.enginePairs = Settings.devicePairs(device);
        }
    }

    function write(value) {
        if (row.path === "" || !Settings) {
            return;
        }
        if (row.pairSide >= 0) {
            const channel = String(value) === "None" ? -1 : Number(value);
            if (Settings.setChannelAt(row.path, row.pairSide, channel)) {
                row.reload();
                row.syncChoice();
                row.edited(value);
            }
            return;
        }
        if (Settings.set(row.path, value)) {
            row.reload();
            row.syncChoice();
            row.edited(value);
        }
    }

    Component.onCompleted: row.reload()
    Connections {
        target: Settings
        function onChanged() { row.reload() }
    }

    RowLayout {
        anchors.fill: parent
        spacing: 8

        Label {
            Layout.preferredWidth: 200
            text: row.label
            color: row.open ? UiSettings.themeText : UiSettings.themeTextDim
            font.pixelSize: 11
            elide: Text.ElideRight
        }

        // A switch, for what is on or off.
        Switch {
            visible: row.kind === "switch"
            enabled: row.open
            Layout.preferredWidth: 60
            checked: row.current === true
            onToggled: row.write(checked)
        }

        // A list of named choices. It carries text; the owner writes back the type the file already had.
        ComboBox {
            id: choice
            visible: row.asList
            enabled: row.open
            Layout.fillWidth: true
            Layout.maximumWidth: 320
            model: row.offered
            font.pixelSize: 11
            Component.onCompleted: row.syncChoice()
            onActivated: row.write(row.numeric ? Number(currentText) : currentText)
        }

        // A slider for a share between two ends, with the number beside it.
        Slider {
            visible: row.kind === "slider"
            enabled: row.open
            Layout.fillWidth: true
            Layout.maximumWidth: 220
            from: row.from
            to: row.to
            stepSize: 1 / Math.pow(10, row.decimals)
            value: row.current === undefined ? row.from : Number(row.current)
            onMoved: row.write(value)
        }

        Label {
            visible: row.kind === "slider"
            Layout.preferredWidth: 48
            horizontalAlignment: Text.AlignRight
            text: row.current === undefined ? "" : Number(row.current).toFixed(row.decimals)
            color: UiSettings.themeTextDim
            font.pixelSize: 11
        }

        SpinBox {
            visible: row.kind === "number"
            enabled: row.open
            Layout.preferredWidth: 120
            from: Math.round(row.from)
            to: Math.round(row.to)
            value: row.current === undefined ? Math.round(row.from) : Math.round(Number(row.current))
            editable: true
            onValueModified: row.write(value)
        }

        // A device the engine is asked about before the DJ commits a name to a file read at start.
        Button {
            visible: row.kind === "check"
            enabled: row.open
            text: qsTr("Check")
            font.pixelSize: 10
            padding: 3
            onClicked: row.answer = Settings.describeDevice(String(row.current !== undefined
                                                                 ? row.current : ""))
        }
        Label {
            visible: row.kind === "check"
            Layout.fillWidth: true
            text: row.answer
            color: row.answer.startsWith(qsTr("no answer")) ? UiSettings.themeWarning : UiSettings.themeTextDim
            font.pixelSize: 10
            elide: Text.ElideRight
        }

        TextField {
            visible: row.asField
            enabled: row.open
            Layout.fillWidth: true
            Layout.maximumWidth: 300
            font.pixelSize: 11
            text: row.currentText
            placeholderText: qsTr("by default")
            onEditingFinished: row.write(text)
        }

        Label {
            Layout.fillWidth: true
            // Why a device is typed rather than chosen: the engine could not be asked, and saying so is better than a
            // list that silently is not one.
            text: !row.open
                  ? qsTr("chosen by the mode")
                  : (row.restart
                     ? qsTr("after restart")
                     : (row.asField && row.kind !== "text"
                        ? qsTr("the engine named nothing; type it") : ""))
            color: UiSettings.themeTextDim
            font.pixelSize: 9
            elide: Text.ElideRight
        }
    }

    // Keeping the list on the value the file has, rather than on the first entry: an unset key must not show as if
    // the first choice had been made.
    function syncChoice() {
        if (!row.asList) {
            return;
        }
        const index = row.offered.indexOf(row.currentText);
        choice.currentIndex = index >= 0 ? index : 0;
        if (row.pairSide >= 0 && row.current !== undefined) {
            // A channel row shows what it has, and "None" when it has nothing: an empty list is not a state.
            choice.currentIndex = row.offered.indexOf(row.currentText);
        }
    }
}
