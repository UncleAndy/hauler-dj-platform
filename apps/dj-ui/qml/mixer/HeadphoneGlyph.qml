// Headphones, drawn rather than typed.
//
// The shell's monospaced font has no headphone glyph on Windows — the same trap the lock fell into (a missing glyph
// showed as an ellipsis). So the shape is geometry: a band arching upward over two cups.
//
// The band is the top half of a round rectangle, clipped at its middle: an arc opening downward is a band, an arc
// opening upward is a bowl (owner's correction, 20.11).

import QtQuick

Item {
    id: glyph

    /// Colour of the shape; the caller decides whether it is lit.
    property color colour: "#d0d0d0"
    /// Thickness of the lines, in pixels.
    property real stroke: Math.max(1.0, Math.round(width / 12))

    implicitWidth: 16
    implicitHeight: 16

    readonly property real bandHeight: Math.max(4, Math.round(glyph.height * 0.55))
    readonly property real cupHeight: Math.max(4, Math.round(glyph.height * 0.42))

    // The band: the upper half of a rounded rectangle, so only the arch that points up is visible.
    Item {
        width: glyph.width
        height: glyph.bandHeight / 2
        clip: true

        Rectangle {
            anchors.horizontalCenter: parent.horizontalCenter
            anchors.top: parent.top
            width: glyph.width - glyph.stroke * 2
            height: glyph.bandHeight
            radius: width / 2
            color: "transparent"
            border.width: glyph.stroke
            border.color: glyph.colour
        }
    }

    // The cups: two upright rounded blocks under the ends of the band, the shape a hand knows.
    Rectangle {
        anchors.left: parent.left
        anchors.bottom: parent.bottom
        width: Math.max(3, Math.round(glyph.width * 0.3))
        height: glyph.cupHeight
        radius: Math.max(1, Math.round(glyph.width / 10))
        color: glyph.colour
    }

    Rectangle {
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        width: Math.max(3, Math.round(glyph.width * 0.3))
        height: glyph.cupHeight
        radius: Math.max(1, Math.round(glyph.width / 10))
        color: glyph.colour
    }
}
