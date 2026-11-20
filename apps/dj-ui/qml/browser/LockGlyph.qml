import QtQuick

// A padlock, drawn rather than typed.
//
// The lock was a character (U+1F512), and a character is a font's business: on the owner's Windows the flag was set
// and the cell stayed empty, because the shell's monospace face carries no such glyph and the fallback is not
// something a package can count on (owner's report, 2026-11-20). These are rectangles: the same shape on every
// machine, in the colour of the row it sits on.
Item {
    id: glyph
    implicitWidth: 12
    implicitHeight: 14
    property color colour: "#d0d0d0"
    /// The shackle's line: a lock drawn with a hairline is a lock nobody sees.
    property real stroke: Math.max(1.0, Math.round(glyph.width / 8))
    readonly property real shackleWidth: Math.round(glyph.width * 0.62)

    Rectangle {
        x: Math.round((glyph.width - glyph.shackleWidth) / 2)
        y: 0
        width: glyph.shackleWidth
        height: Math.round(glyph.height * 0.52)
        radius: width / 2
        color: "transparent"
        border.width: glyph.stroke
        border.color: glyph.colour
    }

    Rectangle {
        x: 1
        y: Math.round(glyph.height * 0.42)
        width: glyph.width - 2
        height: glyph.height - y
        radius: Math.max(1, Math.round(glyph.width / 6))
        color: glyph.colour
    }
}
