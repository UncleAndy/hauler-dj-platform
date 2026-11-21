# Qt 6 / QML shell. Owns no domain logic: it renders the engine snapshot and
# forwards user intent as commands (DJP-UI-001, DJP-ARCH-001, ADR-003).
find_package(Qt6 6.8 REQUIRED COMPONENTS Quick Network)


qt_standard_project_setup(REQUIRES 6.8)

qt_add_executable(dj-ui
    src/main.cpp
    src/BridgeClient.cpp
    src/CrashLog.cpp
    src/checks/PathChecks.cpp
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
    src/GainCurve.cpp
    src/GainCurve.h
    src/LayoutSettings.cpp
    src/LayoutSettings.h
    src/SessionState.cpp
    src/SessionState.h
    src/checks/Checks.cpp
    src/checks/Checks.h
    src/checks/EngineChecks.cpp
    src/checks/DeckViewChecks.cpp
        src/checks/CueChecks.cpp
    src/checks/FxChecks.cpp
    src/EngineSettings.cpp
    src/EngineSettings.h
    src/SettingsModel.cpp
    src/SettingsModel.h
    src/checks/BrowserChecks.cpp
    src/checks/LibraryChecks.cpp
    src/checks/PlaylistChecks.cpp
    src/checks/LockChecks.cpp
    src/checks/ReconnectCheck.cpp
)

qt_add_qml_module(dj-ui
    URI DjPlatform.Ui
    VERSION 0.1
    QML_FILES
        qml/Main.qml
        qml/shell/ShellLayout.qml
        qml/shell/GlobalBar.qml
        qml/shell/ModelPanel.qml
        qml/browser/BrowserPanel.qml
        qml/browser/LockGlyph.qml
        qml/decks/DeckPanel.qml
        qml/decks/ToggleButton.qml
        qml/decks/GridTool.qml
        qml/controls/MenuEntry.qml
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
        qml/mixer/HeadphoneGlyph.qml
        qml/mixer/PitchFader.qml
        qml/mixer/RefusalButton.qml
)

# The knob's law is one of Signalsmith's curve primitives (`crates/dj-signalsmith-sys/vendor`), the same
# vendored copy the audio side builds against — one licence record, one version.
target_include_directories(dj-ui PRIVATE
    # The checks live one folder down and include the shell's headers by name, the way main.cpp does.
    ${CMAKE_CURRENT_SOURCE_DIR}/src
    ${CMAKE_CURRENT_SOURCE_DIR}/../../crates/dj-signalsmith-sys/vendor)

target_link_libraries(dj-ui PRIVATE Qt6::Quick Qt6::Network)

# A format string is checked by the compiler or by nobody: a `%.9` that lost its `f` printed garbage on
# the screen and the build said nothing. Warnings are on, and a broken format is an error, because that
# defect shows up as nonsense in front of a person rather than as a hint in a log.
target_compile_options(dj-ui PRIVATE -Wall -Wextra -Werror=format)


# WIN32_EXECUTABLE ON is the Windows GUI subsystem: with it OFF the shell was a console program, and Windows gave
# it a console window on every start — the "terminal" next to the player (owner's report, 2026-11-20). CMake
# ignores this property outside Windows, and a build started from cmd still prints into that cmd's console.
# The window and taskbar icon: the same drawing at four sizes, taken from the owner's PNGs. It lives in the
# binary, so the shell shows it without any file next to it.
qt_add_resources(dj-ui "appicons"
    PREFIX "/icons"
    BASE "../../assets/icons"
    FILES
        ../../assets/icons/hauler-dj-32.png
        ../../assets/icons/hauler-dj-64.png
        ../../assets/icons/hauler-dj-96.png
        ../../assets/icons/hauler-dj-128.png
)

set_target_properties(dj-ui PROPERTIES
    WIN32_EXECUTABLE ON
    MACOSX_BUNDLE OFF
)

if (WIN32)
    # The fault writer names the module of every frame; dbghelp is what reads the module table.
    target_link_libraries(dj-ui PRIVATE dbghelp)
endif ()
