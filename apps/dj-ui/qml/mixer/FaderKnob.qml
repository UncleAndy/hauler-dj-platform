// A channel's fader, drawn as a bright line across its level meter (DJP-MIX-001).
//
// The fader is the level control a hand rides during a mix: it moves constantly, it sits after the EQ,
// and it does not boost — 1.0 is the top of the scale, 0.0 is silence. The gain is a separate knob at
// the top of the strip, because a trim correction and a fader throw must never write one number.
//
// The pointer lives inside the meter's rectangle instead of beside it: a strip is narrow, and a
// separate column for the fader cost the meter a quarter of its width. The handle follows the hand
// while the button is down, because a control that waits for a round trip through the engine reads as
// dead.
import QtQuick

Item {
    id: knob

    required property string deckId
    // The channel's fader as the engine reports it, 0.0 … 1.0.
    required property real position

    objectName: "fader_" + deckId

    readonly property real fraction: Math.max(0.0, Math.min(1.0, position))
    // The pointer travels inside the meter, not on its frame: at unity it would otherwise be drawn on
    // the top edge, half of it clipped, and a fader whose line cannot be seen at rest reads as dead.
    readonly property real inset: 3
    // Where the handle is drawn, from the top.
    readonly property real handleY: knob.inset
                                    + (height - 2 * knob.inset)
                                      * (1.0 - (knob.dragging ? knob.dragged : knob.fraction))

    // What the last gesture asked for, so a check can tell a dead fader from a refused command.
    property real lastY: -1
    property real lastPosition: -1
    property bool dragging: false
    property real dragged: 0.0

    // Wheel: one notch is five percent of the throw, from the value the fader currently shows.
    signal scrolled(int notches)

    function set(y) {
        const travel = Math.max(1.0, height - 2 * knob.inset);
        const share = 1.0 - Math.max(0.0, Math.min(1.0, (y - knob.inset) / travel));
        knob.lastY = y;
        knob.lastPosition = share;
        knob.dragged = share;
        Bridge.setChannelFader(knob.deckId, share);
    }

    // A double click puts the fader back at the top, where a channel stands when it is open.
    function reset() {
        knob.lastPosition = 1.0;
        knob.dragged = 1.0;
        Bridge.setChannelFader(knob.deckId, 1.0);
    }

    // The top of the scale: unity, where a fader stands when the channel is open.
    Rectangle {
        anchors.left: parent.left
        anchors.right: parent.right
        y: knob.inset
        height: 1
        color: UiSettings.themeBorder
    }

    // The fader line: a clean cut across the meter, so the level below it and the headroom above it
    // are read at a glance instead of in two separate widgets.
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
        onDoubleClicked: knob.reset()
        onWheel: function (wheel) {
            const notches = Math.max(-3, Math.min(3, Math.round(wheel.angleDelta.y / 120)));
            if (notches !== 0) {
                knob.scrolled(notches);
            }
        }
    }
}
