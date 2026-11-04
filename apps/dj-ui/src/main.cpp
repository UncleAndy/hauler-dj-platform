// DJ Platform UI shell.
//
// M0 scaffold: creates the QML engine, loads the blank two-deck scene and can
// verify itself without a display (`--smoke`). The command/state bridge to the
// audio engine is A06 work; nothing here touches DSP (DJP-ARCH-001).

#include <QCommandLineOption>
#include <QCommandLineParser>
#include <QGuiApplication>
#include <QQmlApplicationEngine>
#include <QString>

#include <cstdio>

namespace {

int g_warningCount = 0;

void collectMessages(QtMsgType type, const QMessageLogContext &context, const QString &message)
{
    if (type != QtDebugMsg && type != QtInfoMsg) {
        ++g_warningCount;
    }
    const char *category = context.category != nullptr ? context.category : "default";
    std::fprintf(stderr, "qt.%s: %s\n", category, qPrintable(message));
}

} // namespace

int main(int argc, char *argv[])
{
    qInstallMessageHandler(collectMessages);

    QGuiApplication app(argc, argv);
    QCoreApplication::setApplicationName(QStringLiteral("dj-platform"));
    QCoreApplication::setApplicationVersion(QStringLiteral("0.1.0"));
    QCoreApplication::setOrganizationName(QStringLiteral("DJ Platform"));

    QCommandLineParser parser;
    parser.setApplicationDescription(
        QStringLiteral("DJ Platform UI shell (M0 scaffold; the audio engine is not wired yet)."));
    parser.addHelpOption();
    parser.addVersionOption();
    const QCommandLineOption smokeOption(
        QStringLiteral("smoke"),
        QStringLiteral("Load the QML scene, report Qt warnings, then exit without a window."));
    parser.addOption(smokeOption);
    parser.process(app);

    QQmlApplicationEngine engine;
    engine.loadFromModule(QStringLiteral("DjPlatform.Ui"), QStringLiteral("Main"));

    if (engine.rootObjects().isEmpty()) {
        std::fprintf(stderr, "dj-ui: the QML scene did not load\n");
        return 1;
    }

    if (parser.isSet(smokeOption)) {
        if (g_warningCount != 0) {
            std::fprintf(stderr, "dj-ui: %d Qt warning(s) while loading the scene\n", g_warningCount);
            return 1;
        }
        std::fprintf(stdout, "dj-ui: QML scene loaded, no Qt warnings\n");
        return 0;
    }

    return app.exec();
}
