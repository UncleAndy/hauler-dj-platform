// Browser: the folder the user is looking at and its tracks (DJP-MEDIA-001).
//
// A track row loads into the deck that was last given focus, so the browser is one list for four
// decks rather than four lists. The target deck is shown, not assumed.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Pane {
    id: browser

    // Which deck a double-click loads into; the deck panels set it when they are clicked.
    property string target: "A"

    background: Rectangle {
        color: UiSettings.themePanel
        border.color: UiSettings.themeBorder
        border.width: 1
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 6
        spacing: 4

        RowLayout {
            Layout.fillWidth: true
            spacing: 6
            Label {
                text: qsTr("Browser")
                font.bold: true
                color: UiSettings.themeText
            }
            Label {
                Layout.fillWidth: true
                text: Browser.folder
                elide: Text.ElideMiddle
                color: UiSettings.themeTextDim
                font.pixelSize: 10
            }
            Button {
                text: qsTr("Up")
                enabled: Browser.entries.length > 0
                onClicked: Browser.up()
            }
            RowLayout {
                spacing: 2
                Label {
                    text: qsTr("→")
                    color: UiSettings.themeTextDim
                    font.pixelSize: 10
                }
                Repeater {
                    model: ["A", "B", "C", "D"]
                    delegate: Button {
                        required property string modelData
                        text: modelData
                        font.pixelSize: 9
                        padding: 2
                        implicitWidth: 20
                        implicitHeight: 18
                        checkable: true
                        checked: browser.target === modelData
                        onClicked: browser.target = modelData
                    }
                }
            }
        }

        ListView {
            id: list
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            model: Browser.entries
            boundsBehavior: Flickable.StopAtBounds
            ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }

            delegate: Rectangle {
                id: row
                required property var modelData
                required property int index
                width: list.width
                height: 20
                color: row.index % 2 === 0 ? UiSettings.themePanel : UiSettings.themeBackground
                readonly property bool directory: row.modelData.directory === true

                Label {
                    anchors.left: parent.left
                    anchors.leftMargin: 6
                    anchors.verticalCenter: parent.verticalCenter
                    text: (row.directory ? "▸ " : "  ") + row.modelData.name
                    color: row.directory ? UiSettings.themeText : UiSettings.themeTextDim
                    font.pixelSize: 11
                    elide: Text.ElideRight
                    width: parent.width - 12
                }
                MouseArea {
                    anchors.fill: parent
                    onDoubleClicked: {
                        if (row.directory) {
                            Browser.open(row.modelData.path);
                        } else {
                            Bridge.load(browser.target, row.modelData.path);
                        }
                    }
                }
            }
        }

        Label {
            visible: Browser.entries.length === 0
            text: Browser.error.length > 0 ? Browser.error : qsTr("no tracks here")
            color: UiSettings.themeWaveOutline
            font.pixelSize: 11
        }
    }
}
