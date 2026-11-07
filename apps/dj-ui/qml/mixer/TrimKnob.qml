// A channel's gain — the trim, as the gear calls it (DJP-MIX-001).
//
// This is not the fader. Gain lines a track's level up once, before the EQ, and then stays where it
// was put; the fader is the level control a hand rides during a mix and it lives on the meter. Keeping
// them apart is why this is a knob at the top of the strip: a trim correction and a fader throw must
// never move the same number.
//
// Travel is the same decibel range as before (-40 … +6, unity near the top), and the arc runs from
// unity to the knob's angle, so a boost and a cut are told apart by which way the light goes.
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

    readonly property int minimumDb: -40
    readonly property int maximumDb: 6
    readonly property real db: gain > 0.0 ? 20.0 * Math.log(gain) / Math.LN10 : minimumDb
    readonly property real fraction: Math.max(0.0, Math.min(1.0,
                                                    (db - minimumDb) / (maximumDb - minimumDb)))
    readonly property real unityFraction: (0.0 - minimumDb) / (maximumDb - minimumDb)
    readonly property real angle: -135.0 + 270.0 * fraction
    readonly property real unityAngle: -135.0 + 270.0 * unityFraction

    property bool dragging: false
    property real draggedDb: 0.0
    property real pressDb: 0.0
    property real pressY: 0.0
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
        readonly property real side: Math.min(knob.width, knob.diameter)
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
            knob.draggedDb = knob.pressDb;
        }
        onPositionChanged: function (mouse) {
            if (!knob.dragging) {
                return;
            }
            knob.ask(knob.pressDb + (knob.pressY - mouse.y) * 0.5);
        }
        onReleased: knob.dragging = false
        onCanceled: knob.dragging = false
        onWheel: function (wheel) {
            const notches = Math.max(-3, Math.min(3, Math.round(wheel.angleDelta.y / 120)));
            if (notches !== 0) {
                knob.ask(knob.db + notches);
            }
        }
    }
}
