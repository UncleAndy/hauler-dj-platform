// A button that says "no" out loud (DJP-UI-001).
//
// When an action is refused — a load into a deck that is playing — the button blinks red. It is not
// disabled: a dead button cannot tell anyone why nothing happened, and the press is exactly the moment
// the reason should be visible.
import QtQuick
import QtQuick.Controls

Button {
    id: control

    // How many refusals this button has signalled, so a check can read it.
    property int refusals: 0
    property bool flashing: false
    property bool flashOn: false
    property int pulses: 6

    // Signal a refusal: three red blinks, then back to normal.
    function refuse() {
        control.refusals += 1;
        control.flashOn = true;
        control.flashing = true;
        pulse.stop();
        pulse.ticks = 0;
        pulse.start();
    }

    Timer {
        id: pulse
        property int ticks: 0
        interval: 90
        repeat: true
        onTriggered: {
            pulse.ticks += 1;
            control.flashOn = !control.flashOn;
            if (pulse.ticks >= control.pulses) {
                pulse.stop();
                control.flashOn = false;
                control.flashing = false;
            }
        }
    }

    background: Rectangle {
        radius: 2
        color: control.flashOn ? UiSettings.themeDanger
                               : (control.down || control.checked ? UiSettings.themeAccentDim
                                                                  : UiSettings.themeRaised)
        border.color: control.flashOn ? UiSettings.themeDanger : UiSettings.themeBorder
        border.width: 1
    }
    contentItem: Text {
        text: control.text
        font: control.font
        color: control.flashOn ? UiSettings.themeBackground : UiSettings.themeText
        horizontalAlignment: Text.AlignHCenter
        verticalAlignment: Text.AlignVCenter
    }
}
