# Qt 6 / QML shell. Owns no domain logic: it renders the engine snapshot and
# forwards user intent as commands (DJP-UI-001, DJP-ARCH-001, ADR-003).
find_package(Qt6 6.8 REQUIRED COMPONENTS Quick Network)

qt_standard_project_setup(REQUIRES 6.8)

qt_add_executable(dj-ui
    src/main.cpp
    src/BridgeClient.cpp
    src/BridgeClient.h
    src/Waveform.cpp
    src/Waveform.h
    src/WaveformItem.cpp
    src/WaveformItem.h
    src/WaveformPainter.cpp
    src/WaveformPainter.h
    src/Scenarios.cpp
    src/Scenarios.h
    src/UiSettings.cpp
    src/UiSettings.h
    src/LayoutSettings.cpp
    src/LayoutSettings.h
    src/SessionState.cpp
    src/SessionState.h
)

qt_add_qml_module(dj-ui
    URI DjPlatform.Ui
    VERSION 0.1
    QML_FILES
        qml/Main.qml
        qml/shell/ShellLayout.qml
        qml/shell/GlobalBar.qml
        qml/browser/BrowserPanel.qml
        qml/decks/DeckPanel.qml
        qml/views/WaveformView.qml
        qml/views/TrackOverview.qml
        qml/mixer/Crossfader.qml
        qml/mixer/CurveGlyph.qml
        qml/mixer/MasterKnob.qml
        qml/mixer/MasterMeter.qml
        qml/mixer/MixerPanel.qml
        qml/mixer/TrimKnob.qml
        qml/mixer/FaderKnob.qml
        qml/mixer/EqKnob.qml
        qml/mixer/LevelMeter.qml
        qml/mixer/PitchFader.qml
        qml/mixer/RefusalButton.qml
)

target_link_libraries(dj-ui PRIVATE Qt6::Quick Qt6::Network)

set_target_properties(dj-ui PROPERTIES
    WIN32_EXECUTABLE OFF
    MACOSX_BUNDLE OFF
)
