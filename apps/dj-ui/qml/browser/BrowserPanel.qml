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
    // Which section the list is showing. The browser object holds the mode; this is its name, so the
    // rail can light the row the list belongs to instead of guessing.
    readonly property string section: Browser.mode === "collection" ? "collection" : "files"

    // The rows the DJ picked: a list of paths, so a folder and a file can travel to the collection
    // together. Ctrl toggles one, Shift takes the run between the anchor and the click.
    property var selection: []
    property int anchorIndex: -1

    // What the engine says about the import, if one was started since it came up.
    readonly property var importState: Bridge.snapshot !== undefined && Bridge.snapshot.import !== undefined
                                       ? Bridge.snapshot.import : null
    readonly property var importQuestion: browser.importState !== null
                                          && browser.importState.question !== undefined
                                          ? browser.importState.question : null

    function isChosen(path) {
        return browser.selection.indexOf(path) >= 0;
    }

    function choose(index, modifiers) {
        if (index < 0 || index >= Browser.entries.length) {
            return;
        }
        const path = Browser.entries[index].path;
        let chosen = browser.selection.slice();
        if ((modifiers & Qt.ShiftModifier) !== 0 && browser.anchorIndex >= 0) {
            const from = Math.min(browser.anchorIndex, index);
            const to = Math.max(browser.anchorIndex, index);
            chosen = [];
            for (let row = from; row <= to; ++row) {
                chosen.push(Browser.entries[row].path);
            }
        } else if ((modifiers & Qt.ControlModifier) !== 0) {
            const at = chosen.indexOf(path);
            if (at >= 0) {
                chosen.splice(at, 1);
            } else {
                chosen.push(path);
            }
            browser.anchorIndex = index;
        } else {
            chosen = [path];
            browser.anchorIndex = index;
        }
        browser.selection = chosen;
    }

    // One file goes straight in: the confirmation exists for batches, not for a single click.
    function askImport() {
        const chosen = browser.selection.slice();
        if (chosen.length === 0) {
            browser.lastAction = "nothing to add";
            return false;
        }
        if (chosen.length === 1 && Browser.entryIsFile(chosen[0])) {
            browser.startImport(chosen, true);
            return true;
        }
        importDialog.open();
        return true;
    }

    function startImport(paths, parallel) {
        Bridge.importPaths(paths, LayoutState.roomiestRoot(), parallel);
        browser.lastAction = "import " + paths.length + " into " + LayoutState.roomiestRoot();
    }

    /// One line of facts for the duplicate dialog: what the two copies are, in numbers a DJ reads.
    function factsLine(facts) {
        if (facts === undefined || facts === null) {
            return "";
        }
        const parts = [];
        if (facts.format !== undefined && facts.format.length > 0) {
            parts.push(facts.format);
        }
        if (facts.lossless === true) {
            parts.push("lossless");
        }
        if (facts.sample_rate > 0) {
            parts.push((facts.sample_rate / 1000).toFixed(1) + " kHz");
        }
        if (facts.bits > 0) {
            parts.push(facts.bits + " bit");
        }
        if (facts.bitrate_kbps > 0) {
            parts.push(Math.round(facts.bitrate_kbps) + " kbps");
        }
        const seconds = Math.round(facts.duration_ms / 1000);
        parts.push(Math.floor(seconds / 60) + ":" + ("0" + (seconds % 60)).slice(-2));
        parts.push((facts.size_bytes / (1024 * 1024)).toFixed(1) + " MB");
        return parts.join(" · ");
    }

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
    // The list's text size and the rail's width are the DJ's settings, not the file's: they are read
    // from the layout file at start and written back when they change (DJP-UI-001).
    property int listFont: LayoutState.browserFont
    onListFontChanged: LayoutState.browserFont = browser.listFont
    readonly property int minListFont: 8
    readonly property int maxListFont: 20
    // The row grows with its text: a bigger font in a fixed row is a clipped font.
    readonly property int rowHeight: browser.listFont + 7

    // Width of the sections rail, in pixels, and the range a drag may leave it in: a rail narrower
    // than its longest word is a smudge, a rail wider than the download is a list with no room.
    property int railWidth: LayoutState.browserRailWidth
    onRailWidthChanged: LayoutState.browserRailWidth = browser.railWidth
    readonly property int minRailWidth: 72
    readonly property int maxRailWidth: 260

    // Section text follows the list's text: one control for the browser's type, not two.
    readonly property int sectionFont: Math.max(8, browser.listFont - 1)

    /// The header is part of the browser, not a widget bolted on top of it: the title, the path, the two
    /// text-size buttons and the deck buttons all follow the same type size. A browser whose list grows
    /// while its own controls stay small is two panels stacked, and the hand keeps missing the small one.
    readonly property int titleFont: Math.max(10, browser.listFont + 1)
    readonly property int headerFont: Math.max(9, browser.listFont)
    readonly property int headerButtonFont: Math.max(9, browser.listFont - 1)
    readonly property int headerButtonHeight: Math.max(18, browser.listFont + 10)
    readonly property int headerTextButtonWidth: Math.max(24, browser.listFont + 16)
    readonly property int headerDeckButtonWidth: Math.max(20, browser.listFont + 12)

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

    // A section is a place, not a label: pressing one moves the list there.
    function showSection(key) {
        if (key === "collection") {
            Browser.openCollection(LayoutState.collectionRoots);
            browser.lastAction = "collection " + Browser.folder;
            return true;
        }
        if (key === "files") {
            Browser.showFiles();
            browser.lastAction = "files " + Browser.folder;
            return true;
        }
        browser.lastAction = key + " is not built yet";
        return false;
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
        // Read once: a load moves the folder the browser shows, so the second read of the selection
        // would be a different row — and the panel would report a path it never sent.
        const path = browser.selected.path;
        browser.target = deck;
        Bridge.load(deck, path);
        browser.lastAction = "loaded " + path + " into " + deck;
        return true;
    }

    // The context menu: what a DJ does with the rows he picked. One entry today; the place is here
    // so the next one (analyse, reveal, remove) has somewhere to go.
    Menu {
        id: fileMenu
        objectName: "browserMenu"
        MenuItem {
            objectName: "addToCollection"
            text: qsTr("Add files to collection…")
            enabled: browser.selection.length > 0
            onClicked: browser.askImport()
        }
    }

    // The batch's confirmation: how many, and whether to use several workers. A single file skips it.
    Dialog {
        id: importDialog
        objectName: "importDialog"
        title: qsTr("Add to collection")
        modal: true
        anchors.centerIn: Overlay.overlay
        standardButtons: Dialog.NoButton
        ColumnLayout {
            spacing: 8
            Label {
                objectName: "importCount"
                text: qsTr("%1 item(s) selected").arg(browser.selection.length)
                color: UiSettings.themeText
                font.pixelSize: browser.listFont
            }
            Label {
                objectName: "importRoot"
                text: qsTr("into %1").arg(LayoutState.roomiestRoot())
                color: UiSettings.themeTextDim
                font.pixelSize: browser.listFont
            }
            CheckBox {
                id: parallelBox
                objectName: "importParallel"
                text: qsTr("Выполнять параллельно")
                checked: true
                font.pixelSize: browser.listFont
            }
        }
        footer: RowLayout {
            spacing: 6
            Item { Layout.fillWidth: true }
            Button {
                objectName: "importCancel"
                text: qsTr("Отменить")
                font.pixelSize: browser.listFont
                onClicked: importDialog.close()
            }
            Button {
                objectName: "importConfirm"
                text: qsTr("Добавить")
                font.pixelSize: browser.listFont
                onClicked: {
                    browser.startImport(browser.selection, parallelBox.checked);
                    importDialog.close();
                }
            }
        }
    }

    // The batch at work: the counts come from the engine's snapshot, not from this panel.
    Dialog {
        id: importProgress
        objectName: "importProgress"
        title: qsTr("Adding to collection")
        modal: true
        anchors.centerIn: Overlay.overlay
        closePolicy: Popup.NoAutoClose
        standardButtons: Dialog.NoButton
        visible: browser.importState !== null && browser.importState.running === true
                 && browser.importQuestion === null
        ColumnLayout {
            spacing: 8
            Label {
                objectName: "importProgressText"
                text: qsTr("%1 of %2 added, %3 skipped, %4 failed")
                      .arg(browser.importState !== null ? browser.importState.done : 0)
                      .arg(browser.importState !== null ? browser.importState.total : 0)
                      .arg(browser.importState !== null ? browser.importState.skipped : 0)
                      .arg(browser.importState !== null ? browser.importState.failed : 0)
                color: UiSettings.themeText
                font.pixelSize: browser.listFont
            }
            Label {
                objectName: "importProgressCurrent"
                text: browser.importState !== null && browser.importState.current !== undefined
                      ? browser.importState.current : ""
                elide: Text.ElideMiddle
                Layout.preferredWidth: 320
                color: UiSettings.themeTextDim
                font.pixelSize: browser.listFont
            }
            ProgressBar {
                objectName: "importProgressBar"
                from: 0
                to: browser.importState !== null ? Math.max(1, browser.importState.total) : 1
                value: browser.importState !== null ? browser.importState.done : 0
                Layout.fillWidth: true
            }
        }
        footer: RowLayout {
            Item { Layout.fillWidth: true }
            Button {
                objectName: "importStop"
                text: qsTr("Отменить")
                font.pixelSize: browser.listFont
                onClicked: Bridge.cancelImport()
            }
        }
    }

    // The duplicate: both copies in numbers, and four answers that mean different things.
    Dialog {
        id: duplicateDialog
        objectName: "duplicateDialog"
        title: qsTr("This track is already in the collection")
        modal: true
        anchors.centerIn: Overlay.overlay
        closePolicy: Popup.NoAutoClose
        standardButtons: Dialog.NoButton
        visible: browser.importQuestion !== null
        ColumnLayout {
            spacing: 6
            Label {
                objectName: "duplicateAgreement"
                text: qsTr("match %1 %").arg(browser.importQuestion !== null
                                             ? Math.round(browser.importQuestion.agreement * 100) : 0)
                color: UiSettings.themeTextDim
                font.pixelSize: browser.listFont
            }
            Label {
                objectName: "duplicateExisting"
                text: qsTr("In the collection: %1").arg(browser.importQuestion !== null
                                                        ? browser.importQuestion.existing.name : "")
                color: UiSettings.themeText
                font.pixelSize: browser.listFont
            }
            Label {
                objectName: "duplicateExistingFacts"
                text: browser.importQuestion !== null
                      ? browser.factsLine(browser.importQuestion.existing) : ""
                color: UiSettings.themeTextDim
                font.pixelSize: browser.listFont
            }
            Label {
                objectName: "duplicateIncoming"
                text: qsTr("Adding: %1").arg(browser.importQuestion !== null
                                             ? browser.importQuestion.incoming.name : "")
                color: UiSettings.themeText
                font.pixelSize: browser.listFont
            }
            Label {
                objectName: "duplicateIncomingFacts"
                text: browser.importQuestion !== null
                      ? browser.factsLine(browser.importQuestion.incoming) : ""
                color: UiSettings.themeTextDim
                font.pixelSize: browser.listFont
            }
        }
        footer: RowLayout {
            spacing: 4
            Button {
                objectName: "duplicateSkip"
                text: qsTr("Пропустить")
                font.pixelSize: browser.listFont
                onClicked: Bridge.answerImport("skip")
            }
            Button {
                objectName: "duplicateReplace"
                text: qsTr("Заменить")
                font.pixelSize: browser.listFont
                onClicked: Bridge.answerImport("replace")
            }
            Button {
                objectName: "duplicateAdd"
                text: qsTr("Добавить")
                font.pixelSize: browser.listFont
                onClicked: Bridge.answerImport("add")
            }
            Button {
                objectName: "duplicateAddMarked"
                text: qsTr("Добавить с меткой")
                font.pixelSize: browser.listFont
                onClicked: Bridge.answerImport("add_marked")
            }
        }
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
                    ready: true
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
                    // The section the list is showing is lit. Playlists has nothing behind it yet, so
                    // it stays dim rather than pretending to be a place that holds nothing.
                    readonly property bool current: ready && browser.section === key
                    color: current ? UiSettings.themeAccentDim : "transparent"
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
                        color: current ? UiSettings.themeText
                                       : (ready ? UiSettings.themeTextDim
                                                : UiSettings.themeWaveOutline)
                    }
                    MouseArea {
                        anchors.fill: parent
                        enabled: ready
                        cursorShape: ready ? Qt.PointingHandCursor : Qt.ArrowCursor
                        onClicked: browser.showSection(key)
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
                    objectName: "browserTitle"
                    text: qsTr("Browser")
                    font.bold: true
                    font.pixelSize: browser.titleFont
                    color: UiSettings.themeText
                }
                Label {
                    objectName: "browserPath"
                    Layout.fillWidth: true
                    text: Browser.folder
                    elide: Text.ElideMiddle
                    color: UiSettings.themeTextDim
                    font.pixelSize: browser.headerFont
                }
                // How much the collection holds and how much of it the search is showing: a filter
                // that hides everything must not look like an empty collection.
                Label {
                    objectName: "collectionCount"
                    visible: browser.section === "collection"
                    text: Browser.filter.length > 0
                          ? qsTr("%1 of %2").arg(Browser.entries.length).arg(Browser.total)
                          : qsTr("%1 tracks").arg(Browser.total)
                    color: UiSettings.themeTextDim
                    font.pixelSize: browser.headerFont
                }
                // The text size, changed where it is read: a browser whose font cannot be changed is
                // a browser for one pair of eyes.
                Button {
                    objectName: "fontMinus"
                    text: qsTr("A−")
                    font.pixelSize: browser.headerButtonFont
                    padding: 2
                    implicitWidth: browser.headerTextButtonWidth
                    implicitHeight: browser.headerButtonHeight
                    enabled: browser.listFont > browser.minListFont
                    onClicked: browser.stepFont(-1)
                }
                Button {
                    objectName: "fontPlus"
                    text: qsTr("A+")
                    font.pixelSize: browser.headerButtonFont
                    padding: 2
                    implicitWidth: browser.headerTextButtonWidth
                    implicitHeight: browser.headerButtonHeight
                    enabled: browser.listFont < browser.maxListFont
                    onClicked: browser.stepFont(1)
                }
                // The target is named in words, not painted into one of the buttons: at rest every
                // deck button looks the same, and only a playing deck stands out.
                Label {
                    objectName: "browserTarget"
                    text: qsTr("→ %1").arg(browser.target)
                    color: UiSettings.themeTextDim
                    font.pixelSize: browser.headerFont
                }
                Repeater {
                    model: ["A", "B", "C", "D"]
                    delegate: RefusalButton {
                        required property string modelData
                        objectName: "deckButton_" + modelData
                        text: modelData
                        font.pixelSize: browser.headerButtonFont
                        padding: 2
                        implicitWidth: browser.headerDeckButtonWidth
                        implicitHeight: browser.headerButtonHeight
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

            // The search lives in the section that needs it: a field that is always there is a field
            // that means nothing in a folder.
            TextField {
                objectName: "collectionSearch"
                visible: browser.section === "collection"
                Layout.fillWidth: true
                font.pixelSize: browser.listFont
                selectByMouse: true
                placeholderText: qsTr("search artist or title")
                text: Browser.filter
                onTextChanged: {
                    // The filter is the browser's state, not the field's: writing it back from here
                    // would make the field a second source of truth for what the list shows.
                    if (text !== Browser.filter) {
                        Browser.setFilter(text);
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
                    readonly property bool chosen: browser.isChosen(row.modelData.path)
                    // A chosen row is the loudest thing in the list: the DJ must see what a batch
                    // will take before he adds it.
                    color: row.chosen ? UiSettings.themeAccent
                                      : (row.current ? UiSettings.themeAccentDim
                                                     : (row.index % 2 === 0 ? UiSettings.themePanel
                                                                            : UiSettings.themeBackground))

                    Label {
                        id: rowMeta
                        objectName: "browserMeta_" + row.index
                        anchors.right: parent.right
                        anchors.rightMargin: 6
                        anchors.verticalCenter: parent.verticalCenter
                        // Tempo and key travel with the file in its metadata; a collection row that
                        // shows neither makes the DJ open the track to learn what he already owns.
                        readonly property var tempo: row.modelData.bpm
                        readonly property var key: row.modelData.key
                        text: browser.section !== "collection" ? ""
                              : [tempo === undefined ? "" : Math.round(tempo) + " BPM",
                                 key === undefined ? "" : key]
                                .filter(part => part.length > 0).join(" · ")
                        color: UiSettings.themeWaveOutline
                        font.pixelSize: browser.listFont
                    }
                    Label {
                        anchors.left: parent.left
                        anchors.leftMargin: 6
                        anchors.right: rowMeta.text.length > 0 ? rowMeta.left : parent.right
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
                        acceptedButtons: Qt.LeftButton | Qt.RightButton
                        onClicked: function (mouse) {
                            list.currentIndex = row.index;
                            list.forceActiveFocus();
                            if (mouse.button === Qt.RightButton) {
                                // A right click on a row that is not chosen picks it first: a menu
                                // that acts on something else is a trap.
                                if (!row.chosen) {
                                    browser.choose(row.index, Qt.NoModifier);
                                }
                                fileMenu.popup();
                                return;
                            }
                            browser.choose(row.index, mouse.modifiers);
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
                font.pixelSize: browser.headerFont
            }
        }
    }
}
