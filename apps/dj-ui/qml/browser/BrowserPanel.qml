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

import "../controls"

import "../mixer"

Pane {
    id: browser
    objectName: "browserPanel"

    // Which deck a double click loads into; the header shows it, nothing is assumed.
    property string target: "A"
    // What the last activation did: read by --browser-check, so the click path can be measured.
    property string lastAction: ""
    // A load in flight: what was asked for, and how long the deck has had to take it.
    property string pendingDeck: ""
    property string pendingPath: ""
    property int pendingTicks: 0
    // Which section the list is showing. The browser object holds the mode; this is its name, so the
    // rail can light the row the list belongs to instead of guessing.
    readonly property string section: Browser.mode === "collection" ? "collection" : "files"

    // The rows the DJ picked: a list of paths, so a folder and a file can travel to the collection
    // together. Ctrl toggles one, Shift takes the run between the anchor and the click.
    property var selection: []
    /// The chosen tracks whose grid is locked. A locked grid is the DJ's decision, so the analysis stands
    /// down for these: the collection cannot rewrite what the lock protects.
    readonly property var lockedSelection: {
        const locked = [];
        for (const path of browser.selection) {
            for (const entry of Browser.entries) {
                if (entry.path === path && entry.analysis_lock === true) {
                    locked.push(path);
                    break;
                }
            }
        }
        return locked;
    }
    property int anchorIndex: -1

    // What the engine says about the import, if one was started since it came up.
    readonly property var importState: Bridge.snapshot !== undefined && Bridge.snapshot.import !== undefined
                                       ? Bridge.snapshot.import : null
    readonly property var importQuestion: browser.importState !== null
                                          && browser.importState.question !== undefined
                                          ? browser.importState.question : null
    // What the engine says about the analysis started from this panel.
    readonly property var analysisState: Bridge.snapshot !== undefined && Bridge.snapshot.analysis !== undefined
                                         ? Bridge.snapshot.analysis : null

    // The numbers land in the files, not in a snapshot: the list is read again once, when the batch stops.
    // Once, not on every snapshot: the snapshot also carries the meters and the playheads, and a refresh
    // on each of those reopens the collection under the answer that is already on its way — the list then
    // never fills, because every answer is an answer to a question that has just been replaced.
    property bool analysisWasRunning: false
    onAnalysisStateChanged: {
        const running = browser.analysisState !== null && browser.analysisState.running === true;
        if (analysisWasRunning && !running) {
            Browser.refreshCollection();
        }
        analysisWasRunning = running;
    }

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

    /// Measure the chosen tracks: one batch, with a window that says how far it has come. A single
    /// track asks nothing — there is nothing to decide — and goes straight to the progress.
    function analyzeSelection() {
        // The locked tracks are left out rather than measured: the engine refuses them anyway, and a
        // batch that quietly rewrote a locked grid would be the one thing the lock exists to prevent.
        const chosen = [];
        for (const path of browser.selection) {
            if (browser.lockedSelection.indexOf(path) < 0) {
                chosen.push(path);
            }
        }
        const skipped = browser.selection.length - chosen.length;
        if (chosen.length === 0) {
            browser.lastAction = skipped > 0 ? "all chosen tracks are locked" : "nothing to analyse";
            return false;
        }
        if (skipped > 0) {
            browser.lastAction = "analysing %1, %2 locked".arg(chosen.length).arg(skipped);
        }
        if (chosen.length === 1) {
            Bridge.analyzePaths(chosen, true);
            if (skipped === 0) {
                browser.lastAction = "analysing 1";
            }
            return true;
        }
        analyzeDialog.paths = chosen;
        analyzeDialog.open();
        return true;
    }

    function startImport(paths, parallel) {
        Bridge.importPaths(paths, LayoutState.roomiestRoot(), parallel);
        browser.lastAction = "import " + paths.length + " into " + LayoutState.roomiestRoot();
    }

    // The collection's columns: order and widths are a setting, because what matters in a table is
    // the DJ's business. Widths are shares of the list, so a wider window does not lose the shape.
    readonly property var columns: LayoutState.collectionColumns
    function columnId(index) {
        return index < columns.length ? columns[index].id : "";
    }
    function columnShare(index) {
        return index < columns.length ? columns[index].width : 0;
    }
    // The row number sits in its own gutter to the left of the columns. It is a position, not data: it cannot
    // be dragged away, sorted or hidden, and every column shares what is left of the width after it.
    readonly property real numberWidth: Math.round(browser.listFont * 2.4)
    // The lock column sits beside the number: a locked grid is a fact about the track, and the DJ reads it
    // where he reads the position. Empty for a track nobody locked, so the list stays quiet.
    readonly property real lockWidth: Math.round(browser.listFont * 1.6)
    function columnWidth(index, total) {
        return columnShare(index) * (total - numberWidth - lockWidth);
    }
    function columnX(index, total) {
        let x = numberWidth + lockWidth;
        for (let i = 0; i < index && i < columns.length; ++i) {
            x += columns[i].width * (total - numberWidth - lockWidth);
        }
        return x;
    }
    // Which column the pointer is over: the drop target of a dragged header.
    /// Move the cursor by one row and remember it: the keyboard is a hand too.
    ///
    /// The row is counted from the remembered cursor, not from the view's index: the index follows the
    /// cursor through a binding, and reading it in the same call gives the row the list had before the
    /// key — the view then moves a step behind the hand.
    function stepCursor(delta) {
        const count = Browser.entries.length;
        if (count === 0) {
            return;
        }
        const row = Browser.rowOf(Browser.cursor);
        const from = row < 0 ? 0 : row;
        const next = Math.max(0, Math.min(count - 1, from + delta));
        Browser.setCursor(Browser.entries[next].path);
        // The row the DJ is on sits in the middle of the window, not against its edge: a list that
        // parks the cursor under the last visible row gives the eye nothing below it to read next.
        list.positionViewAtIndex(next, ListView.Center);
    }

    /// Put the cursor row in the middle of the window.
    ///
    /// A key is not the only way a cursor arrives: a remembered row comes back when the section is
    /// opened, a click lands anywhere in the list, the list itself is rebuilt under the cursor. Qt
    /// scrolls to a new current row on its own, but it scrolls the row to the nearest edge — the very
    /// placement the DJ reported as wrong — so the middle has to be asked for after that scroll.
    function centreOnCursor(attempt) {
        const row = Browser.rowOf(Browser.cursor);
        if (row < 0) {
            return;
        }
        list.positionViewAtIndex(row, ListView.Center);
        // The list is laid out lazily and the window is sized after it: when the cursor comes back while
        // the rows are still arriving, one `positionViewAtIndex` is computed against a half-built list
        // and lands short — the row ends up below the window. Ask again on the next turns until the
        // offset the row needs is the offset the view has.
        const wanted = Math.max(0, Math.min(row * rowHeight + rowHeight / 2 - list.height / 2,
                                            list.contentHeight - list.height));
        const tries = attempt === undefined ? 0 : attempt;
        if (Math.abs(list.contentY - wanted) <= 0.5 || tries >= 12) {
            return;
        }
        Qt.callLater(function () {
            centreOnCursor(tries + 1);
        });
    }

    /// Centre the cursor row only if the window has lost it.
    ///
    /// A key is centred by `stepCursor` itself; this is for the cursor that arrives on its own — the
    /// remembered row of a section being opened, a row rebuilt under the cursor. A click lands on a row
    /// that is already visible, and moving the list under the mouse would put a different row there for
    /// the second click of a double click.
    function centreIfLost() {
        const row = Browser.rowOf(Browser.cursor);
        if (row < 0 || row >= list.count) {
            return;
        }
        const top = row * rowHeight;
        if (top < list.contentY || top + rowHeight > list.contentY + list.height) {
            centreOnCursor();
        }
    }

    // A click selects a row that is already under the pointer; it must not move the list, or the second
    // click of a double click lands on a different row. Everything else that moves the cursor — a key, a
    // remembered row coming back with its section, a list rebuilt under it — centres the row.
    property bool keepScrollOnCursor: false

    /// Does this snapshot value, anywhere inside it, name the file?
    function snapshotHas(value, name) {
        if (value === null || value === undefined) {
            return false;
        }
        if (Array.isArray(value)) {
            for (let index = 0; index < value.length; ++index) {
                if (browser.snapshotHas(value[index], name)) {
                    return true;
                }
            }
            return false;
        }
        if (typeof value === "object") {
            for (const key in value) {
                if (browser.snapshotHas(value[key], name)) {
                    return true;
                }
            }
            return false;
        }
        return String(value).indexOf(name) >= 0;
    }

    // 200 ms is quick enough to look instant and cheap enough to cost nothing; 300 ticks are a minute,
    // which is what a long lossless file needs before it can be called loaded.
    Timer {
        id: loadWatch
        objectName: "loadWatch"
        interval: 200
        repeat: true
        running: browser.pendingDeck !== ""
        onTriggered: {
            const name = browser.pendingPath.split("/").pop();
            if (browser.snapshotHas(Bridge.snapshot, name)) {
                browser.lastAction = "loaded " + browser.pendingPath + " into " + browser.pendingDeck;
                browser.pendingDeck = "";
                return;
            }
            browser.pendingTicks += 1;
            if (browser.pendingTicks >= 300) {
                browser.lastAction = "deck " + browser.pendingDeck + " did not take " + name;
                browser.pendingDeck = "";
            }
        }
    }

    Connections {
        target: Browser
        function onCursorChanged() {
            if (!browser.keepScrollOnCursor) {
                centreOnCursor();
            }
        }
        function onChanged() {
            // The rows can arrive after the cursor, and Qt then scrolls the row to the nearest edge —
            // which makes it «visible» and would keep it out of the middle. With the wheel moving the
            // cursor, the middle is where the DJ put it, so the request is unconditional here too.
            centreOnCursor();
        }
    }

    /// Where a named column sits now: a row cell belongs to its column, not to a position, so a
    /// moved column takes its numbers with it.
    /// The mark a sorted column carries: filled for the key that decides, hollow for the one after it.
    function sortMark(id) {
        const keys = Browser.collectionSort;
        for (let index = 0; index < keys.length; ++index) {
            if (keys[index].column === id) {
                if (index === 0) {
                    return keys[index].ascending ? " ▲" : " ▼";
                }
                return keys[index].ascending ? " △" : " ▽";
            }
        }
        return "";
    }

    function columnIndexOf(id) {
        for (let i = 0; i < columns.length; ++i) {
            if (columns[i].id === id) {
                return i;
            }
        }
        return -1;
    }
    function columnIndexAt(x, total) {
        let edge = 0;
        for (let i = 0; i < columns.length; ++i) {
            edge += columns[i].width * total;
            if (x < edge) {
                return i;
            }
        }
        return Math.max(0, columns.length - 1);
    }
    function columnTitle(id) {
        switch (id) {
        case "artist": return qsTr("Исполнитель");
        case "album": return qsTr("Альбом");
        case "title": return qsTr("Название");
        case "time": return qsTr("Время");
        case "bpm": return "BPM";
        case "key": return "Key";
        }
        return "";
    }
    function columnAlign(id) {
        return (id === "time" || id === "bpm" || id === "key") ? Text.AlignRight : Text.AlignLeft;
    }
    function columnText(entry, id) {
        switch (id) {
        case "artist": return entry.artist !== undefined ? entry.artist : "";
        case "album": return entry.album !== undefined ? entry.album : "";
        case "title": return entry.title !== undefined ? entry.title : "";
        case "time": return browser.clock(entry.duration_ms);
        case "bpm": return entry.bpm !== undefined ? Math.round(entry.bpm) : "";
        case "key": return entry.key !== undefined ? entry.key : "";
        }
        return "";
    }

    /// A length in milliseconds as a DJ reads it: `3:21`. Empty when nothing measured it.
    function clock(ms) {
        if (!(ms > 0)) {
            return "";
        }
        const seconds = Math.round(ms / 1000);
        return Math.floor(seconds / 60) + ":" + ("0" + (seconds % 60)).slice(-2);
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

    // The genre chips the DJ left on: read from the layout file at start and written back when they
    // change, like the font and the rail width (DJP-UI-001).
    property bool genresSeeded: false
    Component.onCompleted: {
        Browser.genreFilter = LayoutState.collectionGenres;
        genresSeeded = true;
    }
    Connections {
        target: Browser
        // Only a pick is a decision of the DJ: a reload also moves the chips, and writing the layout
        // from it would re-open the collection the layout change asks for.
        function onGenresPicked() {
            if (browser.genresSeeded) {
                LayoutState.collectionGenres = Browser.genreFilter;
            }
        }
    }

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
        // No engine, no deck: saying "loaded" here would be the same lie the panel used to tell.
        if (!Bridge.connected) {
            browser.lastAction = "no engine for " + deck;
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
        // A load is a request, not an outcome: the deck is the only witness. Until the engine shows the
        // track the panel says "loading", and if it never shows it the panel says that instead — a
        // panel that claims a load the deck never took hides a deck that is stuck.
        browser.pendingDeck = deck;
        browser.pendingPath = path;
        browser.pendingTicks = 0;
        browser.lastAction = "loading " + path + " into " + deck;
        return true;
    }

    // The context menu: what a DJ does with the rows he picked. One entry today; the place is here
    // so the next one (analyse, reveal, remove) has somewhere to go.
    Menu {
        id: fileMenu
        objectName: "browserMenu"
        MenuEntry {
            objectName: "addToCollection"
            text: qsTr("Add files to collection…")
            // A file that is already in the collection cannot be added to it: the entry belongs to the
            // file list, and a menu that offers the impossible is a menu that lies.
            visible: browser.section !== "collection"
            enabled: browser.selection.length > 0
            onClicked: browser.askImport()
        }
        MenuEntry {
            objectName: "rescanCollection"
            text: qsTr("Rescan collection…")
            // Tags and metadata read from the files again, and the genre counts recounted. Files that
            // already have a record keep their measured tempo and key; only the missing ones are measured.
            visible: browser.section === "collection"
            onClicked: Browser.rescanCollection()
        }
        MenuEntry {
            objectName: "analyzeSelection"
            text: qsTr("Analyze…")
            // Tempo and key, measured again: what an import could not measure, or what the DJ wants
            // re-measured after a remaster.
            visible: browser.section === "collection"
            // Nothing chosen is nothing to ask for, and every chosen track locked is a request the lock
            // has already answered: the entry stands down instead of promising a rewrite.
            enabled: browser.selection.length > 0
                     && browser.lockedSelection.length < browser.selection.length
            onClicked: browser.analyzeSelection()
        }
    }

    // A list that comes back — another folder, the collection, the same folder after a restart —
    // puts the highlight where the DJ left it.
    // The analysis writes the file, not a snapshot: the numbers appear when the worker is done, so
    // the list is read again a few times instead of being waited on once.
    // The rescan writes records and rebuilds the index; the list is read again while it runs, so the
    // rows and the counts follow the files instead of appearing only after a restart.
    Timer {
        id: rescanPoll
        objectName: "rescanPoll"
        interval: 900
        repeat: true
        running: browser.section === "collection" && browser.importState !== null
                 && browser.importState.running === true
        onTriggered: Browser.refreshCollection()
    }

    Timer {
        id: analyzePoll
        objectName: "analyzePoll"
        interval: 900
        repeat: true
        running: browser.analysisState !== null && browser.analysisState.running === true
        onTriggered: Browser.refreshCollection()
    }

    // The collection's batch: how many tracks, and whether to measure them at once. A single track
    // skips the question — there is nothing to decide — and goes straight to the progress window.
    Dialog {
        id: analyzeDialog
        objectName: "analyzeDialog"
        title: qsTr("Analyze")
        modal: true
        anchors.centerIn: Overlay.overlay
        closePolicy: Popup.NoAutoClose
        standardButtons: Dialog.NoButton
        property var paths: []
        // The checkbox lives in the dialog's content, and its id is not visible from the footer:
        // the choice is kept here, where both sides can read it.
        property bool parallel: true
        ColumnLayout {
            spacing: 8
            Label {
                objectName: "analyzeDialogText"
                text: qsTr("%1 track(s) will be measured: tempo and key, written beside the files.")
                      .arg(analyzeDialog.paths.length)
                color: UiSettings.themeText
                font.pixelSize: browser.listFont
            }
            CheckBox {
                objectName: "analyzeParallelBox"
                text: qsTr("Выполнять параллельно")
                checked: analyzeDialog.parallel
                onToggled: analyzeDialog.parallel = checked
                font.pixelSize: browser.listFont
            }
        }
        footer: RowLayout {
            Item { Layout.fillWidth: true }
            Button {
                objectName: "analyzeStart"
                text: qsTr("Анализировать")
                font.pixelSize: browser.listFont
                onClicked: {
                    analyzeDialog.close();
                    Bridge.analyzePaths(analyzeDialog.paths, analyzeDialog.parallel);
                }
            }
            Button {
                objectName: "analyzeDialogCancel"
                text: qsTr("Отменить")
                font.pixelSize: browser.listFont
                onClicked: analyzeDialog.close()
            }
        }
    }

    // What the batch is doing: a decode per file takes seconds, and a list that changes with no
    // explanation is a list the DJ does not trust.
    Dialog {
        id: analyzeProgress
        objectName: "analyzeProgress"
        title: qsTr("Analyzing")
        modal: true
        anchors.centerIn: Overlay.overlay
        closePolicy: Popup.NoAutoClose
        standardButtons: Dialog.NoButton
        visible: browser.analysisState !== null && browser.analysisState.running === true
        ColumnLayout {
            spacing: 8
            Label {
                objectName: "analyzeProgressText"
                text: qsTr("%1 of %2 measured")
                      .arg(browser.analysisState !== null ? browser.analysisState.done : 0)
                      .arg(browser.analysisState !== null ? browser.analysisState.total : 0)
                color: UiSettings.themeText
                font.pixelSize: browser.listFont
            }
            Label {
                objectName: "analyzeProgressCurrent"
                text: browser.analysisState !== null && browser.analysisState.current !== undefined
                      ? browser.analysisState.current : ""
                elide: Text.ElideMiddle
                Layout.preferredWidth: 320
                color: UiSettings.themeTextDim
                font.pixelSize: browser.listFont
            }
            ProgressBar {
                objectName: "analyzeProgressBar"
                from: 0
                to: browser.analysisState !== null ? Math.max(1, browser.analysisState.total) : 1
                value: browser.analysisState !== null ? browser.analysisState.done : 0
                Layout.fillWidth: true
            }
        }
        footer: RowLayout {
            Item { Layout.fillWidth: true }
            Button {
                objectName: "analyzeStop"
                text: qsTr("Отменить")
                font.pixelSize: browser.listFont
                onClicked: Bridge.cancelAnalysis()
            }
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

    // The genre filter: the whole list with how many tracks carry each genre, and the choice applied
    // only when the DJ says so. A box that narrows the list on the spot is a list that changes under
    // the hand that is still choosing.
    Dialog {
        id: genreFilterDialog
        objectName: "genreFilterDialog"
        title: qsTr("Genre filter")
        modal: true
        anchors.centerIn: Overlay.overlay
        standardButtons: Dialog.NoButton
        closePolicy: Popup.NoAutoClose
        // What Filter will apply. The panel's own filter is untouched until then, so Cancel is free.
        property var pending: []

        onAboutToShow: pending = Browser.genreFilter.slice()

        contentItem: ColumnLayout {
            spacing: 6
            Label {
                objectName: "genreFilterHint"
                text: qsTr("%1 genre(s), %2 chosen")
                      .arg(Browser.genres.length).arg(genreFilterDialog.pending.length)
                color: UiSettings.themeTextDim
                font.pixelSize: browser.listFont
            }
            ListView {
                id: genreFilterList
                objectName: "genreFilterList"
                Layout.preferredWidth: 340
                // Tall enough to read, short enough to leave the footer on the screen. Past that the
                // list scrolls: a collection with two hundred genres must not be a dialog off the
                // bottom of the window.
                Layout.preferredHeight: Math.min(320, Math.max(60, contentHeight))
                clip: true
                interactive: true
                model: Browser.genres
                boundsBehavior: Flickable.StopAtBounds
                ScrollBar.vertical: ScrollBar {
                    objectName: "genreFilterBar"
                    policy: ScrollBar.AsNeeded
                }

                delegate: Item {
                    id: genreRow
                    required property var modelData
                    objectName: "genreRow_" + genreRow.modelData.name
                    // What the row says, read by the check: the census is the point of the dialog.
                    property string label: genreRow.modelData.name + "  " + genreRow.modelData.count
                    width: genreFilterList.width
                    height: Math.round(browser.rowHeight * 0.8)

                    CheckBox {
                        objectName: "genreBox_" + genreRow.modelData.name
                        anchors.left: parent.left
                        anchors.verticalCenter: parent.verticalCenter
                        text: genreRow.label
                        font.pixelSize: browser.listFont
                        // Bound, never toggled by the control itself: the choice lives in `pending`
                        // until Filter is pressed, and a box that keeps its own value would disagree
                        // with the list after the dialog is reopened.
                        checked: genreFilterDialog.pending.indexOf(genreRow.modelData.name) >= 0
                    }

                    MouseArea {
                        anchors.fill: parent
                        onClicked: {
                            const wanted = genreFilterDialog.pending.slice();
                            const at = wanted.indexOf(genreRow.modelData.name);
                            if (at >= 0) {
                                wanted.splice(at, 1);
                            } else {
                                wanted.push(genreRow.modelData.name);
                            }
                            genreFilterDialog.pending = wanted;
                        }
                    }
                }
            }
        }

        footer: RowLayout {
            spacing: 6
            Item { Layout.fillWidth: true }
            Button {
                objectName: "genreFilterCancel"
                text: qsTr("Cancel")
                font.pixelSize: browser.listFont
                onClicked: genreFilterDialog.close()
            }
            Button {
                objectName: "genreFilterApply"
                text: qsTr("Filter")
                font.pixelSize: browser.listFont
                onClicked: {
                    Browser.genreFilter = genreFilterDialog.pending.slice();
                    genreFilterDialog.close();
                }
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
            // Places, not labels: a DJ starts in the collection and digs in the file list, so the
            // collection sits at the top of the rail and the files at its bottom. Playlists goes with
            // the collection — it is a place the same way, and an empty one is still a place.
            Component {
                id: sectionRow
                Rectangle {
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
            ListModel {
                id: sectionTop
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
                model: sectionTop
                delegate: sectionRow
            }
            Item {
                Layout.fillHeight: true
            }
            ListModel {
                id: sectionBottom
                ListElement {
                    key: "files"
                    title: "Files"
                    ready: true
                }
            }
            Repeater {
                model: sectionBottom
                delegate: sectionRow
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
                // The rows are a window over the index, not a walk over the disks: the header says
                // which one the DJ is looking at, so a batch that arrives later is not a surprise.
                Label {
                    objectName: "collectionSource"
                    visible: browser.section === "collection" && Browser.paged
                    text: qsTr("for collection")
                    color: UiSettings.themeTextDim
                    font.pixelSize: browser.headerFont
                }
                // The collection may live on several roots: a path here would be one of them and a lie
                // about the others. What the DJ needs is how many tracks it holds.
                Label {
                    objectName: "collectionTotal"
                    visible: browser.section === "collection"
                    text: qsTr("(%1)").arg(Browser.total)
                    color: UiSettings.themeTextDim
                    font.pixelSize: browser.headerFont
                }
                Label {
                    objectName: "browserPath"
                    Layout.fillWidth: true
                    visible: browser.section !== "collection"
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

            // The filter button and the search share one row. The rows of the header are the space the
            // list does not get, and a row holding one button is the most expensive row there is.
            RowLayout {
                objectName: "collectionFilterRow"
                visible: browser.section === "collection"
                Layout.fillWidth: true
                Layout.preferredHeight: browser.headerButtonHeight
                spacing: 6

                // The genre filter is a question, not a row of chips: the collection may carry fifty
                // genres, and fifty boxes in the header push the list off the screen. One button says
                // whether a filter is on; the choosing happens in a dialog that scrolls.
                Button {
                    objectName: "genreFilter"
                    text: Browser.genreFilter.length > 0
                          ? qsTr("Genre filter… (%1)").arg(Browser.genreFilter.length)
                          : qsTr("Genre filter…")
                    font.pixelSize: browser.headerButtonFont
                    implicitHeight: browser.headerButtonHeight
                    // A button that opens an empty dialog teaches nothing: it waits for the census.
                    enabled: Browser.genres.length > 0
                    onClicked: genreFilterDialog.open()
                }

                // The search lives in the section that needs it: a field that is always there is a field
                // that means nothing in a folder.
                TextField {
                    objectName: "collectionSearch"
                    Layout.fillWidth: true
                    Layout.preferredHeight: browser.headerButtonHeight
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
            }

            // The collection's columns, named: a row of numbers with no header is a puzzle. The file
            // name is not among them — a DJ looks for the artist, the album and the title.
            Item {
                id: header
                objectName: "collectionHeader"
                visible: browser.section === "collection"
                Layout.fillWidth: true
                Layout.preferredHeight: Math.round(browser.rowHeight * 0.85)

                // Where a column ends, in pixels: the line the pointer catches to change a width. One
                // definition, because a check that measures the edge from the cells and a header that
                // measures it from the shares disagree by the inset, and a press beside the line is a click.
                function boundaryX(index) {
                    return browser.columnX(index, header.width)
                        + browser.columnWidth(index, header.width);
                }

                function boundaryAt(x) {
                    for (let index = 0; index < browser.columns.length; ++index) {
                        const edge = boundaryX(index);
                        if (Math.abs(x - edge) <= 5) {
                            return index;
                        }
                    }
                    return -1;
                }

                Label {
                    objectName: "collectionHeaderNumber"
                    x: 0
                    width: browser.numberWidth - 6
                    text: "№"
                    horizontalAlignment: Text.AlignRight
                    color: UiSettings.themeTextDim
                    font.pixelSize: browser.listFont
                    elide: Text.ElideRight
                }

                Label {
                    objectName: "collectionHeaderLock"
                    x: browser.numberWidth
                    width: browser.lockWidth
                    text: "\uD83D\uDD12"
                    horizontalAlignment: Text.AlignHCenter
                    color: UiSettings.themeTextDim
                    font.pixelSize: Math.max(9, browser.listFont - 2)
                }

                Label {
                    objectName: "collectionHeaderCell_0"
                    x: browser.columnX(0, header.width) + 6
                    width: Math.max(0, browser.columnWidth(0, header.width) - 12)
                    visible: browser.columnId(0) !== ""
                    text: browser.columnTitle(browser.columnId(0))
                          + browser.sortMark(browser.columnId(0))
                    horizontalAlignment: browser.columnAlign(browser.columnId(0))
                    color: browser.sortMark(browser.columnId(0)) === ""
                           ? UiSettings.themeTextDim : UiSettings.themeAccent
                    font.pixelSize: browser.listFont
                    elide: Text.ElideRight
                }
                Label {
                    objectName: "collectionHeaderCell_1"
                    x: browser.columnX(1, header.width) + 6
                    width: Math.max(0, browser.columnWidth(1, header.width) - 12)
                    visible: browser.columnId(1) !== ""
                    text: browser.columnTitle(browser.columnId(1))
                          + browser.sortMark(browser.columnId(1))
                    horizontalAlignment: browser.columnAlign(browser.columnId(1))
                    color: browser.sortMark(browser.columnId(1)) === ""
                           ? UiSettings.themeTextDim : UiSettings.themeAccent
                    font.pixelSize: browser.listFont
                    elide: Text.ElideRight
                }
                Label {
                    objectName: "collectionHeaderCell_2"
                    x: browser.columnX(2, header.width) + 6
                    width: Math.max(0, browser.columnWidth(2, header.width) - 12)
                    visible: browser.columnId(2) !== ""
                    text: browser.columnTitle(browser.columnId(2))
                          + browser.sortMark(browser.columnId(2))
                    horizontalAlignment: browser.columnAlign(browser.columnId(2))
                    color: browser.sortMark(browser.columnId(2)) === ""
                           ? UiSettings.themeTextDim : UiSettings.themeAccent
                    font.pixelSize: browser.listFont
                    elide: Text.ElideRight
                }
                Label {
                    objectName: "collectionHeaderCell_3"
                    x: browser.columnX(3, header.width) + 6
                    width: Math.max(0, browser.columnWidth(3, header.width) - 12)
                    visible: browser.columnId(3) !== ""
                    text: browser.columnTitle(browser.columnId(3))
                          + browser.sortMark(browser.columnId(3))
                    horizontalAlignment: browser.columnAlign(browser.columnId(3))
                    color: browser.sortMark(browser.columnId(3)) === ""
                           ? UiSettings.themeTextDim : UiSettings.themeAccent
                    font.pixelSize: browser.listFont
                    elide: Text.ElideRight
                }
                Label {
                    objectName: "collectionHeaderCell_4"
                    x: browser.columnX(4, header.width) + 6
                    width: Math.max(0, browser.columnWidth(4, header.width) - 12)
                    visible: browser.columnId(4) !== ""
                    text: browser.columnTitle(browser.columnId(4))
                          + browser.sortMark(browser.columnId(4))
                    horizontalAlignment: browser.columnAlign(browser.columnId(4))
                    color: browser.sortMark(browser.columnId(4)) === ""
                           ? UiSettings.themeTextDim : UiSettings.themeAccent
                    font.pixelSize: browser.listFont
                    elide: Text.ElideRight
                }
                Label {
                    objectName: "collectionHeaderCell_5"
                    x: browser.columnX(5, header.width) + 6
                    width: Math.max(0, browser.columnWidth(5, header.width) - 12)
                    visible: browser.columnId(5) !== ""
                    text: browser.columnTitle(browser.columnId(5))
                          + browser.sortMark(browser.columnId(5))
                    horizontalAlignment: browser.columnAlign(browser.columnId(5))
                    color: browser.sortMark(browser.columnId(5)) === ""
                           ? UiSettings.themeTextDim : UiSettings.themeAccent
                    font.pixelSize: browser.listFont
                    elide: Text.ElideRight
                }

                // The edges are drawn: a column that can be resized has to show where its edge is,
                // otherwise the DJ aims at a fifteen-pixel strip in the dark.
                Rectangle {
                    x: browser.columnX(1, header.width) - 1
                    width: 2
                    height: parent.height
                    visible: browser.columns.length > 1
                    color: UiSettings.themePanel
                }
                Rectangle {
                    x: browser.columnX(2, header.width) - 1
                    width: 2
                    height: parent.height
                    visible: browser.columns.length > 2
                    color: UiSettings.themePanel
                }
                Rectangle {
                    x: browser.columnX(3, header.width) - 1
                    width: 2
                    height: parent.height
                    visible: browser.columns.length > 3
                    color: UiSettings.themePanel
                }
                Rectangle {
                    x: browser.columnX(4, header.width) - 1
                    width: 2
                    height: parent.height
                    visible: browser.columns.length > 4
                    color: UiSettings.themePanel
                }
                Rectangle {
                    x: browser.columnX(5, header.width) - 1
                    width: 2
                    height: parent.height
                    visible: browser.columns.length > 5
                    color: UiSettings.themePanel
                }

                // One pointer does both jobs: on a column's edge it changes the width, anywhere else
                // it moves the column. Two targets on a fifteen-pixel strip would fight each other.
                MouseArea {
                    id: headerDrag
                    objectName: "collectionHeaderDrag"
                    anchors.fill: parent
                    property int resizeIndex: -1
                    property real startX: 0
                    property real startShare: 0
                    property int dragIndex: -1
                    property int dropIndex: -1
                    // Where the press started: a release in the same place is a click, and a click on a
                    // column's name sorts by it — the strip already owns the width and the move.
                    property real pressX: -1
                    property int pressIndex: -1
                    // The cursor says what the strip does: a splitter under the pointer is the only
                    // way a DJ learns that the edge is draggable at all.
                    property int hoverBoundary: -1
                    hoverEnabled: true
                    cursorShape: (hoverBoundary >= 0 || resizeIndex >= 0)
                                 ? Qt.SizeHorCursor
                                 : (dragIndex >= 0 ? Qt.ClosedHandCursor : Qt.ArrowCursor)
                    onPressed: function (mouse) {
                        const edge = header.boundaryAt(mouse.x);
                        if (edge >= 0) {
                            resizeIndex = edge;
                            startX = mouse.x;
                            startShare = browser.columnShare(edge);
                            dragIndex = -1;
                        } else {
                            resizeIndex = -1;
                            dragIndex = browser.columnIndexAt(mouse.x, header.width);
                            dropIndex = dragIndex;
                            pressX = mouse.x;
                            pressIndex = dragIndex;
                        }
                    }
                    onPositionChanged: function (mouse) {
                        hoverBoundary = header.boundaryAt(mouse.x);
                        if (resizeIndex >= 0) {
                            LayoutState.setCollectionColumnWidth(
                                browser.columnId(resizeIndex),
                                startShare + (mouse.x - startX) / Math.max(1, header.width));
                        } else if (dragIndex >= 0) {
                            dropIndex = browser.columnIndexAt(mouse.x, header.width);
                        }
                    }
                    onReleased: function (mouse) {
                        if (resizeIndex < 0 && pressIndex >= 0 && dropIndex === pressIndex
                            && Math.abs(mouse.x - pressX) <= 4) {
                            // A click on the name: sort by this column, or by it second with Shift.
                            Browser.sortCollection(browser.columnId(pressIndex),
                                                   (mouse.modifiers & Qt.ShiftModifier) !== 0);
                        } else if (dragIndex >= 0 && dropIndex >= 0 && dropIndex !== dragIndex) {
                            LayoutState.moveCollectionColumn(browser.columnId(dragIndex), dropIndex);
                        }
                        resizeIndex = -1;
                        dragIndex = -1;
                        dropIndex = -1;
                        pressIndex = -1;
                        pressX = -1;
                    }
                }

                // Where the column will land: a line at the edge it would take.
                Rectangle {
                    objectName: "collectionDropMark"
                    visible: headerDrag.dragIndex >= 0 && headerDrag.dropIndex >= 0
                    x: browser.columnX(headerDrag.dropIndex, header.width)
                    width: 2
                    height: parent.height
                    color: UiSettings.themeAccent
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
                // The list stays one continuous thing: when its end comes into view, the next batch of
                // the index is asked for and appended — no pages, no seams.
                onAtYEndChanged: if (atYEnd) Browser.loadMore()
                keyNavigationWraps: false
                focus: true
                ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }
                // The highlight is where the browser remembers the cursor stood: a list that comes
                // back puts it there, and a row that is gone leaves no highlight at all. Written as a
                // binding on purpose — setting the index by hand would break it and lose the place.
                currentIndex: Browser.entries.length >= 0 ? Browser.rowOf(Browser.cursor) : -1
                // The cursor is placed by hand, and the view goes there at once: a list that glides to
                // the row it already knows reads as lag, not as motion.
                highlightMoveDuration: 0
                highlightResizeDuration: 0
                // The window is laid out after the list: a height that arrives late must not leave the
                // cursor outside the new window.
                onHeightChanged: browser.centreIfLost()
                // Arrow keys move the cursor and say so. Deliberately not `onCurrentIndexChanged`:
                // a view handed a new model resets its index to the first row, and that reset would
                // be remembered as the DJ's choice, wiping the place he actually left.
                Keys.onUpPressed: browser.stepCursor(-1)
                Keys.onDownPressed: browser.stepCursor(1)

                // The wheel moves the cursor, not only the picture. The row the DJ is on sits in the
                // middle of the window, so a scroll that moved the picture alone would slide the
                // selection out of the window — which is exactly what the DJ saw.
                WheelHandler {
                    onWheel: function (event) {
                        const rows = Math.round(-event.angleDelta.y / 120);
                        if (rows !== 0) {
                            browser.stepCursor(rows);
                        }
                    }
                }
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

                    // A collection row is about the track, not about the file: artist, album, title,
                    // how long it plays, its tempo and its key. The file name says none of that.
                    Item {
                        objectName: "collectionRow_" + row.index
                        anchors.fill: parent
                        visible: browser.section === "collection" && !row.isFolder

                        Label {
                            objectName: "collectionCell_number_" + row.index
                            x: 0
                            width: browser.numberWidth - 6
                            anchors.verticalCenter: parent.verticalCenter
                            text: (row.index + 1)
                            horizontalAlignment: Text.AlignRight
                            font.pixelSize: browser.listFont
                            color: UiSettings.themeTextDim
                            elide: Text.ElideRight
                        }
                        Label {
                            objectName: "collectionCell_lock_" + row.index
                            x: browser.numberWidth
                            width: browser.lockWidth
                            anchors.verticalCenter: parent.verticalCenter
                            text: row.modelData.analysis_lock ? "\uD83D\uDD12" : ""
                            horizontalAlignment: Text.AlignHCenter
                            font.pixelSize: browser.listFont
                            color: UiSettings.themeTextDim
                        }
                        Label {
                            objectName: "collectionCell_artist_" + row.index
                            readonly property int column: browser.columnIndexOf("artist")
                            x: column >= 0 ? browser.columnX(column, row.width) + 6 : 0
                            width: column >= 0 ? Math.max(0, browser.columnWidth(column, row.width) - 12) : 0
                            visible: column >= 0
                            anchors.verticalCenter: parent.verticalCenter
                            text: browser.columnText(row.modelData, "artist")
                            elide: Text.ElideRight
                            font.pixelSize: browser.listFont
                            color: row.current ? UiSettings.themePlayhead : UiSettings.themeText
                        }
                        Label {
                            objectName: "collectionCell_album_" + row.index
                            readonly property int column: browser.columnIndexOf("album")
                            x: column >= 0 ? browser.columnX(column, row.width) + 6 : 0
                            width: column >= 0 ? Math.max(0, browser.columnWidth(column, row.width) - 12) : 0
                            visible: column >= 0
                            anchors.verticalCenter: parent.verticalCenter
                            text: browser.columnText(row.modelData, "album")
                            elide: Text.ElideRight
                            font.pixelSize: browser.listFont
                            color: row.current ? UiSettings.themePlayhead : UiSettings.themeTextDim
                        }
                        Label {
                            objectName: "collectionCell_title_" + row.index
                            readonly property int column: browser.columnIndexOf("title")
                            x: column >= 0 ? browser.columnX(column, row.width) + 6 : 0
                            width: column >= 0 ? Math.max(0, browser.columnWidth(column, row.width) - 12) : 0
                            visible: column >= 0
                            anchors.verticalCenter: parent.verticalCenter
                            text: browser.columnText(row.modelData, "title")
                            elide: Text.ElideRight
                            font.pixelSize: browser.listFont
                            color: row.current ? UiSettings.themePlayhead : UiSettings.themeText
                        }
                        Label {
                            objectName: "collectionCell_time_" + row.index
                            readonly property int column: browser.columnIndexOf("time")
                            x: column >= 0 ? browser.columnX(column, row.width) + 6 : 0
                            width: column >= 0 ? Math.max(0, browser.columnWidth(column, row.width) - 12) : 0
                            visible: column >= 0
                            anchors.verticalCenter: parent.verticalCenter
                            text: browser.columnText(row.modelData, "time")
                            horizontalAlignment: Text.AlignRight
                            font.pixelSize: browser.listFont
                            color: row.current ? UiSettings.themePlayhead : UiSettings.themeTextDim
                        }
                        Label {
                            objectName: "collectionCell_bpm_" + row.index
                            readonly property int column: browser.columnIndexOf("bpm")
                            x: column >= 0 ? browser.columnX(column, row.width) + 6 : 0
                            width: column >= 0 ? Math.max(0, browser.columnWidth(column, row.width) - 12) : 0
                            visible: column >= 0
                            anchors.verticalCenter: parent.verticalCenter
                            text: browser.columnText(row.modelData, "bpm")
                            horizontalAlignment: Text.AlignRight
                            font.pixelSize: browser.listFont
                            color: row.current ? UiSettings.themePlayhead : UiSettings.themeTextDim
                        }
                        Label {
                            objectName: "collectionCell_key_" + row.index
                            readonly property int column: browser.columnIndexOf("key")
                            x: column >= 0 ? browser.columnX(column, row.width) + 6 : 0
                            width: column >= 0 ? Math.max(0, browser.columnWidth(column, row.width) - 12) : 0
                            visible: column >= 0
                            anchors.verticalCenter: parent.verticalCenter
                            text: browser.columnText(row.modelData, "key")
                            horizontalAlignment: Text.AlignRight
                            font.pixelSize: browser.listFont
                            color: row.current ? UiSettings.themePlayhead : UiSettings.themeTextDim
                        }
                    }
                    Label {
                        // The folder list keeps the names it has: a file is a file, and its name is
                        // how a DJ recognises it there.
                        visible: browser.section !== "collection"
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
                        acceptedButtons: Qt.LeftButton | Qt.RightButton
                        onClicked: function (mouse) {
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
                            // A click is a choice of place, not only of row.
                            browser.keepScrollOnCursor = true;
                            Browser.setCursor(row.modelData.path);
                            browser.keepScrollOnCursor = false;
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
