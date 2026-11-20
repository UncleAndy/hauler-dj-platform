// One of the two headphone knobs in the middle of the mixer: the volume and the proportion between monitoring and
// the main mix (owner's decision, 20.11).
//
// The same cap as the master knob — a ring with a pointer, no fill — because these are the same class of control: a
// level a hand sets while a track plays. The difference is the law: volume runs 0 … 2 linear, the proportion 0 … 1,
// and both start where the last snapshot says they are.

import QtQuick

Item {
    id: knob

    /// Which knob this is: `phones` sends the volume, `mix` the proportion.
    required property string kind
    /// The value the engine published: linear gain for the volume, 0 … 1 for the proportion.
    required property real value
    /// Caption above the cap.
    required property string caption
    /// Diameter of the cap, taken from the master knob so the three cannot drift apart.
    required property real diameter

    implicitWidth: diameter
    implicitHeight: diameter
    objectName: "cueKnob_" + kind

    /// Full travel of the knob, in the units of its own value.
    readonly property real maximum: knob.kind === "phones" ? 2.0 : 1.0
    readonly property real share: Math.max(0.0, Math.min(1.0, knob.value / knob.maximum))
    // 270 degrees of travel: silence at the lower left, full at the lower right.
    readonly property real angle: -135.0 + 270.0 * knob.share

    property bool dragging: false
    property real pressShare: 0.0
    property real pressY: 0.0
    // A full sweep of the hand is 120 px, the same as the master's.
    readonly property real travelPixels: 120.0

    /// What the last gesture asked for, so a check can tell a dead knob from a refused command.
    property real lastShare: -1

    function ask(share) {
        const wanted = Math.max(0.0, Math.min(1.0, share));
        knob.lastShare = wanted;
        if (knob.kind === "phones") {
            Bridge.setCueGain(wanted * knob.maximum);
        } else {
            Bridge.setCueMix(wanted * knob.maximum);
        }
    }

    Rectangle {
        anchors.fill: parent
        radius: width / 2
        color: UiSettings.themeSurface
        border.width: 1
        border.color: UiSettings.themeBorder
    }

    // The pointer: from the centre to the rim, turned by the knob's angle.
    Rectangle {
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.bottom: parent.verticalCenter
        width: 2
        height: parent.height / 2 - 3
        color: UiSettings.themeText
        transformOrigin: Item.Bottom
        rotation: knob.share > 0.0 ? knob.angle + 90.0 : -135.0
    }

    MouseArea {
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onPressed: function (mouse) {
            knob.dragging = true;
            knob.pressShare = knob.share;
            knob.pressY = mouse.y;
        }
        onPositionChanged: function (mouse) {
            if (knob.dragging) {
                knob.ask(knob.pressShare + (knob.pressY - mouse.y) / knob.travelPixels);
            }
        }
        onReleased: knob.dragging = false
        onCanceled: knob.dragging = false
        // The neutral of both knobs is the top of their travel: headphones open, monitoring only.
        onDoubleClicked: knob.ask(0.5)
        onWheel: function (wheel) {
            const notches = Math.max(-3, Math.min(3, Math.round(wheel.angleDelta.y / 120)));
            if (notches !== 0) {
                knob.ask(knob.share + notches * 0.03);
            }
        }
    }
}
