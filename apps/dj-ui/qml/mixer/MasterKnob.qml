// The master gain knob at the top of the mixer's middle column (DJP-MIX-001).
//
// A volume knob, not a trim: it only attenuates, so unity sits at the far right — three o'clock — and
// the travel runs from silence at the lower left up to full at the right. That is the position a hand
// expects on a master level; a trim, by contrast, has its zero straight up, because a trim is set
// around unity in both directions.
//
// The cap is the same size as the channel gain knobs and drawn the same way (same ring, same pointer):
// the master is the gain of the whole mix, not a different class of control.
import QtQuick
import QtQuick.Controls

Item {
    id: knob

    // Master gain as the engine reports it, linear.
    required property real gain
    // Diameter of the cap, taken from the channel knobs so the two cannot drift apart.
    required property real diameter

    implicitWidth: diameter
    implicitHeight: diameter
    objectName: "masterKnob"

    // The scale: silence at the foot, unity at the end. A master level does not boost.
    readonly property int floorDb: -60
    readonly property real db: gain > 0.0 ? 20.0 * Math.log(gain) / Math.LN10 : knob.floorDb
    readonly property real shownDb: Math.max(knob.floorDb, Math.min(0.0, knob.db))
    // 225 degrees of travel: -135 at silence, +90 — straight right — at unity.
    readonly property real angle: knob.angleFor(knob.shownDb)

    property bool dragging: false
    property real draggedDb: 0.0
    property real pressDb: 0.0
    property real pressY: 0.0
    // Decibels per pixel of drag: the same both ways, and a press does not move the value — a level knob
    // that jumps to wherever the pointer landed is a level jump in the room.
    readonly property real dbPerPixel: 0.5
    // What the last gesture asked for, so a check can tell a dead knob from a refused command.
    property real lastDb: 1.0

    function angleFor(decibels) {
        const share = (Math.max(knob.floorDb, Math.min(0.0, decibels)) - knob.floorDb)
                      / (0.0 - knob.floorDb);
        return -135.0 + 225.0 * share;
    }

    function ask(decibels) {
        const wanted = Math.max(knob.floorDb, Math.min(0.0, decibels));
        knob.draggedDb = wanted;
        knob.lastDb = wanted;
        Bridge.setMasterGain(Math.pow(10.0, wanted / 20.0));
    }



    Rectangle {
        anchors.fill: parent
        radius: width / 2
        color: UiSettings.themeBackground
        border.width: 1
        border.color: UiSettings.themeBorder
    }

    // The travelled part of the ring, from silence to where the knob stands: a volume knob fills from
    // the foot of its scale, so the lit part is the amount of gain in hand.
    Canvas {
        id: arc
        anchors.fill: parent
        anchors.margins: 3
        onPaint: {
            const ctx = getContext("2d");
            ctx.reset();
            const centre = width / 2;
            const radius = Math.min(width, height) / 2 - 1;
            // Canvas measures from three o'clock clockwise; the knob measures from twelve, so both
            // ends lose ninety degrees.
            const startDeg = -135.0 - 90.0;
            const endDeg = knob.shownAngle - 90.0;
            if ((endDeg - startDeg) < 0.5) {
                return;
            }
            ctx.strokeStyle = UiSettings.themeAccent.toString();
            ctx.lineWidth = 2;
            ctx.beginPath();
            ctx.arc(centre, centre, radius, startDeg * Math.PI / 180.0, endDeg * Math.PI / 180.0);
            ctx.stroke();
        }
    }

    readonly property real shownAngle: knob.dragging ? knob.angleFor(knob.draggedDb) : knob.angle
    onShownAngleChanged: arc.requestPaint()
    onWidthChanged: arc.requestPaint()

    // The pointer: from the centre outwards at the knob's angle.
    Rectangle {
        width: 1.5
        height: parent.height * 0.34
        color: UiSettings.themeText
        antialiasing: true
        x: parent.width / 2 - width / 2
        y: parent.height / 2 - height + 2
        transformOrigin: Item.Bottom
        rotation: knob.shownAngle
    }

    // Unity: a tick where the pointer stands when the master is open.
    Rectangle {
        width: 1
        height: 2
        color: UiSettings.themeTextDim
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.top: parent.top
        rotation: 90.0
        transformOrigin: Item.Bottom
        y: 0
    }

    MouseArea {
        anchors.fill: parent
        cursorShape: Qt.SizeVerCursor
        onPressed: function (mouse) {
            knob.dragging = true;
            knob.pressDb = knob.shownDb;
            knob.pressY = mouse.y;
            knob.draggedDb = knob.pressDb;
        }
        onPositionChanged: function (mouse) {
            if (knob.dragging) {
                knob.ask(knob.pressDb + (knob.pressY - mouse.y) * knob.dbPerPixel);
            }
        }
        onReleased: knob.dragging = false
        // A double click stands the master at three quarters of its throw: high enough to play, low enough
        // that a forgotten master does not leave the room at full scale.
        onDoubleClicked: knob.ask(knob.floorDb + 0.75 * (0.0 - knob.floorDb))
        onCanceled: knob.dragging = false
        onWheel: function (wheel) {
            const notches = Math.max(-3, Math.min(3, Math.round(wheel.angleDelta.y / 120)));
            if (notches !== 0) {
                knob.ask(knob.shownDb + notches);
            }
        }
    }
}
