// In-app file browser for loading a track.
//
// Deliberately not Qt's FileDialog: on Linux that is the native GTK/portal dialog, and a missing
// `org.gtk.Settings.FileChooser` schema aborts the whole process with a GLib-GIO error. A missing
// desktop package must not take the audio UI down. The listing comes from `Browser` (QDir in C++),
// so nothing beyond QtQuick is needed — the QML folder model is not built into this Qt either.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Dialog {
    id: dialog

    required property string deckId
    required property string deckLabel

    title: qsTr("Load a track into %1").arg(dialog.deckLabel)
    modal: true
    standardButtons: Dialog.Close
    width: 720
    height: 520

    signal picked(string path)

    onOpened: Browser.open("")

    ColumnLayout {
        anchors.fill: parent
        spacing: 8

        RowLayout {
            Layout.fillWidth: true
            spacing: 8
            Button {
                text: qsTr("Up")
                onClicked: Browser.up()
            }
            Label {
                Layout.fillWidth: true
                text: Browser.folder
                elide: Text.ElideLeft
                opacity: 0.7
                font.pixelSize: 11
            }
        }

        ListView {
            id: list
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            model: Browser.entries
            ScrollBar.vertical: ScrollBar {}
            delegate: ItemDelegate {
                width: list.width
                text: modelData.dir ? "▸ " + modelData.name : modelData.name
                onClicked: {
                    if (modelData.dir) {
                        Browser.open(modelData.path);
                    } else {
                        dialog.picked(modelData.path);
                        dialog.close();
                    }
                }
            }
        }

        Label {
            Layout.fillWidth: true
            text: Browser.error.length > 0
                  ? Browser.error
                  : qsTr("%1 entries · audio files only").arg(Browser.entries.length)
            color: Browser.error.length > 0 ? UiSettings.themeDanger : UiSettings.themeTextDim
            opacity: 0.6
            font.pixelSize: 11
        }
    }
}
