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
    const QString folder = document.object().value(QStringLiteral("folder")).toString();
    if (folder.isEmpty()) {
        return false;
    }
    return rememberFolder(folder);
}

bool SessionState::save() const
{
    const QFileInfo info(m_file);
    if (!QDir().mkpath(info.absolutePath())) {
        return false;
    }
    QJsonObject root;
    root.insert(QStringLiteral("folder"), m_folder);
    QFile file(m_file);
    if (!file.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
        return false;
    }
    file.write(QJsonDocument(root).toJson(QJsonDocument::Indented));
    return true;
}
