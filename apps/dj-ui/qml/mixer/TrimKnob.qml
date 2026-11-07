// A channel's gain — the trim, as the gear calls it (DJP-MIX-001).
//
// This is not the fader. Gain lines a track's level up once, before the EQ, and then stays where it
// was put; the fader is the level control a hand rides during a mix and it lives on the meter. Keeping
// them apart is why this is a knob at the top of the strip: a trim correction and a fader throw must
// never move the same number.
//
// Zero stands straight up, as a trim has it: a trim is set around unity in both directions, so the cut
// (40 dB) and the boost (6 dB) each get 135 degrees of the same travel, and the pointer at unity points
// at twelve o'clock. The arc runs from unity to the knob's angle, so a boost and a cut are told apart by
// which way the light goes.
import QtQuick
import QtQuick.Controls

Item {
    id: knob

    required property string deckId
    // The channel's gain as the engine reports it, linear.
    required property real gain
    // Diameter of the cap, taken from the master knob so the two cannot drift apart.
    required property real diameter

    implicitWidth: diameter
    implicitHeight: diameter + 11
    objectName: "trim_" + deckId
    readonly property real capSide: diameter

    readonly property int minimumDb: -40
    readonly property int maximumDb: 6
    readonly property real db: gain > 0.0 ? 20.0 * Math.log(gain) / Math.LN10 : minimumDb
    // Zero up: 135 degrees each way, the cut compressed into 40 dB and the boost into 6.
    readonly property real angle: db >= 0.0
                                   ? 135.0 * db / maximumDb
                                   : 135.0 * db / (-minimumDb)
    readonly property real unityAngle: 0.0

    property bool dragging: false
    property real draggedDb: 0.0
    property real pressDb: 0.0
    property real pressY: 0.0
    property real pressAngle: 0.0
    // Two hundred and seventy degrees over the cap, the same in both directions: a trim is a knob, so the
    // hand turns the pointer at one rate and the decibels follow the angle.
    readonly property real degPerPixel: 270.0 / knob.capSide
    // One wheel notch is a fixed turn, not a fixed number of decibels: the wheel moves the pointer, and a
    // pointer cannot step evenly in angle and unevenly in decibels at the same time.
    readonly property real degPerNotch: 13.5

    /// The angle the pointer stands at for a gain.
    function angleOf(decibels) {
        return decibels >= 0.0 ? 135.0 * decibels / knob.maximumDb
                               : 135.0 * decibels / (-knob.minimumDb);
    }

    /// The gain the pointer's angle means, clamped to the ends of the travel.
    function dbOf(angle) {
        const clamped = Math.max(-135.0, Math.min(135.0, angle));
        return clamped >= 0.0 ? clamped / 135.0 * knob.maximumDb
                              : clamped / 135.0 * (-knob.minimumDb);
    }
    // What the last gesture asked for, so a check can tell a dead knob from a refused command.
    property real lastDb: -1.0

    function ask(decibels) {
        const wanted = Math.max(knob.minimumDb, Math.min(knob.maximumDb, decibels));
        knob.draggedDb = wanted;
        knob.lastDb = wanted;
        Bridge.setChannelGain(knob.deckId, Math.pow(10.0, wanted / 20.0));
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
            border.color: UiSettings.themeBorder
        }

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

        // Unity, where the pointer stands when the track is lined up.
        Rectangle {
            width: 1
            height: 2
            color: UiSettings.themeTextDim
            anchors.horizontalCenter: parent.horizontalCenter
            anchors.top: parent.top
            rotation: knob.unityAngle
            transformOrigin: Item.Bottom
            y: 0
        }
    }

    onAngleChanged: arc.requestPaint()

    Label {
        text: qsTr("GAIN")
        height: 11
        anchors.top: face.bottom
        anchors.horizontalCenter: parent.horizontalCenter
        verticalAlignment: Text.AlignVCenter
        color: UiSettings.themeTextDim
        font.pixelSize: 8
    }

    MouseArea {
        anchors.fill: parent
        cursorShape: Qt.SizeVerCursor
        onPressed: function (mouse) {
            knob.dragging = true;
            knob.pressDb = knob.db;
            knob.pressY = mouse.y;
            knob.pressAngle = knob.angleOf(knob.pressDb);
            knob.draggedDb = knob.pressDb;
        }
        onPositionChanged: function (mouse) {
            if (!knob.dragging) {
                return;
            }
            // The hand turns the pointer at one rate; the trim comes from the angle it lands on.
            knob.ask(knob.dbOf(knob.pressAngle + (knob.pressY - mouse.y) * knob.degPerPixel));
        }
        onReleased: knob.dragging = false
        // A double click trims back to unity.
        onDoubleClicked: knob.ask(0.0)
        onCanceled: knob.dragging = false
        onWheel: function (wheel) {
            const notches = Math.max(-3, Math.min(3, Math.round(wheel.angleDelta.y / 120)));
            if (notches !== 0) {
                knob.ask(knob.dbOf(knob.angleOf(knob.db) + notches * knob.degPerNotch));
            }
        }
    }
}
