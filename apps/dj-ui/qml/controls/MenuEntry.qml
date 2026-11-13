// An entry of a context menu. Controls' own MenuItem paints its text with the palette's window text in
// every state, so an entry that cannot act looks exactly like the ones that can — and a menu is the one
// place that must not promise what it will refuse. The colour here follows `enabled`: the shell's dim
// grey when the entry is dead, the shell's text colour when it answers. The colour an entry carries as
// its icon is drawn as a swatch, so the cue colours keep their mark.
import QtQuick
import QtQuick.Controls

MenuItem {
    id: entry

    contentItem: Row {
        spacing: 6

        Rectangle {
            width: 10
            height: 10
            radius: 2
            anchors.verticalCenter: parent.verticalCenter
            // A colour entry keeps its swatch; a plain entry has no icon and takes no room for one.
            visible: entry.icon.color.a > 0.0
            color: entry.icon.color
        }

        Text {
            text: entry.text
            font: entry.font
            color: entry.enabled ? UiSettings.themeText : UiSettings.themeTextDim
            verticalAlignment: Text.AlignVCenter
        }
    }
}
