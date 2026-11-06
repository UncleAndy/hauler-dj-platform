// The master gain knob, at the top of the mixer's middle (DJP-MIX-001).
//
// Round, because it is the one control that governs everything: a round cap reads as "the whole
// output" next to the square knobs of the channel strips. Travel is the same decibel range as a
// channel (-40 … +6, unity at 0), and the pointer shows where the knob stands rather than a filled
// bar, so the value is legible at a glance from across the booth.
import QtQuick
import QtQuick.Controls

Item {
    id: knob

    // Master gain as the engine reports it, linear.
    required property real gain

    implicitWidth: 46
    implicitHeight: 46
    objectName: "masterKnob"

    readonly property int minimumDb: -40
    readonly property int maximumDb: 6
    readonly property real db: gain > 0.0 ? 20.0 * Math.log(gain) / Math.LN10 : minimumDb
    readonly property real fraction: Math.max(0.0, Math.min(1.0,
                                       (db - minimumDb) / (maximumDb - minimumDb)))
    // 270 degrees of travel: -135 at the bottom, +135 at the top, the top dead centre at unity.
    readonly property real angle: -135.0 + 270.0 * fraction

    // What the last gesture asked for, so a check can tell a dead knob from a refused command.
    property real lastY: -1
    property real lastGain: -1
    property bool dragging: false
    property real dragged: 0.0

    function gainFor(decibels) {
        return Math.pow(10.0, decibels / 20.0);
    }

    function set(y) {
        const share = 1.0 - Math.max(0.0, Math.min(1.0, y / height));
        const decibels = minimumDb + share * (maximumDb - minimumDb);
        knob.lastY = y;
        knob.lastGain = knob.gainFor(decibels);
        knob.dragged = share;
        Bridge.setMasterGain(knob.lastGain);
    }

    Rectangle {
        anchors.fill: parent
        radius: width / 2
        color: UiSettings.themeBackground
        border.width: 1
        border.color: UiSettings.themeBorder
    }

    // The travelled part of the arc, drawn as a ring segment: the same angle the pointer shows.
    Canvas {
        id: arc
        anchors.fill: parent
        anchors.margins: 3
        onPaint: {
            const ctx = getContext("2d");
            ctx.reset();
            const centre = width / 2;
            const radius = Math.min(width, height) / 2 - 1;
            const start = (-135 - 90) * Math.PI / 180.0;
            const end = (knob.angle - 90) * Math.PI / 180.0;
            ctx.strokeStyle = UiSettings.themeAccent.toString();
            ctx.lineWidth = 2.5;
            ctx.beginPath();
            ctx.arc(centre, centre, radius, start, end);
            ctx.stroke();
        }
    }

    onAngleChanged: arc.requestPaint()

    // The pointer: from the centre outwards at the knob's angle.
    Rectangle {
        width: 2
        height: parent.height * 0.32
        color: UiSettings.themeText
        antialiasing: true
        x: parent.width / 2 - width / 2
        y: parent.height / 2 - height + 4
        transformOrigin: Item.Bottom
        rotation: knob.angle
    }

    MouseArea {
        anchors.fill: parent
        cursorShape: Qt.SizeVerCursor
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
        onWheel: function (wheel) {
            const notches = Math.max(-3, Math.min(3, Math.round(wheel.angleDelta.y / 120)));
            if (notches === 0) {
                return;
            }
            const decibels = knob.db + notches;
            Bridge.setMasterGain(knob.gainFor(decibels));
        }
    }
}
