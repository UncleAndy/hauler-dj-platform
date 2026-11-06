// The master output meter, in the middle of the mixer (DJP-MIX-001).
//
// Coloured by level — green, then yellow, then red — unlike the channel meters, which only speak for
// one deck. Green means headroom, yellow means the limiter is close, red means it is working: the eye
// finds the master among four identical strips by colour alone.
import QtQuick

Item {
    id: meter

    // Peak on the master bus, linear 0..1, as the engine publishes it.
    required property real peak

    objectName: "masterMeter"

    // It is a strip, not a line: below this it stops reading as a scale.
    implicitHeight: 40

    // Where the colours change. -6 dB is where a mix stops having room; -1 dB is where the limiter
    // starts to matter.
    readonly property real yellowFrom: 0.5
    readonly property real redFrom: 0.89

    Rectangle {
        anchors.fill: parent
        radius: 2
        color: UiSettings.themeBackground
        border.width: 1
        border.color: UiSettings.themeBorder
    }

    // The lit part: the whole scale, revealed up to the level, so the colours always sit at the same
    // heights and a glance reads the level by how far the colour reaches.
    Rectangle {
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        anchors.margins: 2
        height: Math.max(0.0, Math.min(1.0, meter.peak)) * (parent.height - 4)
        radius: 1
        gradient: Gradient {
            GradientStop { position: 0.00; color: UiSettings.themeMeterGreen }
            GradientStop { position: meter.yellowFrom; color: UiSettings.themeMeterGreen }
            GradientStop { position: meter.yellowFrom + 0.01; color: UiSettings.themeMeterYellow }
            GradientStop { position: meter.redFrom; color: UiSettings.themeMeterYellow }
            GradientStop { position: meter.redFrom + 0.01; color: UiSettings.themeMeterRed }
            GradientStop { position: 1.00; color: UiSettings.themeMeterRed }
        }
    }

    // The thresholds themselves, so the scale is visible even in silence.
    Rectangle {
        anchors.left: parent.left
        anchors.right: parent.right
        y: parent.height * (1.0 - meter.yellowFrom)
        height: 1
        color: UiSettings.themeBorder
    }
    Rectangle {
        anchors.left: parent.left
        anchors.right: parent.right
        y: parent.height * (1.0 - meter.redFrom)
        height: 1
        color: UiSettings.themeBorder
    }
}
