// Headphones, drawn rather than typed.
//
// The shell's monospaced font has no headphone glyph on Windows — the same trap the lock fell into (a missing glyph
// showed as an ellipsis). So the shape is geometry: a band over two cups, sized from `width`.

import QtQuick

Item {
    id: glyph

    /// Colour of the shape; the caller decides whether it is lit.
    property color colour: "#d0d0d0"
    /// Thickness of the lines, in pixels.
    property real stroke: Math.max(1.0, Math.round(width / 10))

    implicitWidth: 16
    implicitHeight: 16

    // The band: a round-topped arc, made by clipping the top half of a rounded rectangle.
    Item {
        width: parent.width
        height: parent.height * 0.62
        clip: true

        Rectangle {
            anchors.horizontalCenter: parent.horizontalCenter
            anchors.bottom: parent.bottom
            width: glyph.width - glyph.stroke * 2
            height: glyph.width * 0.9
            radius: width / 2
            color: "transparent"
            border.width: glyph.stroke
            border.color: glyph.colour
        }
    }

    // The two cups: small rounded blocks at the ends of the band.
    Rectangle {
        anchors.left: parent.left
        anchors.bottom: parent.bottom
        width: Math.max(3, Math.round(glyph.width * 0.26))
        height: Math.max(4, Math.round(glyph.height * 0.34))
        radius: Math.max(1, Math.round(glyph.width / 12))
        color: glyph.colour
    }

    Rectangle {
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        width: Math.max(3, Math.round(glyph.width * 0.26))
        height: Math.max(4, Math.round(glyph.height * 0.34))
        radius: Math.max(1, Math.round(glyph.width / 12))
        color: glyph.colour
    }
}
