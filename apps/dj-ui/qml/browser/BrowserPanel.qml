// Browser: the folder the user is looking at and its tracks (DJP-MEDIA-001).
//
// A click selects a track, a double click loads it into the deck chosen on the right, and a folder
// opens on the same gesture. Selection is a real state here: the row stays lit, the header shows what
// is chosen, and the Load button acts on it — a list you cannot pick from is a picture of a list.
//
// The way up is the first row of the list (".."), not a button of its own: that is how every file
// manager on this desktop works, and a separate button is a second way to say the same thing.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

import "../mixer"

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

    // Loading is deliberate: a deck button, or the number key of that deck. A double click only
    // opens a folder — it never puts a track into a deck, because a stray double click during a set
    // is a ruined transition.
    readonly property var deckIds: ["A", "B", "C", "D"]

    // Size of the list's text, in pixels. The range is generous on purpose: 8 is a dense list for a
    // wide screen, 20 is a list read from across the booth.
    property int listFont: 11
    readonly property int minListFont: 8
    readonly property int maxListFont: 20
    // The row grows with its text: a bigger font in a fixed row is a clipped font.
    readonly property int rowHeight: browser.listFont + 7

    // Width of the sections rail, in pixels, and the range a drag may leave it in: a rail narrower
    // than its longest word is a smudge, a rail wider than the download is a list with no room.
    property int railWidth: 92
    readonly property int minRailWidth: 72
    readonly property int maxRailWidth: 260

    // Section text follows the list's text: one control for the browser's type, not two.
    readonly property int sectionFont: Math.max(8, browser.listFont - 1)

    // The sections rail. Today there is one section; the collection and the playlists are the next
    // two entries and need no new layout, only a row in sectionModel.

    function stepFont(delta) {
        browser.listFont = Math.max(browser.minListFont,
                                    Math.min(browser.maxListFont, browser.listFont + delta));
    }

    function deckIsPlaying(id) {
        const decks = Bridge.snapshot.decks;
        if (decks === undefined || decks === null) {
            return false;
        }
        for (let index = 0; index < decks.length; ++index) {
            if (decks[index].deck === id) {
                return decks[index].playing === true;
            }
        }
        return false;
    }

    // Double click and Enter on a folder: open it. Nothing else acts on a double click.
    function openFolder(index) {
        if (index < 0 || index >= Browser.entries.length) {
            return false;
        }
        const entry = Browser.entries[index];
        if (entry.dir !== true) {
            browser.lastAction = "not a folder";
            return false;
        }
        Browser.open(entry.path);
        browser.lastAction = "opened " + entry.path;
        return true;
    }

    // Load the selected track into `deck`; refuses a deck that is playing rather than cutting the
    // track the room is dancing to.
    function loadInto(deck) {
        if (browser.selected === null || browser.selected.dir === true) {
            browser.lastAction = "nothing to load";
            return false;
        }
        if (browser.deckIsPlaying(deck)) {
            browser.lastAction = "deck " + deck + " is playing";
            return false;
        }
        browser.target = deck;
        Bridge.load(deck, browser.selected.path);
        browser.lastAction = "loaded " + browser.selected.path + " into " + deck;
        return true;
    }

    RowLayout {
        anchors.fill: parent
        anchors.margins: 6
        spacing: 6

        // The rail of sections: Files is the list on the right; the collection and the playlists will
        // be their own views behind their own entries.
        ColumnLayout {
            objectName: "browserSections"
            // The rail takes exactly what the divider gives it: a layout that may stretch it would
            // make the divider a decoration.
            Layout.preferredWidth: browser.railWidth
            Layout.minimumWidth: browser.railWidth
            Layout.maximumWidth: browser.railWidth
            Layout.fillHeight: true
            spacing: 2

            Label {
                objectName: "sectionHeading"
                text: qsTr("Sections")
                font.pixelSize: Math.max(8, browser.sectionFont - 1)
                font.bold: true
                color: UiSettings.themeTextDim
            }
            ListModel {
                id: sectionModel
                ListElement {
                    key: "files"
                    title: "Files"
                    ready: true
                }
                ListElement {
                    key: "collection"
                    title: "Collection"
                    ready: false
                }
                ListElement {
                    key: "playlists"
                    title: "Playlists"
                    ready: false
                }
            }
            Repeater {
                model: sectionModel
                delegate: Rectangle {
                    required property string key
                    required property string title
                    required property bool ready
                    objectName: "section_" + key
                    Layout.fillWidth: true
                    height: browser.rowHeight
                    // Only Files exists today; the others are dimmed until they do, rather than
                    // pretending to be a place that holds nothing.
                    color: ready ? UiSettings.themeAccentDim : "transparent"
                    radius: 2
                    Label {
                        objectName: "sectionLabel_" + key
                        anchors.left: parent.left
                        anchors.leftMargin: 6
                        anchors.verticalCenter: parent.verticalCenter
                        width: parent.width - 8
                        text: title
                        font.pixelSize: browser.sectionFont
                        elide: Text.ElideRight
                        color: ready ? UiSettings.themeText : UiSettings.themeWaveOutline
                    }
                }
            }
            Item {
                Layout.fillHeight: true
            }
        }

        // The divider: its width is the rail's width, and dragging it is the only way to change it.
        Rectangle {
            id: railSplitter
            objectName: "browserRailSplitter"
            Layout.fillHeight: true
            Layout.preferredWidth: 5
            color: railDrag.pressed ? UiSettings.themeAccent : UiSettings.themeBorder

            MouseArea {
                id: railDrag
                anchors.fill: parent
                anchors.leftMargin: -3
                anchors.rightMargin: -3
                cursorShape: Qt.SplitHCursor
                property real anchorX: 0
                property int anchorWidth: 0
                onPressed: {
                    railDrag.anchorX = mouse.x;
                    railDrag.anchorWidth = browser.railWidth;
                }
                onPositionChanged: {
                    if (!railDrag.pressed) {
                        return;
                    }
                    const moved = Math.round(mouse.x - railDrag.anchorX);
                    browser.railWidth = Math.max(browser.minRailWidth,
                                                 Math.min(browser.maxRailWidth,
                                                          railDrag.anchorWidth + moved));
                }
            }
        }

        ColumnLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
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
                // The text size, changed where it is read: a browser whose font cannot be changed is
                // a browser for one pair of eyes.
                Button {
                    objectName: "fontMinus"
                    text: qsTr("A−")
                    font.pixelSize: 9
                    padding: 2
                    implicitWidth: 24
                    implicitHeight: 18
                    enabled: browser.listFont > browser.minListFont
                    onClicked: browser.stepFont(-1)
                }
                Button {
                    objectName: "fontPlus"
                    text: qsTr("A+")
                    font.pixelSize: 9
                    padding: 2
                    implicitWidth: 24
                    implicitHeight: 18
                    enabled: browser.listFont < browser.maxListFont
                    onClicked: browser.stepFont(1)
                }
                // The target is named in words, not painted into one of the buttons: at rest every
                // deck button looks the same, and only a playing deck stands out.
                Label {
                    text: qsTr("→ %1").arg(browser.target)
                    color: UiSettings.themeTextDim
                    font.pixelSize: 10
                }
                Repeater {
                    model: ["A", "B", "C", "D"]
                    delegate: RefusalButton {
                        required property string modelData
                        objectName: "deckButton_" + modelData
                        text: modelData
                        font.pixelSize: 9
                        padding: 2
                        implicitWidth: 20
                        implicitHeight: 18
                        // Blue for a playing deck: the browser tells the truth about the decks it feeds.
                        // The chosen target is not a colour of its own — it is written in the header.
                        playing: browser.deckIsPlaying(modelData)
                        // Not disabled: pressing a playing deck's button is how a person learns why
                        // nothing happened. It blinks red instead of doing nothing at all.
                        ToolTip.visible: hovered && browser.deckIsPlaying(modelData)
                        ToolTip.text: qsTr("deck %1 is playing").arg(modelData)
                        onClicked: {
                            if (!browser.loadInto(modelData)) {
                                refuse();
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
                // A folder opens with Enter; a track does not, because Enter is not a load key here.
                Keys.onReturnPressed: browser.openFolder(list.currentIndex)
                Keys.onEnterPressed: browser.openFolder(list.currentIndex)
                Keys.onPressed: function (event) {
                    const deck = ["1", "2", "3", "4"].indexOf(event.text);
                    if (deck >= 0) {
                        browser.loadInto(browser.deckIds[deck]);
                        event.accepted = true;
                    }
                }

                delegate: Rectangle {
                    id: row
                    required property var modelData
                    required property int index
                    width: list.width
                    height: browser.rowHeight
                    readonly property bool isFolder: row.modelData.dir === true
                    readonly property bool isParent: row.modelData.parent === true
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
                        // ".." is the way up and is written as such: no folder arrow, because it is
                        // not a folder you can do anything else with.
                        text: row.isParent ? ".." : (row.isFolder ? "▸ " : "  ") + row.modelData.name
                        color: row.isParent
                               ? (row.current ? UiSettings.themePlayhead : UiSettings.themeTextDim)
                               : (row.isFolder ? UiSettings.themeText
                                               : (row.current ? UiSettings.themePlayhead
                                                              : UiSettings.themeTextDim))
                        font.pixelSize: browser.listFont
                        elide: Text.ElideRight
                    }
                    MouseArea {
                        anchors.fill: parent
                        acceptedButtons: Qt.LeftButton
                        onClicked: {
                            list.currentIndex = row.index;
                            list.forceActiveFocus();
                        }
                        onDoubleClicked: {
                            // Folders only: opening one is navigation. A track needs a deck button or
                            // a number key, so a double click cannot cut what is playing.
                            if (row.isFolder) {
                                browser.openFolder(row.index);
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
}
