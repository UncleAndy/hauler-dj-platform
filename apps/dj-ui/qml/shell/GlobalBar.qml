// The strip above the decks: what belongs to the whole set (DJP-UI-001).
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Pane {
    id: bar

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
            objectName: "layoutButton"
            Layout.preferredWidth: 176
            text: qsTr("Layout: %1").arg(bar.labelFor(LayoutState.preset))
            font.pixelSize: 10
            padding: 4
            ToolTip.text: qsTr("Arrange the shell")
            ToolTip.visible: hovered
            onClicked: layoutMenu.open()
        }
        Item { Layout.fillWidth: true }
        Label {
            text: Bridge.status
            color: Bridge.connected ? UiSettings.themeText : UiSettings.themeDanger
        }
        Label {
            text: Bridge.connected
                  ? qsTr("rev %1").arg(Bridge.snapshot.revision !== undefined
                                        ? Bridge.snapshot.revision : 0)
                  : qsTr("no engine")
            color: UiSettings.themeTextDim
            font.pixelSize: 10
        }
        // Справа от счётчика и с постоянной шириной: текст состояния меняется каждую секунду, и
        // кнопка, стоящая слева от него, ездила бы под рукой.
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
        // Under the button, not at the pointer: a list that opens where the mouse happened to be is a
        // list the DJ has to look for.
        x: bar.width - width - 8
        y: bar.height

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
