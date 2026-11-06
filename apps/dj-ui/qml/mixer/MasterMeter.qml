// The master output meter, in the middle of the mixer (DJP-MIX-001).
//
// Coloured by level — green, then yellow, then red — unlike the channel meters, which only speak for
// one deck. The meaning is the one a master meter has: green anywhere there is room, yellow when the
// mix is close to 0 dB, red when it is at or past 0 dB and the limiter is the only thing holding it.
//
// The scale is decibels, not amplitude: with a linear height the whole yellow band would sit in the
// last two pixels of a 28 px strip and red would never appear at all. The channel meters stay
// linear, because a channel's job is to show what it contributes; the master's job is to show how
// much room is left.
import QtQuick

Item {
    id: meter

    // Peak on the master bus, linear 0..1, as the engine publishes it.
    required property real peak

    objectName: "masterMeter"

    // It is a strip, not a line: below this it stops reading as a scale.
    implicitHeight: 40

    // The scale: silence at the foot, full scale at the top edge. 0 dB has to be the top edge —
    // with headroom above it the loudest the master can be left the top of the meter dark, and a
    // meter that never fills is a meter nobody trusts.
    readonly property real floorDb: -40.0
    readonly property real ceilingDb: 0.0
    // Where the colour changes: yellow for the last six decibels, red for the last three.
    readonly property real yellowFromDb: -6.0
    readonly property real redFromDb: -3.0

    // The same thresholds as fractions of the scale, for drawing.
    readonly property real yellowFrom: fractionForDb(yellowFromDb)
    readonly property real redFrom: fractionForDb(redFromDb)

    // What the meter says right now, so a check can ask rather than guess.
    readonly property string zone: zoneFor(peak)

    function fractionForDb(db) {
        return Math.max(0.0, Math.min(1.0, (db - meter.floorDb) / (meter.ceilingDb - meter.floorDb)));
    }

    function dbFor(linear) {
        return linear > 0.0 ? 20.0 * Math.log(linear) / Math.LN10 : meter.floorDb;
    }

    function zoneFor(linear) {
        const db = meter.dbFor(linear);
        if (db >= meter.redFromDb) {
            return "red";
        }
        return db >= meter.yellowFromDb ? "yellow" : "green";
    }

    Rectangle {
        anchors.fill: parent
        radius: 2
        color: UiSettings.themeBackground
        border.width: 1
        border.color: UiSettings.themeBorder
    }

    // The revealed part of a fixed scale: the colours sit at their own heights, so the top of the bar
    // is the colour of the level it reached, not the top of a gradient squeezed into the bar.
    Item {
        id: lit

        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        anchors.margins: 2
        height: meter.fractionForDb(meter.dbFor(meter.peak)) * (meter.height - 4)
        clip: true

        Rectangle {
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.bottom: parent.bottom
            height: meter.height - 4
            radius: 1
            // A gradient runs from the top down, while the thresholds are measured from the bottom:
            // red sits at the top edge, yellow below it, green fills the rest. Getting this backwards
            // put the red block at the foot of the meter, where silence lives.
            gradient: Gradient {
                GradientStop { position: 0.00; color: UiSettings.themeMeterRed }
                GradientStop { position: 1.0 - meter.redFrom; color: UiSettings.themeMeterRed }
                GradientStop { position: 1.0 - meter.redFrom + 0.001; color: UiSettings.themeMeterYellow }
                GradientStop { position: 1.0 - meter.yellowFrom; color: UiSettings.themeMeterYellow }
                GradientStop { position: 1.0 - meter.yellowFrom + 0.001; color: UiSettings.themeMeterGreen }
                GradientStop { position: 1.00; color: UiSettings.themeMeterGreen }
            }
        }
    }

    // The thresholds themselves, so the scale is readable in silence too.
    Rectangle {
        anchors.left: parent.left
        anchors.right: parent.right
        y: (parent.height - 4) * (1.0 - meter.yellowFrom) + 2
        height: 1
        color: UiSettings.themeBorder
    }
    Rectangle {
        anchors.left: parent.left
        anchors.right: parent.right
        y: (parent.height - 4) * (1.0 - meter.redFrom) + 2
        height: 1
        color: UiSettings.themeBorder
    }
}
