// A channel's gain pointer, drawn on its level meter (DJP-MIX-001).
//
// The pointer lives inside the meter's rectangle instead of beside it: a strip is narrow, and a
// separate column for the fader cost the meter a quarter of its width. Vertical travel in decibels —
// -40 at the foot, +6 at the top, unity marked — and the handle follows the hand while the button is
// down, because a control that waits for a round trip through the engine feels dead.
import QtQuick

Item {
    id: knob

    required property string deckId
    // The deck's gain as the engine reports it, linear.
    required property real gain

    objectName: "knob_" + deckId

    readonly property int minimumDb: -40
    readonly property int maximumDb: 6
    readonly property real db: gain > 0.0 ? 20.0 * Math.log(gain) / Math.LN10 : minimumDb
    readonly property real fraction: Math.max(0.0, Math.min(1.0,
                                       (db - minimumDb) / (maximumDb - minimumDb)))
    readonly property real unityFraction: (0.0 - minimumDb) / (maximumDb - minimumDb)
    // Where the handle is drawn, from the top.
    readonly property real handleY: height * (1.0 - (knob.dragging ? knob.dragged : knob.fraction))

    // What the last gesture asked for, so a check can tell a dead knob from a refused command.
    property real lastY: -1
    property real lastGain: -1
    property bool dragging: false
    property real dragged: 0.0

    function gainFor(decibels) {
        return Math.pow(10.0, decibels / 20.0);
    }
    // Wheel: one notch is one decibel, from the value the knob currently shows.
    signal scrolled(int notches)

    function set(y) {
        const share = 1.0 - Math.max(0.0, Math.min(1.0, y / height));
        const decibels = minimumDb + share * (maximumDb - minimumDb);
        knob.lastY = y;
        knob.lastGain = knob.gainFor(decibels);
        knob.dragged = share;
        Bridge.setChannelGain(knob.deckId, knob.lastGain);
    }

    // Unity: where the pointer lives when nothing has been touched.
    Rectangle {
        anchors.left: parent.left
        anchors.right: parent.right
        y: parent.height * (1.0 - knob.unityFraction)
        height: 1
        color: UiSettings.themeBorder
    }

    // The gain line: a clean cut across the meter, so the level below it and the gain above it are read
    // at a glance instead of in two separate widgets.
    Rectangle {
        anchors.left: parent.left
        anchors.right: parent.right
        y: knob.handleY - height / 2
        height: 2
        color: UiSettings.themeText
    }

    // The grip: the part the hand takes. Narrow, so most of the meter stays visible.
    Rectangle {
        width: Math.min(18, parent.width * 0.55)
        height: 7
        radius: 1
        x: (parent.width - width) / 2
        y: knob.handleY - height / 2
        color: UiSettings.themeRaised
        border.color: UiSettings.themeTextDim
        border.width: 1
    }

    MouseArea {
        anchors.fill: parent
        onPressed: function (mouse) {
            knob.dragging = true;
            knob.set(mouse.y);
        }
        onPositionChanged: function (mouse) {
            if (knob.dragging) {
                knob.set(mouse.y);
            }
        }
        onReleased: knob.dragging = false
        onCanceled: knob.dragging = false
        // The wheel is taken here, where the pointer actually is: a handler on the strip never saw a
        // wheel that this area had already claimed.
        onWheel: function (wheel) {
            const notches = Math.max(-3, Math.min(3, Math.round(wheel.angleDelta.y / 120)));
            if (notches !== 0) {
                knob.scrolled(notches);
            }
        }
    }
}
