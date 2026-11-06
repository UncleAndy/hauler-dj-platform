// The crossfader law as a picture: both sides, drawn across the handle's travel.
//
// A person picks a law because of its shape, not its name, so the button shows the shape. The
// formulas are the engine's own (dj-mixer::side_gain): if they ever disagree, the picture is a lie and
// the test below is what catches it.
//
// Static: it repaints when the law or the size changes, never per frame.
import QtQuick

Canvas {
    id: glyph

    property string law: "constant_power"
    // The side that is open at the left of the crossfader.
    property color open: "#5090f0"
    // The side that closes there.
    property color closed: "#8a8a8a"
    property color guide: "#383838"

    onLawChanged: requestPaint()
    onWidthChanged: requestPaint()
    onHeightChanged: requestPaint()
    onOpenChanged: requestPaint()
    onClosedChanged: requestPaint()
    Component.onCompleted: requestPaint()

    // One side's gain at a position, following the same laws the mixer applies.
    function gain(position) {
        const p = Math.max(0.0, Math.min(1.0, position));
        switch (law) {
        case "linear":
            return p;
        case "smooth":
            return p * p * (3.0 - 2.0 * p);
        case "sharp":
            return Math.min(1.0, p / 0.25);
        default:
            return Math.sin(p * Math.PI / 2.0);
        }
    }

    onPaint: {
        const ctx = getContext("2d");
        ctx.reset();
        const w = width;
        const h = height;
        const pad = 3;
        if (w <= 2 * pad || h <= 2 * pad) {
            return;
        }

        // The middle of the travel, so a law's crossing point reads at a glance.
        ctx.strokeStyle = guide;
        ctx.lineWidth = 1;
        ctx.beginPath();
        ctx.moveTo(Math.round(w / 2) + 0.5, pad);
        ctx.lineTo(Math.round(w / 2) + 0.5, h - pad);
        ctx.stroke();

        const plot = function (side, colour) {
            ctx.strokeStyle = colour;
            ctx.lineWidth = 1.5;
            ctx.beginPath();
            for (let step = 0; step <= 40; ++step) {
                const t = step / 40.0;
                const x = pad + t * (w - 2 * pad);
                const y = h - pad - side(t) * (h - 2 * pad);
                if (step === 0) {
                    ctx.moveTo(x, y);
                } else {
                    ctx.lineTo(x, y);
                }
            }
            ctx.stroke();
        };
        plot(function (t) { return glyph.gain(1.0 - t); }, open);
        plot(function (t) { return glyph.gain(t); }, closed);
    }
}
