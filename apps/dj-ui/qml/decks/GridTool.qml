// A tool of the beatgrid strip. When it cannot act it is drawn flat and pale, not merely deaf: a lock
// that refuses clicks without looking different is a lock the DJ has to discover by trying.
import QtQuick
import QtQuick.Controls

Button {
    id: control

    focusPolicy: Qt.NoFocus
    implicitHeight: 18

    background: Rectangle {
        radius: 2
        color: UiSettings.themeRaised
        border.width: 1
        border.color: control.enabled ? UiSettings.themeBorder : UiSettings.themeRaised
        opacity: control.enabled ? 1.0 : 0.30
    }

    contentItem: Text {
        text: control.text
        font: control.font
        color: control.enabled ? UiSettings.themeText : UiSettings.themeTextDim
        horizontalAlignment: Text.AlignHCenter
        verticalAlignment: Text.AlignVCenter
        opacity: control.enabled ? 1.0 : 0.45
    }
}
