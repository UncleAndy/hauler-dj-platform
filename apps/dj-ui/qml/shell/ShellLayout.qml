// The shell's arrangement (DJP-UI-001).
//
// The proportions come from LayoutSettings, whose default is measured from the reference: a global
// strip across the top, two rows of decks with one mixer column between them, the browser along the
// bottom. Both splitters are draggable, and a dropped splitter writes its number back, so the
// arrangement is a setting rather than a decision made in code.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

import "../decks"
import "../mixer"
import "../browser"

Item {
    id: shell
    objectName: "shellLayout"

    // Which deck the browser loads into; shown there, never guessed.
    property string targetDeck: "A"

    // The browser folds away to give the decks the whole window. Space is its key: playback is not
    // bound to it, so the room can be resized without touching the sound.
    property bool browserOpen: true

    function toggleBrowser() {
        shell.browserOpen = !shell.browserOpen;
    }

    // What the arrangement puts where. One instance per deck, always: a deck named twice would give the
    // checks two `deckPanel_A` and the DJ two panels that disagree about the same music.
    readonly property string arrangement: LayoutState.arrangement

    /// Which decks stand in each column for a given arrangement. The only place that decides it, so the
    /// shell, the browser and the checks cannot drift apart.
    function columnsFor(name) {
        if (name === "classic") {
            return { left: ["A", "C"], right: ["B", "D"] };
        }
        if (name === "duo") {
            return { left: ["A"], right: ["B"] };
        }
        if (name === "stack" || name === "stack-duo") {
            // One column of decks with the mixer at an edge: the empty column is not drawn at all, so
            // the mixer stands where it was asked to stand.
            const decks = name === "stack" ? ["A", "B", "C", "D"] : ["A", "B"];
            return LayoutState.mixerEdge === "right" ? { left: decks, right: [] }
                                                     : { left: [], right: decks };
        }
        if (name === "studio") {
            return LayoutState.mixerEdge === "right" ? { left: ["A"], right: [] }
                                                     : { left: [], right: ["A"] };
        }
        return { left: [], right: [] };
    }

    // Set by `applyArrangement`, not bound: a binding here is read once at start before the shell has
    // finished, and a `concat` of two properties that do not exist yet is a scene that fails to load.
    property var leftDeckIds: ["A", "C"]
    property var rightDeckIds: ["B", "D"]
    /// Every deck this arrangement puts on the screen, in reading order.
    readonly property var shownDeckIds: leftDeckIds.concat(rightDeckIds)
    // Two columns share what the mixer leaves; one column takes all of it.
    readonly property bool twoColumns: leftDeckIds.length > 0 && rightDeckIds.length > 0

    function applyArrangement() {
        const columns = columnsFor(arrangement);
        shell.leftDeckIds = columns.left;
        shell.rightDeckIds = columns.right;
        // A deck the layout does not show cannot be the target: loading into it would put a track on a
        // panel nobody can see. The browser follows this, and so does the header.
        if (shell.shownDeckIds.indexOf(shell.targetDeck) < 0) {
            shell.targetDeck = "A";
        }
    }

    onArrangementChanged: applyArrangement()
    Component.onCompleted: applyArrangement()

    Shortcut {
        objectName: "browserToggle"
        sequence: "Space"
        context: Qt.ApplicationShortcut
        onActivated: shell.toggleBrowser()
    }

    // The splitter writes percentages back so the next start has the same proportions.
    function publishRowSizes() {
        const total = rows.height;
        if (total <= 0) {
            return;
        }
        LayoutState.preset = "custom";
        LayoutState.globalPercent = rows.itemAt(0).height / total * 100.0;
        // The deck area is the remainder by construction; only the two boundaries are choices.
        LayoutState.browserPercent = rows.itemAt(2).height / total * 100.0;
        LayoutState.save();
    }

    // Reading the current splitter position into the settings; `remember` is false at start, where
    // there is nothing to remember yet.
    function publishColumnSizes(remember) {
        const total = columns.width;
        if (total <= 0) {
            return;
        }
        LayoutState.mixerPercent = columns.mixerItem.width / total * 100.0;
        if (remember === true) {
            LayoutState.save();
        }
    }

    SplitView {
        id: rows
        anchors.fill: parent
        orientation: Qt.Vertical

        // Global strip: what belongs to the whole set, not to a deck.
        GlobalBar {
            id: globalBar
            objectName: "globalBar"
            SplitView.preferredHeight: shell.height * LayoutState.globalPercent / 100.0
            SplitView.minimumHeight: 34
        }

        SplitView {
            id: columns
            objectName: "deckArea"
            orientation: Qt.Horizontal
            SplitView.fillHeight: true
            readonly property Item mixerItem: mixerPanel

            // Decks A and C on the left, B and D on the right: a deck's column is its side of the
            // mixer, and the two rows of a side are the same instrument at two moments.
            ColumnLayout {
                id: leftDecks
                objectName: "leftDecks"
                visible: shell.leftDeckIds.length > 0
                // A column asks for a little less than its share and lets `fillWidth` absorb the
                // difference, so the mixer keeps the width its setting gives it.
                SplitView.preferredWidth: shell.twoColumns
                                          ? Math.max(200, (columns.width - columns.mixerItem.width) / 2.0 - 8)
                                          : Math.max(200, columns.width - columns.mixerItem.width - 8)
                SplitView.fillWidth: true
                SplitView.minimumWidth: 200
                spacing: 6
                Repeater {
                    model: shell.leftDeckIds
                    delegate: DeckPanel {
                        required property string modelData
                        objectName: "deckPanel_" + modelData
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        deckId: modelData
                        deckLabel: qsTr("Deck %1").arg(modelData)
                        targeted: shell.targetDeck === modelData
                        onChosen: shell.targetDeck = modelData
                    }
                }
            }

            MixerPanel {
                id: mixerPanel
                objectName: "mixer"
                SplitView.preferredWidth: columns.width * LayoutState.mixerPercent / 100.0
                SplitView.minimumWidth: 120
            }

            ColumnLayout {
                id: rightDecks
                objectName: "rightDecks"
                visible: shell.rightDeckIds.length > 0
                // The divider handles take their own pixels out of the row: asking for the whole
                // share squeezes the mixer to its minimum instead.
                SplitView.preferredWidth: shell.twoColumns
                                          ? Math.max(200, (columns.width - columns.mixerItem.width) / 2.0 - 8)
                                          : 0
                SplitView.fillWidth: true
                SplitView.minimumWidth: 200
                spacing: 6
                Repeater {
                    model: shell.rightDeckIds
                    delegate: DeckPanel {
                        required property string modelData
                        objectName: "deckPanel_" + modelData
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        deckId: modelData
                        deckLabel: qsTr("Deck %1").arg(modelData)
                        targeted: shell.targetDeck === modelData
                        onChosen: shell.targetDeck = modelData
                    }
                }
            }

            // The mixer's share is written when the DJ drops the divider, not on every relayout: a
            // width read before the splitter has distributed its space is a guess, and a guess written
            // back to the settings shrinks the mixer under the hand.
            onResizingChanged: {
                if (!columns.resizing) {
                    shell.publishColumnSizes(true);
                }
            }
        }

        BrowserPanel {
            id: browserPanel
            objectName: "browserPanel"
            // A hidden item takes no room in a SplitView, which is how the fold works without
            // touching any of the numbers the user set.
            visible: shell.browserOpen
            target: shell.targetDeck
            shownDecks: shell.shownDeckIds
            SplitView.preferredHeight: shell.height * LayoutState.browserPercent / 100.0
            SplitView.minimumHeight: 90
        }

        // A dropped handle is the only moment the numbers change, so the file is written once per
        // gesture rather than on every pixel of the drag.
        onResizingChanged: {
            if (!rows.resizing) {
                shell.publishRowSizes();
            }
        }
        // Nothing is published at start: the numbers come from the settings, and the shell is built
        // to them rather than the other way round.
    }
}
