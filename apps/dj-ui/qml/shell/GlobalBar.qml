// The strip above the decks: what belongs to the whole set (DJP-UI-001).
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Pane {
    id: bar

    // The settings window covers the whole application, so it cannot live inside this strip: this bar only asks for
    // it, and the window that owns the arrangement puts it up.
    signal settingsAsked()

    background: Rectangle { color: UiSettings.themeBackground }

    /// The name a layout wears in the menu and on the button. The stored value stays short and stable;
    /// only the label carries the words.
    function labelFor(name) {
        switch (name) {
        case "classic":
            return qsTr("Classic");
        case "duo":
            return qsTr("Duo");
        case "stack-left":
            return qsTr("Stack \u00b7 mixer left");
        case "stack-right":
            return qsTr("Stack \u00b7 mixer right");
        case "stack-duo-left":
            return qsTr("Stack Duo \u00b7 mixer left");
        case "stack-duo-right":
            return qsTr("Stack Duo \u00b7 mixer right");
        case "studio-left":
            return qsTr("Studio \u00b7 mixer left");
        case "studio-right":
            return qsTr("Studio \u00b7 mixer right");
        case "stack-nomixer":
            return qsTr("Stack \u00b7 no mixer");
        case "stack-duo-nomixer":
            return qsTr("Stack Duo \u00b7 no mixer");
        case "studio-nomixer":
            return qsTr("Studio \u00b7 no mixer");
        case "custom":
            return qsTr("Custom");
        }
        return name;
    }

    RowLayout {
        anchors.fill: parent
        anchors.leftMargin: 8
        anchors.rightMargin: 8
        spacing: 12

        Label {
            text: qsTr("DJ Platform")
            font.bold: true
            color: UiSettings.themeText
        }
        // The layout lives here, where the old build only printed its name. One button, one list: the
        // arrangements are not a mode the DJ has to remember a flag for.
        Button {
            id: layoutButton
            objectName: "layoutButton"
            Layout.preferredWidth: 176
            text: qsTr("Layout: %1").arg(bar.labelFor(LayoutState.preset))
            font.pixelSize: 10
            padding: 4
            ToolTip.text: qsTr("Arrange the shell")
            ToolTip.visible: hovered
            // The list opens under the button, not at a fixed corner: a menu that appears far from the
            // hand that asked for it reads as a menu from another window (owner's report, 2026-11-19).
            onClicked: {
                // In the popup's own parent (the overlay), not in the strip: a popup is placed in the
                // window's coordinates, and a corner taken from the strip lands a few pixels off.
                const at = layoutButton.mapToItem(layoutMenu.parent, 0, 0);
                layoutMenu.x = at.x;
                layoutMenu.y = at.y + layoutButton.height;
                layoutMenu.open();
            }
        }
        Item { Layout.fillWidth: true }
        Label {
            text: Bridge.status
            color: Bridge.connected ? UiSettings.themeText : UiSettings.themeDanger
        }
        Label {
            // The endpoint is part of the message: "no engine" alone leaves the DJ guessing which name was
            // asked for, and the engine's own log answers with the name it bound.
            text: Bridge.connected
                  ? qsTr("rev %1").arg(Bridge.snapshot.revision !== undefined
                                        ? Bridge.snapshot.revision : 0)
                  : qsTr("no engine on %1").arg(Bridge.endpoint)
            color: UiSettings.themeTextDim
            font.pixelSize: 10
            elide: Text.ElideMiddle
            Layout.maximumWidth: 320
        }
        // Справа от счётчика и с постоянной шириной: текст состояния меняется каждую секунду, и
        // кнопка, стоящая слева от него, ездила бы под рукой.
        // One door to everything the three files hold. The gear stands next to the model button, where the
        // other window-wide actions live.
        Button {
            objectName: "settingsButton"
            Layout.preferredWidth: 92
            text: qsTr("Settings")
            font.pixelSize: 10
            padding: 4
            ToolTip.text: qsTr("Sound, library, effects, interface")
            ToolTip.visible: hovered
            onClicked: bar.settingsAsked()
        }
        Button {
            objectName: "modelButton"
            Layout.preferredWidth: 112
            text: Bridge.snapshot.model !== undefined && Bridge.snapshot.model.variant !== undefined
                  ? qsTr("Model: %1").arg(Bridge.snapshot.model.variant)
                  : qsTr("Model: built-in")
            font.pixelSize: 10
            padding: 4
            onClicked: modelPanel.open()
        }
    }

    Menu {
        id: layoutMenu
        objectName: "layoutMenu"
        // The place is set when the list is asked for: under the button that asked. These are only the
        // numbers before the first open. No style margins: the frame sits exactly where it was put.
        x: 0
        y: 0
        margins: 0

        MenuItem {
            objectName: "layoutChoice_classic"
            text: bar.labelFor("classic")
            checkable: true
            checked: LayoutState.preset === "classic"
            onTriggered: LayoutState.setLayout("classic")
        }
        MenuItem {
            objectName: "layoutChoice_duo"
            text: bar.labelFor("duo")
            checkable: true
            checked: LayoutState.preset === "duo"
            onTriggered: LayoutState.setLayout("duo")
        }
        MenuSeparator {}
        MenuItem {
            objectName: "layoutChoice_stack-left"
            text: bar.labelFor("stack-left")
            checkable: true
            checked: LayoutState.preset === "stack-left"
            onTriggered: LayoutState.setLayout("stack-left")
        }
        MenuItem {
            objectName: "layoutChoice_stack-right"
            text: bar.labelFor("stack-right")
            checkable: true
            checked: LayoutState.preset === "stack-right"
            onTriggered: LayoutState.setLayout("stack-right")
        }
        MenuSeparator {}
        MenuItem {
            objectName: "layoutChoice_stack-duo-left"
            text: bar.labelFor("stack-duo-left")
            checkable: true
            checked: LayoutState.preset === "stack-duo-left"
            onTriggered: LayoutState.setLayout("stack-duo-left")
        }
        MenuItem {
            objectName: "layoutChoice_stack-duo-right"
            text: bar.labelFor("stack-duo-right")
            checkable: true
            checked: LayoutState.preset === "stack-duo-right"
            onTriggered: LayoutState.setLayout("stack-duo-right")
        }
        MenuSeparator {}
        MenuItem {
            objectName: "layoutChoice_studio-left"
            text: bar.labelFor("studio-left")
            checkable: true
            checked: LayoutState.preset === "studio-left"
            onTriggered: LayoutState.setLayout("studio-left")
        }
        MenuItem {
            objectName: "layoutChoice_studio-right"
            text: bar.labelFor("studio-right")
            checkable: true
            checked: LayoutState.preset === "studio-right"
            onTriggered: LayoutState.setLayout("studio-right")
        }
        MenuSeparator {}
        MenuItem {
            objectName: "layoutChoice_stack-nomixer"
            text: bar.labelFor("stack-nomixer")
            checkable: true
            checked: LayoutState.preset === "stack-nomixer"
            onTriggered: LayoutState.setLayout("stack-nomixer")
        }
        MenuItem {
            objectName: "layoutChoice_stack-duo-nomixer"
            text: bar.labelFor("stack-duo-nomixer")
            checkable: true
            checked: LayoutState.preset === "stack-duo-nomixer"
            onTriggered: LayoutState.setLayout("stack-duo-nomixer")
        }
        MenuItem {
            objectName: "layoutChoice_studio-nomixer"
            text: bar.labelFor("studio-nomixer")
            checkable: true
            checked: LayoutState.preset === "studio-nomixer"
            onTriggered: LayoutState.setLayout("studio-nomixer")
        }
        MenuSeparator {}
        // Not a choice but a state: the numbers were moved in the layout the DJ is in.
        MenuItem {
            objectName: "layoutChoice_custom"
            text: bar.labelFor("custom")
            enabled: false
            visible: LayoutState.preset === "custom"
        }
    }

    ModelPanel {
        id: modelPanel
    }

}
