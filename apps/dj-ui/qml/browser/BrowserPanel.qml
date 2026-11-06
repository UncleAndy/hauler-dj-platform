// Browser: the folder the user is looking at and its tracks (DJP-MEDIA-001).
//
// A click selects a track, a double click loads it into the deck chosen on the right, and a folder
// opens on the same gesture. Selection is a real state here: the row stays lit, the header shows what
// is chosen, and the Load button acts on it — a list you cannot pick from is a picture of a list.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Pane {
    id: browser
    objectName: "browserPanel"

    // Which deck a double click loads into; the header shows it, nothing is assumed.
    property string target: "A"
    // What the last activation did: read by --browser-check, so the click path can be measured.
    property string lastAction: ""

    background: Rectangle {
        color: UiSettings.themePanel
        border.color: UiSettings.themeBorder
        border.width: 1
    }

    readonly property var selected: list.currentIndex >= 0 && list.currentIndex < Browser.entries.length
                                    ? Browser.entries[list.currentIndex] : null

    // The one place a row acts: the mouse and the Load button both come through here, so a check can
    // call it and exercise the same path the user does.
    function activate(index) {
        if (index < 0 || index >= Browser.entries.length) {
            browser.lastAction = "nothing at " + index;
            return false;
        }
        const entry = Browser.entries[index];
        if (entry.dir === true) {
            Browser.open(entry.path);
            browser.lastAction = "opened " + entry.path;
            return true;
        }
        Bridge.load(browser.target, entry.path);
        browser.lastAction = "loaded " + entry.path + " into " + browser.target;
        return true;
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
                font.pixelSize: 10
                enabled: Browser.entries.length > 0
                onClicked: Browser.up()
            }
            Label {
                text: qsTr("→")
                color: UiSettings.themeTextDim
                font.pixelSize: 10
            }
            Repeater {
                model: ["A", "B", "C", "D"]
                delegate: Button {
                    required property string modelData
                    objectName: "deckButton_" + modelData
                    text: modelData
                    font.pixelSize: 9
                    padding: 2
                    implicitWidth: 20
                    implicitHeight: 18
                    checkable: true
                    checked: browser.target === modelData
                    onClicked: {
                        browser.target = modelData;
                        // A deck button is a load button when something is chosen: pick the deck and
                        // the track together instead of asking for a second click.
                        if (browser.selected !== null && browser.selected.dir !== true) {
                            browser.activate(list.currentIndex);
                        }
                    }
                }
            }
        }

        ListView {
            id: list
            objectName: "browserList"
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            model: Browser.entries
            boundsBehavior: Flickable.StopAtBounds
            keyNavigationWraps: false
            focus: true
            ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }
            // A click selects; Enter or a double click acts, as in any file list.
            Keys.onReturnPressed: browser.activate(list.currentIndex)
            Keys.onEnterPressed: browser.activate(list.currentIndex)

            delegate: Rectangle {
                id: row
                required property var modelData
                required property int index
                width: list.width
                height: 18
                readonly property bool isFolder: row.modelData.dir === true
                readonly property bool current: list.currentIndex === row.index
                color: row.current ? UiSettings.themeAccentDim
                                   : (row.index % 2 === 0 ? UiSettings.themePanel
                                                          : UiSettings.themeBackground)

                Label {
                    anchors.left: parent.left
                    anchors.leftMargin: 6
                    anchors.right: parent.right
                    anchors.rightMargin: 6
                    anchors.verticalCenter: parent.verticalCenter
                    text: (row.isFolder ? "▸ " : "  ") + row.modelData.name
                    color: row.isFolder ? UiSettings.themeText
                                        : (row.current ? UiSettings.themePlayhead
                                                       : UiSettings.themeTextDim)
                    font.pixelSize: 11
                    elide: Text.ElideRight
                }
                MouseArea {
                    anchors.fill: parent
                    acceptedButtons: Qt.LeftButton
                    onClicked: {
                        list.currentIndex = row.index;
                        list.forceActiveFocus();
                    }
                    onDoubleClicked: browser.activate(row.index)
                }
            }
        }

        RowLayout {
            Layout.fillWidth: true
            spacing: 6
            Label {
                Layout.fillWidth: true
                text: browser.selected !== null
                      ? qsTr("%1 → deck %2").arg(browser.selected.name).arg(browser.target)
                      : qsTr("nothing selected")
                color: UiSettings.themeTextDim
                font.pixelSize: 10
                elide: Text.ElideRight
            }
            Button {
                text: qsTr("Load")
                font.pixelSize: 10
                enabled: browser.selected !== null && browser.selected.dir !== true
                onClicked: browser.activate(list.currentIndex)
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
