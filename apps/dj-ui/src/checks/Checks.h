// Проверки оболочки, вынесенные из main.cpp.
//
// Каждая проверка — отдельный прогон: она поднимает окно, жмёт то, что жмёт DJ, и требует от
// движка ответа. Живут они здесь, а не в main.cpp, потому что main вырос до размера, при котором
// компилятор отказывается от отладочной информации, а читать файл на пятнадцать тысяч строк —
// работа на полдня вместо минуты.

#pragma once

#include "BridgeClient.h"
#include "Scenarios.h"
#include "UiSettings.h"
#include "LayoutSettings.h"
#include "SessionState.h"
#include "Waveform.h"
#include "WaveformItem.h"
#include "WaveformPainter.h"

#include <QCommandLineOption>
#include <QCommandLineParser>
#include <QElapsedTimer>
#include <QFile>
#include <QFileInfo>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QKeyEvent>
#include <QWheelEvent>
#include <QEventLoop>
#include <QGuiApplication>
#include <QPalette>
#include <QDir>
#include <QDirIterator>
#include <QThread>
#include <QMap>
#include <QTemporaryDir>
#include <QImage>
#include <algorithm>
#include <QQuickWindow>
#include <QLinearGradient>
#include <QPainter>
#include <QQmlApplicationEngine>
#include <QQmlComponent>
#include <QQmlEngine>
#include <QQmlContext>
#include <QString>
#include <QQuickItemGrabResult>
#include <QSharedPointer>
#include <QTimer>

#include <cstdio>
#include <functional>
#include <memory>

