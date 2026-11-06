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

    // Which deck the browser loads into; shown there, never guessed.
    property string targetDeck: "A"

    // The browser folds away to give the decks the whole window. Space is its key: playback is not
    // bound to it, so the room can be resized without touching the sound.
    property bool browserOpen: true

    function toggleBrowser() {
        shell.browserOpen = !shell.browserOpen;
    }

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
                SplitView.preferredWidth: (columns.width - columns.mixerItem.width) / 2.0
                SplitView.minimumWidth: 200
                spacing: 6
                Repeater {
                    model: LayoutState.deckRows === 2 ? ["A", "C"] : ["A"]
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
                SplitView.preferredWidth: (columns.width - columns.mixerItem.width) / 2.0
                SplitView.minimumWidth: 200
                spacing: 6
                Repeater {
                    model: LayoutState.deckRows === 2 ? ["B", "D"] : ["B"]
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

            onWidthChanged: Qt.callLater(function () { shell.publishColumnSizes(false) })
        }

        BrowserPanel {
            id: browserPanel
            objectName: "browserPanel"
            // A hidden item takes no room in a SplitView, which is how the fold works without
            // touching any of the numbers the user set.
            visible: shell.browserOpen
            target: shell.targetDeck
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
        Component.onCompleted: Qt.callLater(function () { shell.publishColumnSizes(false) })
    }
}
