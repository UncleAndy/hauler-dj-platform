// The two headphone knobs in the middle of the mixer: the volume and the proportion between monitoring and the
// main mix (owner's decisions, 20.11).
//
// Both follow a law the mixer already has rather than inventing a third one:
//
//   * PHONES runs the master's scale — decibels, silence at the lower left, unity pointing right — and continues
//     past unity to +6 dB, so the far right mirrors the silence end. The number under the cap is the master's:
//     one decimal, with its sign.
//   * MIX runs a gain knob's scale — zero straight up, 135 degrees each way, the same curve as a trim — and its
//     number is the balance: −12 is monitoring only, 0 is even, +12 is the room only.
//
// Both are the master knob's size and sit under it, one above the other.

import QtQuick

Item {
    id: knob

    /// `phones` sends the volume, `mix` the proportion.
    required property string kind
    /// What the engine published: linear gain for the volume, 0 … 1 for the proportion.
    required property real value
    /// Caption above the cap.
    required property string caption
    /// Diameter of the cap: the master's, so the three read as one column.
    required property real diameter

    implicitWidth: diameter
    implicitHeight: diameter + 10
    objectName: "cueKnob_" + kind

    // --- The number the cap shows, in its own scale.
    readonly property int floorDb: -60
    readonly property int ceilingDb: 6
    readonly property real db: knob.kind === "phones"
                             ? (knob.value > 0.0 ? 20.0 * Math.log(knob.value) / Math.LN10 : knob.floorDb)
                             : 0.0
    /// The balance of the proportion knob: −12 monitoring only, 0 even, +12 the room.
    readonly property real balance: (0.5 - knob.value) * 24.0

    /// Angle of the pointer: the master's scale for the volume (225° up to unity, 45° beyond), a gain knob's
    /// 270° around the centre for the proportion.
    readonly property real angle: knob.kind === "phones" ? knob.phonesAngle() : knob.mixAngle()

    property bool dragging: false
    property real pressAngle: 0.0
    property real pressY: 0.0
    /// What the last gesture asked for, so a check can tell a dead knob from a refused command.
    property real lastValue: -1

    function phonesAngle() {
        const clamped = Math.max(knob.floorDb, Math.min(knob.ceilingDb, knob.db));
        if (clamped >= 0.0) {
            const share = UiSettings.gainCurvePosition(clamped / knob.ceilingDb);
            return 90.0 + 45.0 * share;
        }
        const share = UiSettings.gainCurvePosition(-clamped / -knob.floorDb);
        return 90.0 - 225.0 * share;
    }

    function mixAngle() {
        const clamped = Math.max(-12.0, Math.min(12.0, knob.balance));
        const share = UiSettings.gainCurvePosition(Math.abs(clamped) / 12.0);
        return clamped >= 0.0 ? 135.0 * share : -135.0 * share;
    }

    /// The value this knob stands for at a pointer angle.
    function valueAt(angle) {
        if (knob.kind === "mix") {
            const clamped = Math.max(-135.0, Math.min(135.0, angle));
            const share = UiSettings.gainCurveValue(Math.abs(clamped) / 135.0);
            const balance = clamped >= 0.0 ? 12.0 * share : -12.0 * share;
            return Math.max(0.0, Math.min(1.0, 0.5 - balance / 24.0));
        }
        const clamped = Math.max(-135.0, Math.min(135.0, angle));
        if (clamped >= 90.0) {
            const share = UiSettings.gainCurveValue((clamped - 90.0) / 45.0);
            return Math.pow(10.0, (share * knob.ceilingDb) / 20.0);
        }
        const share = UiSettings.gainCurveValue((90.0 - clamped) / 225.0);
        const db = -share * -knob.floorDb;
        return db <= knob.floorDb ? 0.0 : Math.pow(10.0, db / 20.0);
    }

    function ask(value) {
        const wanted = Math.max(0.0, Math.min(knob.kind === "phones" ? 2.0 : 1.0, value));
        knob.lastValue = wanted;
        if (knob.kind === "phones") {
            Bridge.setCueGain(wanted);
        } else {
            Bridge.setCueMix(wanted);
        }
    }

    /// The number under the cap, in the same style as the master's.
    function numberText() {
        if (knob.kind === "phones") {
            const db = knob.db;
            return (db >= 0.0 ? "+" : "") + db.toFixed(1);
        }
        const balance = knob.balance;
        return (balance >= 0.0 ? "+" : "") + balance.toFixed(1);
    }

    Item {
        id: face
        width: knob.diameter
        height: knob.diameter
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.top: parent.top

        Rectangle {
            anchors.fill: parent
            radius: width / 2
            color: UiSettings.themeSurface
            border.width: 1
            border.color: UiSettings.themeBorder
        }

        // The pointer: from the centre to the rim, turned by the law's angle.
        Rectangle {
            anchors.horizontalCenter: parent.horizontalCenter
            anchors.bottom: parent.verticalCenter
            width: 2
            height: parent.height / 2 - 3
            color: UiSettings.themeText
            transformOrigin: Item.Bottom
            rotation: knob.angle + 90.0
        }
    }

    Text {
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.top: face.bottom
        text: knob.numberText()
        color: UiSettings.themeTextDim
        font.pixelSize: 8
    }

    MouseArea {
        anchors.fill: face
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onPressed: function (mouse) {
            knob.dragging = true;
            knob.pressAngle = knob.angle;
            knob.pressY = mouse.y;
        }
        onPositionChanged: function (mouse) {
            if (!knob.dragging) {
                return;
            }
            // Turning: a vertical drag of the cap's width is 200 degrees, the rate a hand expects of a knob.
            const degrees = (knob.pressY - mouse.y) * (200.0 / Math.max(1.0, knob.diameter));
            knob.ask(knob.valueAt(knob.pressAngle + degrees));
        }
        onReleased: knob.dragging = false
        onCanceled: knob.dragging = false
        // The neutral of both knobs: headphones at unity, the proportion even.
        onDoubleClicked: knob.ask(knob.kind === "phones" ? 1.0 : 0.5)
        onWheel: function (wheel) {
            const notches = Math.max(-3, Math.min(3, Math.round(wheel.angleDelta.y / 120)));
            if (notches !== 0) {
                knob.ask(knob.valueAt(knob.angle + notches * 6.0));
            }
        }
    }
}