namespace checks {

/// QML items created by a Repeater are not always in the window's QObject tree, so a search by name
/// has to walk the visual tree — the one the user actually sees.
QQuickItem* findItem(QQuickItem* root, const QString& name);

int measureDeckRows(QQuickWindow* window, const QString& deck);
bool writeClickTrack(const QString& path, double bpm, double seconds);
int measureCollectionHeader(QQuickItem* panel);

QVariantMap deckById(const QVariantMap& snapshot, const QString& deck);
bool deckHasHotcue(const QVariantMap& snapshot, const QString& deck, int slot);
bool deckIsPlaying(const QVariantMap& snapshot, const QString& deck);

int runBusyCheck(BridgeClient& bridge, const QString& socketPath, const QString& track,
                 const QString& other, int timeoutMs);
int runHistoryCheck(BridgeClient& bridge, const QString& socketPath, const QString& track,
                    int timeoutMs);
int runGainCheck(BridgeClient& bridge, const QString& socketPath, int timeoutMs);
int runSelfCheck(BridgeClient& bridge, Waveform& waveform, const QString& socketPath, int timeoutMs,
                 const QString& loadPath, const QString& loadSecondPath, bool expectSpread);
int runBrowserCheck(QQmlApplicationEngine& engine, FileBrowser& browser, LayoutSettings& layout,
                    UiSettings& settings);
int runSyncCheck(QQmlApplicationEngine& engine, std::unique_ptr<BridgeClient>& bridge,
                 QCommandLineParser& parser, const QCommandLineOption& connectOption,
                 const QCommandLineOption& loadOption, const QCommandLineOption& loadSecondOption);
int runSyncSoak(QQmlApplicationEngine& engine, std::unique_ptr<BridgeClient>& bridge,
                QCommandLineParser& parser, const QCommandLineOption& connectOption,
                const QCommandLineOption& soakSecondsOption);
int runLoop(QQmlApplicationEngine& engine, std::unique_ptr<BridgeClient>& bridge,
            QCommandLineParser& parser, const QCommandLineOption& connectOption,
            const QCommandLineOption& loadOption);
int runEqCheck(QQmlApplicationEngine& engine, std::unique_ptr<BridgeClient>& bridge,
               QCommandLineParser& parser, const QCommandLineOption& connectOption);
int runGridLines(QQmlApplicationEngine& engine, std::unique_ptr<BridgeClient>& bridge,
                 QCommandLineParser& parser, const QCommandLineOption& connectOption,
                 const QCommandLineOption& loadOption);
int runLayoutCheck(QQmlApplicationEngine& engine, LayoutSettings& layout, UiSettings& settings);
int runSortCheck(QQmlApplicationEngine& engine, FileBrowser& browser, LayoutSettings& layout,
                 const QString& fixture, const QString& collectionRoot);
int runCollectionProbe(FileBrowser& browser, const QString& root);
bool holdsName(const QVariant& value, const QString& name);
int runCollectionLoadCheck(QQmlApplicationEngine& engine, FileBrowser& browser,
                           BridgeClient& bridge, const QString& collectionRoot);
int runAnalyzeCheck(QQmlApplicationEngine& engine, FileBrowser& browser, LayoutSettings& layout,
                    BridgeClient& bridge, const QString& fixture, const QString& collectionRoot);
int runCollectionPageCheck(QQmlApplicationEngine& engine, FileBrowser& browser,
                           BridgeClient& bridge, const QString& fixture,
                           const QString& collectionRoot);
int runImportCheck(QQmlApplicationEngine& engine, FileBrowser& browser, LayoutSettings& layout,
                   BridgeClient& bridge, const QString& fixture, const QString& collectionRoot);
int runPlaylistCheck(QQmlApplicationEngine& engine, FileBrowser& browser, BridgeClient& bridge,
                     const QString& fixture, const QString& secondFixture,
                     const QString& collectionRoot);
bool writeShortWav(const QString& path, double seconds);
int runScrollCheck(QQmlApplicationEngine& engine, FileBrowser& browser, BridgeClient& bridge,
                   const QString& collectionRoot);
int runLockCheck(QQmlApplicationEngine& engine, FileBrowser& browser, BridgeClient& bridge,
                 const QString& socketPath,
                 const QString& fixture, const QString& collectionRoot);
int runEndCheck(QQmlApplicationEngine& engine, BridgeClient& bridge, const QString& fixture);
int runGenreCheck(QQmlApplicationEngine& engine, FileBrowser& browser, LayoutSettings& layout,
                  const QString& fixture, const QString& collectionRoot);

int runTransportCheck(QQmlApplicationEngine& engine, BridgeClient* bridge, UiSettings& settings,
                      QCommandLineParser& parser, const QCommandLineOption& loadOption,
                      const QCommandLineOption& connectOption);
;

int runPitchCheck(QQmlApplicationEngine& engine, BridgeClient* bridge, QCommandLineParser& parser,
                  const QCommandLineOption& loadOption, const QCommandLineOption& connectOption);
;

int runGridCheck(QQmlApplicationEngine& engine, BridgeClient* bridge, QCommandLineParser& parser,
                 const QCommandLineOption& loadOption, const QCommandLineOption& connectOption);
;

int runLoopBeatsCheck(QQmlApplicationEngine& engine, BridgeClient* bridge,
                      QCommandLineParser& parser, const QCommandLineOption& loadOption,
                      const QCommandLineOption& connectOption);
;

int runGridCreateCheck(QQmlApplicationEngine& engine, BridgeClient* bridge,
                       QCommandLineParser& parser, const QCommandLineOption& loadOption,
                       const QCommandLineOption& loadSecondOption,
                       const QCommandLineOption& connectOption);
;

int runFourDeckCheck(QQmlApplicationEngine& engine, BridgeClient* bridge,
                     QCommandLineParser& parser, const QCommandLineOption& loadOption,
                     const QCommandLineOption& connectOption);
;

int runMasterCheck(QQmlApplicationEngine& engine, BridgeClient* bridge, UiSettings& settings,
                   QCommandLineParser& parser, const QCommandLineOption& loadOption,
                   const QCommandLineOption& connectOption);
;

int runTransportExtrasCheck(QQmlApplicationEngine& engine, BridgeClient* bridge,
                            QCommandLineParser& parser, const QCommandLineOption& loadOption,
                            const QCommandLineOption& connectOption);
;

int runPadsCheck(QQmlApplicationEngine& engine, BridgeClient* bridge, QCommandLineParser& parser,
                 const QCommandLineOption& loadOption, const QCommandLineOption& connectOption);
;

int runMetronomeCheck(QQmlApplicationEngine& engine, BridgeClient* bridge,
                      QCommandLineParser& parser, const QCommandLineOption& loadOption,
                      const QCommandLineOption& connectOption);
;

int runScratchCheck(QQmlApplicationEngine& engine, BridgeClient* bridge, QCommandLineParser& parser,
                    const QCommandLineOption& loadOption, const QCommandLineOption& connectOption);
;

int runTempoRecheckCheck(QQmlApplicationEngine& engine, BridgeClient* bridge,
                         QCommandLineParser& parser, const QCommandLineOption& loadOption,
                         const QCommandLineOption& connectOption);
;

int runGridHereCheck(QQmlApplicationEngine& engine, BridgeClient* bridge,
                     QCommandLineParser& parser, const QCommandLineOption& loadOption,
                     const QCommandLineOption& connectOption);
;

int runRefusalCheck(QQmlApplicationEngine& engine, FileBrowser& browser, BridgeClient* bridge,
                    UiSettings& settings, QCommandLineParser& parser,
                    const QCommandLineOption& loadOption, const QCommandLineOption& connectOption);
;

int runGainCurveCheck(QQmlApplicationEngine& engine, UiSettings& settings);
;

int runCurveCheck(QQmlApplicationEngine& engine, BridgeClient* bridge, QCommandLineParser& parser,
                  const QCommandLineOption& connectOption);
;

int runWheelCheck(QQmlApplicationEngine& engine, BridgeClient* bridge, QCommandLineParser& parser,
                  const QCommandLineOption& connectOption);
;

int runSpaceCheck(QQmlApplicationEngine& engine, BridgeClient* bridge, QCommandLineParser& parser,
                  const QCommandLineOption& loadOption, const QCommandLineOption& connectOption);
;

int runNeutralCheck(QQmlApplicationEngine& engine, BridgeClient* bridge, QCommandLineParser& parser,
                    const QCommandLineOption& connectOption);
;

int runGridReconnectCheck(QQmlApplicationEngine& engine, BridgeClient* bridge,
                          QCommandLineParser& parser, const QCommandLineOption& connectOption);
;

int runTempoCheck(QQmlApplicationEngine& engine, BridgeClient* bridge, QCommandLineParser& parser,
                  const QCommandLineOption& loadOption, const QCommandLineOption& connectOption);
;

int runBrowserRestoreCheck(QQmlApplicationEngine& engine, LayoutSettings& layout,
                           UiSettings& settings);
;

int runStripCheck(QQmlApplicationEngine& engine, BridgeClient* bridge, QCommandLineParser& parser,
                  const QCommandLineOption& loadOption, const QCommandLineOption& connectOption);
;

int runScenario(QQmlApplicationEngine& engine, BridgeClient* bridge, QCommandLineParser& parser,
                const QCommandLineOption& loadOption, const QCommandLineOption& connectOption);
;

int runLayoutNumbersCheck(LayoutSettings& layout, const QString& file);

int runLayoutMenuCheck(QQmlApplicationEngine& engine, FileBrowser& browser, LayoutSettings& layout);

} // namespace checks
