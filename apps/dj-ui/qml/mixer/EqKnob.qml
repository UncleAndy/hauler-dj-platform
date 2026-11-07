// One band of a channel's three-band EQ (A14, DJP-MIX-002).
//
// Round like the master knob and the same size as it: a band control is the same class of control as
// the master gain, and a smaller cap reads as a lesser one. The value is a band gain in decibels: -26
// at the foot, +26 at the top, unity at the top dead centre. The lit arc runs from unity to the
// knob's angle, so a cut and a boost are told apart by which way the light goes instead of by reading
// a number across a booth.
//
// The knob follows the hand while the button is down and the snapshot once it is released: a control
// that waits for a round trip through the engine reads as dead, and one that keeps its own number
// never follows a command that came from anywhere else.
import QtQuick
import QtQuick.Controls

Item {
    id: knob

    required property string deckId
    required property string band
    required property string letter
    // The band's gain in decibels, as the engine published it.
    required property real value
    // Diameter of the cap, taken from the master knob so the two cannot drift apart.
    required property real diameter

    implicitWidth: diameter
    implicitHeight: diameter + 11
    // What is actually drawn: the cap is the diameter even when the strip is a pixel narrower, so the
    // channel knobs and the master measure the same on screen and not only in the request.
    readonly property real capSide: diameter
    objectName: "eq_" + band + "_" + deckId

    readonly property real rangeDb: 26.0
    // What the knob shows: the hand's number while it is being dragged, the engine's otherwise.
    readonly property real db: knob.dragging ? knob.dragged : knob.value
    readonly property real fraction: Math.max(0.0, Math.min(1.0,
                                                    (knob.db + rangeDb) / (2.0 * rangeDb)))
    readonly property real angle: -135.0 + 270.0 * fraction
    readonly property real unityAngle: 0.0

    property bool dragging: false
    property real dragged: 0.0
    property real pressDb: 0.0
    property real pressY: 0.0
    // What the last gesture asked for, so a check can tell a dead knob from a refused command.
    property real lastDb: -1.0

    function ask(decibels) {
        const wanted = Math.max(-knob.rangeDb, Math.min(knob.rangeDb, decibels));
        knob.dragged = wanted;
        knob.lastDb = wanted;
        Bridge.setChannelEq(knob.deckId, knob.band, wanted);
    }

    Item {
        id: face
        readonly property real side: knob.capSide
        width: face.side
        height: face.side
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.top: parent.top

        Rectangle {
            anchors.fill: parent
            radius: width / 2
            color: UiSettings.themeBackground
            border.width: 1
            border.color: Math.abs(knob.db) < 0.01 ? UiSettings.themeBorder
                                                   : UiSettings.themeAccentDim
        }

        // From unity to where the knob stands: a boost lights to the right, a cut to the left.
        Canvas {
            id: arc
            anchors.fill: parent
            onPaint: {
                const ctx = getContext("2d");
                ctx.reset();
                const centre = width / 2;
                const radius = Math.min(width, height) / 2 - 0.5;
                const from = Math.min(knob.angle, knob.unityAngle) - 90.0;
                const to = Math.max(knob.angle, knob.unityAngle) - 90.0;
                if (to - from < 0.5) {
                    return;
                }
                ctx.strokeStyle = UiSettings.themeAccent.toString();
                ctx.lineWidth = 2;
                ctx.beginPath();
                ctx.arc(centre, centre, radius, from * Math.PI / 180.0, to * Math.PI / 180.0);
                ctx.stroke();
            }
        }

        onSideChanged: arc.requestPaint()

        // The pointer, from the centre outwards at the knob's angle.
        Rectangle {
            width: 1.5
            height: face.height * 0.34
            color: UiSettings.themeText
            antialiasing: true
            x: face.width / 2 - width / 2
            y: face.height / 2 - height + 2
            transformOrigin: Item.Bottom
            rotation: knob.angle
        }

        // Unity: a tick at the top dead centre, so flat is findable without the label.
        Rectangle {
            width: 1
            height: 2
            color: UiSettings.themeTextDim
            anchors.horizontalCenter: parent.horizontalCenter
            anchors.top: parent.top
        }
    }

    onAngleChanged: arc.requestPaint()

    // HI / MID / LO, the names a strip in the reference uses: the ear reads the word faster than the
    // letter, and three of them fit under knobs this size.
    Label {
        text: knob.letter
        height: 11
        anchors.top: face.bottom
        anchors.horizontalCenter: parent.horizontalCenter
        verticalAlignment: Text.AlignVCenter
        color: Math.abs(knob.db) < 0.01 ? UiSettings.themeTextDim : UiSettings.themeText
        font.pixelSize: 8
    }

    MouseArea {
        anchors.fill: parent
        cursorShape: Qt.SizeVerCursor
        onPressed: function (mouse) {
            knob.dragging = true;
            knob.pressDb = knob.db;
            knob.pressY = mouse.y;
            knob.dragged = knob.pressDb;
        }
        onPositionChanged: function (mouse) {
            if (!knob.dragging) {
                return;
            }
            // Half a decibel per pixel: the whole range in about a hundred pixels, fine enough to
            // hear a step and coarse enough to cross the range in one gesture.
            knob.ask(knob.pressDb + (knob.pressY - mouse.y) * 0.5);
        }
        onReleased: knob.dragging = false
        onCanceled: knob.dragging = false
        onWheel: function (wheel) {
            const notches = Math.max(-3, Math.min(3, Math.round(wheel.angleDelta.y / 120)));
            if (notches !== 0) {
                knob.ask(knob.db + notches * 0.5);
            }
        }
    }
}
