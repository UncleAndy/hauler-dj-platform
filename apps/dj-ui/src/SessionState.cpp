#include "SessionState.h"

#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QJsonDocument>
#include <QJsonObject>
#include <QStandardPaths>

SessionState::SessionState(QObject *parent)
    : QObject(parent)
    , m_file(defaultFile())
{
}

QString SessionState::defaultFile()
{
    const QString base = QStandardPaths::writableLocation(QStandardPaths::GenericConfigLocation);
    const QString directory = base.isEmpty() ? QDir::homePath() + QStringLiteral("/.config") : base;
    return directory + QStringLiteral("/hauler-dj/session.json");
}

void SessionState::setFile(const QString &path)
{
    if (path == m_file) {
        return;
    }
    m_file = path;
    emit changed();
}

bool SessionState::rememberFolder(const QString &path)
{
    const QString absolute = QFileInfo(path).absoluteFilePath();
    // A folder that has been deleted or renamed would open an empty browser; fall back instead.
    if (absolute.isEmpty() || !QFileInfo::exists(absolute) || !QFileInfo(absolute).isDir()) {
        return false;
    }
    if (absolute == m_folder) {
        return true;
    }
    m_folder = absolute;
    emit changed();
    return true;
}

bool SessionState::rememberWindow(const QString &state)
{
    if (state != QLatin1String("window") && state != QLatin1String("maximized")
        && state != QLatin1String("fullscreen")) {
        return false;
    }
    if (state == m_window) {
        return true;
    }
    m_window = state;
    emit changed();
    return true;
}

bool SessionState::rememberRestoreTo(const QString &state)
{
    if (state != QLatin1String("window") && state != QLatin1String("maximized")) {
        return false;
    }
    if (state == m_restoreTo) {
        return true;
    }
    m_restoreTo = state;
    emit changed();
    return true;
}

bool SessionState::load()
{
    QFile file(m_file);
    if (!file.open(QIODevice::ReadOnly)) {
        return false;
    }
    const QJsonDocument document = QJsonDocument::fromJson(file.readAll());
    if (!document.isObject()) {
        return false;
    }
    // The two facts are independent: a remembered window shape is still worth having when the folder
    // has been deleted, and the other way round.
    bool any = false;
    const QJsonObject root = document.object();
    const QString folder = root.value(QStringLiteral("folder")).toString();
    if (!folder.isEmpty()) {
        any = rememberFolder(folder) || any;
    }
    const QString window = root.value(QStringLiteral("window")).toString();
    if (!window.isEmpty()) {
        any = rememberWindow(window) || any;
    }
    const QString restoreTo = root.value(QStringLiteral("restore_to")).toString();
    if (!restoreTo.isEmpty()) {
        any = rememberRestoreTo(restoreTo) || any;
    }
    return any;
}

bool SessionState::save() const
{
    const QFileInfo info(m_file);
    if (!QDir().mkpath(info.absolutePath())) {
        return false;
    }
    QJsonObject root;
    root.insert(QStringLiteral("folder"), m_folder);
    root.insert(QStringLiteral("window"), m_window);
    root.insert(QStringLiteral("restore_to"), m_restoreTo);
    QFile file(m_file);
    if (!file.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
        return false;
    }
    file.write(QJsonDocument(root).toJson(QJsonDocument::Indented));
    return true;
}
