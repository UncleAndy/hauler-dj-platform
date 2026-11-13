// An entry of a context menu. Controls' own MenuItem paints its text with the palette's window text in
// every state, so an entry that cannot act looks exactly like the ones that can — and a menu is the one
// place that must not promise what it will refuse. The colour here follows `enabled`: the shell's dim
// grey when the entry is dead, the shell's text colour when it answers.
import QtQuick
import QtQuick.Controls

MenuItem {
    id: entry

    contentItem: Text {
        text: entry.text
        font: entry.font
        color: entry.enabled ? UiSettings.themeText : UiSettings.themeTextDim
        verticalAlignment: Text.AlignVCenter
        elide: Text.ElideRight
    }
}
