// Headphones, drawn after `assets/headphones.svg`: a band arching upward, two stems down, two outlined cups.
//
// The shell's monospaced font has no headphone glyph on Windows — the same trap the lock fell into — so the shape is
// drawn. The canvas is the owner's SVG, in its own 24-unit grid with the same stroke width and round joins: one
// drawing, no line by line guessing at radii (owner's instruction, 20.11: "возьми за образец headphones.svg").

import QtQuick

Canvas {
    id: glyph

    /// Colour of the shape; the caller decides whether it is lit.
    property color colour: "#d0d0d0"
    /// Thickness of the line in the SVG's own units (the drawing is scaled to fit).
    property real unitStroke: 2.0

    implicitWidth: 20
    implicitHeight: 20
    // Antialiasing and no sub-pixel drift: the glyph is a shape, not a picture.
    antialiasing: true

    onColourChanged: requestPaint()
    onWidthChanged: requestPaint()
    onHeightChanged: requestPaint()

    onPaint: {
        const context = getContext("2d");
        context.reset();
        // The SVG is 24 × 24; keep its proportions whatever the button's size.
        const scale = Math.min(width, height) / 24.0;
        context.save();
        context.scale(scale, scale);
        context.strokeStyle = glyph.colour;
        context.lineWidth = glyph.unitStroke;
        context.lineCap = "round";
        context.lineJoin = "round";

        // The band: M3 18 v-6 a9 9 0 0 1 18 0 v6 — up the left stem, over the arch, down the right stem.
        context.beginPath();
        context.moveTo(3, 18);
        context.lineTo(3, 12);
        context.arc(12, 12, 9, Math.PI, 0, false);
        context.lineTo(21, 18);
        context.stroke();

        // The right cup: M21 19 a2 2 0 0 1 -2 2 h-1 a2 2 0 0 1 -2 -2 v-3 a2 2 0 0 1 2 -2 h3 z
        context.beginPath();
        context.moveTo(21, 19);
        context.arc(19, 19, 2, 0, Math.PI / 2, false);
        context.lineTo(18, 21);
        context.arc(18, 19, 2, Math.PI / 2, Math.PI, false);
        context.lineTo(16, 16);
        context.arc(18, 16, 2, Math.PI, Math.PI * 1.5, false);
        context.lineTo(21, 14);
        context.closePath();
        context.stroke();

        // The left cup, mirrored: M3 19 a2 2 0 0 0 2 2 h1 a2 2 0 0 0 2 -2 v-3 a2 2 0 0 0 -2 -2 H3 z
        context.beginPath();
        context.moveTo(3, 19);
        context.arc(5, 19, 2, Math.PI, Math.PI / 2, true);
        context.lineTo(6, 21);
        context.arc(6, 19, 2, Math.PI / 2, 0, true);
        context.lineTo(8, 16);
        context.arc(6, 16, 2, 0, -Math.PI / 2, true);
        context.lineTo(3, 14);
        context.closePath();
        context.stroke();
        context.restore();
    }
}
