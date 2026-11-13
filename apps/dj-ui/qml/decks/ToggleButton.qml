// A deck control with two states, painted the way Snap and Quant are: the accent when it is on, the
// raised face when it is off, the same text colour in both. One paint for every switch in the deck — a
// lit button that means "on" must not depend on which row it happens to sit in.
import QtQuick
import QtQuick.Controls

Button {
    id: control

    /// On or off. The caller binds it to the state the engine publishes; the button only draws it.
    property bool lit: false

    focusPolicy: Qt.NoFocus
    implicitHeight: 20

    background: Rectangle {
        radius: 2
        color: control.lit ? UiSettings.themeAccent : UiSettings.themeRaised
        border.width: 1
        border.color: UiSettings.themeBorder
        // A switch that cannot be pressed reads as unavailable, like the greyed grid tools.
        opacity: control.enabled ? 1.0 : 0.45
    }

    contentItem: Text {
        text: control.text
        font: control.font
        color: control.lit ? UiSettings.themeTextOnAccent : UiSettings.themeText
        horizontalAlignment: Text.AlignHCenter
        verticalAlignment: Text.AlignVCenter
        elide: Text.ElideRight
        opacity: control.enabled ? 1.0 : 0.45
    }
}
