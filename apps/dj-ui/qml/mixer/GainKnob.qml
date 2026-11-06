// A channel's gain knob (DJP-MIX-001).
//
// Vertical travel in decibels: -40 at the bottom, +6 at the top, unity at 0 dB. While the button is
// down the handle follows the hand — a control that waits for a round trip through the engine feels
// dead — and the snapshot takes over on release.
import QtQuick
import QtQuick.Controls

Item {
    id: knob

    required property string deckId
    // The deck's gain as the engine reports it, linear.
    required property real gain

    implicitWidth: 22
    objectName: "knob_" + deckId

    readonly property int minimumDb: -40
    readonly property int maximumDb: 6
    readonly property real db: gain > 0.0 ? 20.0 * Math.log(gain) / Math.LN10 : minimumDb
    readonly property real fraction: Math.max(0.0, Math.min(1.0,
                                       (db - minimumDb) / (maximumDb - minimumDb)))
    readonly property real unityFraction: (0.0 - minimumDb) / (maximumDb - minimumDb)

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

    Rectangle {
        anchors.fill: parent
        radius: 2
        color: UiSettings.themeBackground
        border.color: UiSettings.themeBorder
        border.width: 1
    }
    Rectangle {
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        height: parent.height * (knob.dragging ? knob.dragged : knob.fraction)
        color: UiSettings.themeAccentDim
    }
    // Unity: where the knob lives when nothing has been touched.
    Rectangle {
        anchors.left: parent.left
        anchors.right: parent.right
        y: parent.height * (1.0 - knob.unityFraction)
        height: 1
        color: UiSettings.themeBorder
    }
    Rectangle {
        width: parent.width
        height: 8
        radius: 1
        y: parent.height * (1.0 - (knob.dragging ? knob.dragged : knob.fraction)) - height / 2
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
