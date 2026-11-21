#include "EngineSettings.h"

#include <QCoreApplication>
#include <QDir>
#include <QFileInfo>
#include <QProcess>
#include <QProcessEnvironment>
#include <QStandardPaths>
#include <QStringList>

EngineSettings::EngineSettings(QString file, QObject *parent)
    : QObject(parent)
    , m_file(std::move(file))
{
}

QString EngineSettings::file() const
{
    return resolvedFile();
}

QString EngineSettings::resolvedFile() const
{
    if (!m_file.isEmpty()) {
        return m_file;
    }
    if (!m_resolved.isEmpty()) {
        return m_resolved;
    }
    // The same rule the engine follows (`dj_paths::config_dir` + `hauler-dj/config.toml`): a shell that guessed
    // differently would edit a file nobody reads.
    QString base;
#if defined(Q_OS_WIN)
    base = qEnvironmentVariable("APPDATA");
    if (base.isEmpty()) {
        base = QStandardPaths::writableLocation(QStandardPaths::GenericConfigLocation);
    }
#else
    const QString xdg = qEnvironmentVariable("XDG_CONFIG_HOME");
    base = xdg.startsWith(QLatin1Char('/')) ? xdg : qEnvironmentVariable("HOME") + QStringLiteral("/.config");
#endif
    m_resolved = base + QStringLiteral("/hauler-dj/config.toml");
    return m_resolved;
}

QString EngineSettings::engineBinary() const
{
    const QString named = qEnvironmentVariable("HAULER_ENGINE");
    if (!named.isEmpty()) {
        return named;
    }
    const QString beside = QCoreApplication::applicationDirPath() + QStringLiteral("/dj-engine");
    if (QFileInfo::exists(beside)) {
        return beside;
    }
    const QString inPath = QStandardPaths::findExecutable(QStringLiteral("dj-engine"));
    return inPath;
}

QVariant EngineSettings::value(const QString &key) const
{
    const QString binary = engineBinary();
    if (binary.isEmpty()) {
        return {};
    }
    QProcess process;
    process.start(binary,
                  {QStringLiteral("--settings"), resolvedFile(), QStringLiteral("--get-config"), key});
    if (!process.waitForStarted(3000) || !process.waitForFinished(5000)) {
        return {};
    }
    if (process.exitStatus() != QProcess::NormalExit || process.exitCode() != 0) {
        return {};
    }
    QString text = QString::fromUtf8(process.readAllStandardOutput()).trimmed();
    if (text.isEmpty()) {
        return {};
    }
    if (text.startsWith(QLatin1Char('"')) && text.endsWith(QLatin1Char('"')) && text.size() >= 2) {
        return text.mid(1, text.size() - 2);
    }
    if (text == QStringLiteral("true") || text == QStringLiteral("false")) {
        return text == QStringLiteral("true");
    }
    bool ok = false;
    const double number = text.toDouble(&ok);
    if (ok) {
        return number;
    }
    return text;
}

bool EngineSettings::store(const QString &key, const QVariant &value)
{
    const QString binary = engineBinary();
    if (binary.isEmpty()) {
        return false;
    }
    // Numbers and switches go as TOML text; everything else is a string and goes through the string flag, where the
    // engine quotes it. The shell has no business knowing how TOML escapes a quote in a device name.
    QString flag = QStringLiteral("--set-config");
    QString literal;
    switch (value.typeId()) {
    case QMetaType::Bool:
        literal = value.toBool() ? QStringLiteral("true") : QStringLiteral("false");
        break;
    case QMetaType::Int:
    case QMetaType::LongLong:
    case QMetaType::UInt:
    case QMetaType::ULongLong:
        literal = QString::number(value.toLongLong());
        break;
    case QMetaType::Double:
        literal = QString::number(value.toDouble(), 'g', 17);
        break;
    default:
        flag = QStringLiteral("--set-config-string");
        literal = value.toString();
        break;
    }

    QProcess process;
    process.start(binary, {QStringLiteral("--settings"), resolvedFile(), flag, key + QStringLiteral("=") + literal});
    if (!process.waitForStarted(3000) || !process.waitForFinished(5000)) {
        return false;
    }
    if (process.exitStatus() != QProcess::NormalExit || process.exitCode() != 0) {
        return false;
    }
    emit changed();
    return true;
}

