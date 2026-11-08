// The pitch fader: how fast this deck plays (DJP-DECK-002).
//
// A tempo change is a rate, not a level, so zero stands in the middle: "faster" and "slower" are read
// from which way the handle sits off centre, not from a number. The key stays where the file put it —
// key lock is on by default (DJP-DECK-008) — so the fader moves the material and not the pitch.
import QtQuick
import QtQuick.Controls

Item {
    id: fader

    required property string deckId
    /// The deck's rate as the engine reports it: 1.0 is the file's own speed.
    required property real ratio

    objectName: "pitch_" + deckId

    /// The throw in percent either side of the file's own speed: a setting (`UiSettings`), overridable
    /// per deck. The owner's default is ±16 %; the settings store will drive it when it arrives.
    property real rangePercent: UiSettings.pitchRangePercent
    readonly property real range: fader.rangePercent / 100.0
    readonly property real minimum: 1.0 - fader.range
    readonly property real maximum: 1.0 + fader.range
    readonly property real shown: fader.dragging ? fader.dragged : fader.ratio
    /// Where the handle stands from the top: the top of the throw is the fastest rate.
    readonly property real fraction: (fader.maximum - fader.shown) / (fader.maximum - fader.minimum)
    readonly property real unityFraction: (fader.maximum - 1.0) / (fader.maximum - fader.minimum)
    readonly property int inset: 6
    /// Rate change per pixel: the whole throw is the whole range.
    readonly property real ratioPerPixel: (fader.maximum - fader.minimum)
                                           / Math.max(1, fader.height - 2 * fader.inset)
    /// What the last gesture asked for, so a check can tell a dead fader from a refused command.
    property real lastRatio: -1.0
    property bool dragging: false
    property real dragged: 1.0
    property real pressY: 0.0
    property real pressRatio: 1.0

    implicitWidth: 26
    implicitHeight: 96

    function clamp(ratio) {
        return Math.max(fader.minimum, Math.min(fader.maximum, ratio));
    }

    function ask(ratio) {
        const wanted = fader.clamp(ratio);
        fader.lastRatio = wanted;
        Bridge.setTempo(fader.deckId, wanted);
    }

    // The throw.
    Rectangle {
        id: track
        anchors.horizontalCenter: parent.horizontalCenter
        y: fader.inset
        width: 2
        height: Math.max(1, fader.height - 2 * fader.inset)
        color: UiSettings.themeBorder
    }

    // Zero: the file's own speed, marked so the middle of the fader means something.
    Rectangle {
        anchors.horizontalCenter: parent.horizontalCenter
        y: fader.inset + track.height * fader.unityFraction - 1
        width: parent.width * 0.6
        height: 2
        color: Math.abs(fader.shown - 1.0) < 0.0005 ? UiSettings.themeAccent : UiSettings.themeBorder
    }

    Rectangle {
        objectName: "pitchHandle_" + fader.deckId
        anchors.horizontalCenter: parent.horizontalCenter
        y: fader.inset + track.height * fader.fraction - height / 2
        width: Math.max(12, fader.width * 0.75)
        height: 7
        radius: 2
        color: UiSettings.themeText
        border.width: 1
        border.color: UiSettings.themeBackground
    }

    Label {
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.top: track.bottom
        anchors.topMargin: 2
        text: ((fader.shown - 1.0) * 100.0).toFixed(1) + "%"
        font.pixelSize: 8
        color: Math.abs(fader.shown - 1.0) < 0.0005 ? UiSettings.themeTextDim : UiSettings.themeText
    }

    MouseArea {
        anchors.fill: parent
        cursorShape: Qt.SizeVerCursor
        onPressed: function (mouse) {
            // The gesture starts from the rate the engine is playing, not from the last drag's value:
            // setting `dragging` first makes `shown` report the stale `dragged`, so a second gesture
            // used to begin where the previous one ended instead of where the deck actually is.
            fader.pressRatio = fader.ratio;
            fader.pressY = mouse.y;
            fader.dragged = fader.pressRatio;
            fader.dragging = true;
        }
        onPositionChanged: function (mouse) {
            if (!fader.dragging) {
                return;
            }
            // The hand moves the handle: up is faster, down is slower, and the rate follows the hand.
            fader.dragged = fader.clamp(fader.pressRatio + (fader.pressY - mouse.y)
                                                        * fader.ratioPerPixel);
            fader.ask(fader.dragged);
        }
        onReleased: fader.dragging = false
        onCanceled: fader.dragging = false
        // A double click returns the deck to the file's own speed.
        onDoubleClicked: fader.ask(1.0)
        onWheel: function (wheel) {
            const notches = Math.max(-3, Math.min(3, Math.round(wheel.angleDelta.y / 120)));
            if (notches !== 0) {
                fader.ask(fader.shown + notches * 0.001);
            }
        }
    }
}
