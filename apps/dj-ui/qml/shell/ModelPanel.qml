// The external beat model (ADR-0012): what is installed, what it costs, and what it is licensed under.
//
// The licence and the attribution are shown here, before any button sends a command: the weights are
// downloaded into the user's store on request, and a screen that downloads first and explains later is
// not attribution. Nothing on this panel is required for the app to work — without a model the built-in
// detector measures the tempo by itself.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Dialog {
    id: panel
    objectName: "modelPanel"
    title: qsTr("Beat model")
    modal: true
    width: 560
    // По центру ОКНА, а не полосы: плашка объявлена внутри верхней полосы, и без этого диалог
    // встаёт по её центру — то есть у самого верха экрана.
    parent: Overlay.overlay
    anchors.centerIn: Overlay.overlay
    standardButtons: Dialog.Close

    /// What the engine last published about the model; empty until one is installed.
    readonly property var status: Bridge.snapshot.model !== undefined ? Bridge.snapshot.model : ({})
    readonly property string installed: status.variant !== undefined ? status.variant : ""
    readonly property bool busy: status.busy === true
    readonly property string failure: status.error !== undefined ? status.error : ""

    readonly property string licence: "MIT License\n\n"
        + "Copyright (c) 2024 Jan Schlüter, Sebastian Böck, and the Beat This! contributors,\n"
        + "Institute of Computational Perception, Johannes Kepler University Linz.\n\n"
        + "Permission is hereby granted, free of charge, to any person obtaining a copy of this software "
        + "and associated documentation files (the \"Software\"), to deal in the Software without "
        + "restriction, including without limitation the rights to use, copy, modify, merge, publish, "
        + "distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the "
        + "Software is furnished to do so, subject to the following conditions:\n\n"
        + "The above copyright notice and this permission notice shall be included in all copies or "
        + "substantial portions of the Software.\n\n"
        + "THE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING "
        + "BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND "
        + "NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, "
        + "DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, "
        + "OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.\n\n"
        + "Model: Beat This! (ISMIR 2024), https://github.com/CPJKU/beat_this\n"
        + "Paper: Böck, Schlüter et al., \"Beat This! Accurate Beat Tracking Without DBN Postprocessing\".\n"
        + "Weights are converted to ONNX unchanged; the conversion is not distributed as the original."

    // The body carries a name of its own: the dialog itself is a Popup, not an item, so a check that
    // walks the item tree can only find what is inside it.
    contentItem: ColumnLayout {
        objectName: "modelPanelBody"
        spacing: 10

        Label {
            Layout.fillWidth: true
            wrapMode: Text.WordWrap
            text: panel.installed.length > 0
                  ? qsTr("Installed: %1 (%2). The model names the tempo; the beat grid is still built here.")
                        .arg(panel.installed).arg(panel.status.license)
                  : qsTr("No model installed. The built-in detector measures the tempo by itself.")
        }
        Label {
            Layout.fillWidth: true
            wrapMode: Text.WordWrap
            font.pixelSize: 11
            color: UiSettings.themeTextDim
            text: qsTr("The model is optional and never ships with the app: it is downloaded into your own data "
                       + "directory, checked against its size and SHA-256, and can be deleted again. "
                       + "small is 10,6 MB (about 3,5 s per track), full is 83,2 MB (about 5,9 s).")
        }
        Label {
            Layout.fillWidth: true
            wrapMode: Text.WordWrap
            visible: panel.busy
            text: qsTr("Working… the engine is downloading or removing the file.")
        }
        Label {
            Layout.fillWidth: true
            wrapMode: Text.WordWrap
            visible: panel.failure.length > 0
            color: UiSettings.themeDanger
            text: panel.failure
        }

        ScrollView {
            Layout.fillWidth: true
            Layout.preferredHeight: 180
            clip: true
            TextArea {
                objectName: "modelLicence"
                readOnly: true
                font.family: "monospace"
                font.pixelSize: 10
                wrapMode: TextArea.Wrap
                text: panel.licence
            }
        }

        RowLayout {
            Layout.fillWidth: true
            spacing: 8
            Button {
                objectName: "modelDownloadSmall"
                text: qsTr("Download small")
                enabled: !panel.busy && panel.installed !== "small"
                onClicked: Bridge.downloadModel("small")
            }
            Button {
                objectName: "modelDownloadFull"
                text: qsTr("Download full")
                enabled: !panel.busy && panel.installed !== "full"
                onClicked: Bridge.downloadModel("full")
            }
            Button {
                objectName: "modelRemove"
                text: qsTr("Delete")
                enabled: !panel.busy && panel.installed.length > 0
                onClicked: Bridge.removeModel(panel.installed)
            }
            Item { Layout.fillWidth: true }
        }
    }
}
