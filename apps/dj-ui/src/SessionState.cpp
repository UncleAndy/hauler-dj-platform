#include "SessionState.h"

#include <QJsonObject>
#include <QVariantMap>

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

bool SessionState::rememberCursor(const QString &path)
{
    if (path == m_cursor) {
        return false;
    }
    m_cursor = path;
    emit changed();
    return true;
}

bool SessionState::rememberCollectionCursor(const QString &path)
{
    if (path == m_collectionCursor) {
        return false;
    }
    m_collectionCursor = path;
    emit changed();
    return true;
}

bool SessionState::rememberRail(const QString &place)
{
    if (place == m_rail) {
        return false;
    }
    m_rail = place;
    emit changed();
    return true;
}

bool SessionState::rememberCrossfaderCurve(const QString &curve)
{
    if (curve.isEmpty() || curve == m_crossfaderCurve) {
        return false;
    }
    m_crossfaderCurve = curve;
    save();
    emit changed();
    return true;
}

bool SessionState::rememberDeckFx(const QString &deck, const QString &kind)
{
    if (deck.isEmpty() || kind.isEmpty() || m_deckFx.value(deck).toString() == kind) {
        return false;
    }
    m_deckFx.insert(deck, kind);
    save();
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
    m_cursor = root.value(QStringLiteral("cursor")).toString();
    m_collectionCursor = root.value(QStringLiteral("collection_cursor")).toString();
    m_rail = root.value(QStringLiteral("rail")).toString();
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
    const QString curve = root.value(QStringLiteral("crossfader_curve")).toString();
    if (!curve.isEmpty()) {
        any = rememberCrossfaderCurve(curve) || any;
    }
    const QJsonObject deckFx = root.value(QStringLiteral("deck_fx")).toObject();
    for (auto it = deckFx.constBegin(); it != deckFx.constEnd(); ++it) {
        const QString kind = it.value().toString();
        if (!kind.isEmpty()) {
            any = rememberDeckFx(it.key(), kind) || any;
        }
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
    root.insert(QStringLiteral("cursor"), m_cursor);
    root.insert(QStringLiteral("collection_cursor"), m_collectionCursor);
    root.insert(QStringLiteral("rail"), m_rail);
    root.insert(QStringLiteral("window"), m_window);
    root.insert(QStringLiteral("crossfader_curve"), m_crossfaderCurve);
    QJsonObject deckFx;
    for (auto it = m_deckFx.constBegin(); it != m_deckFx.constEnd(); ++it) {
        deckFx.insert(it.key(), it.value().toString());
    }
    root.insert(QStringLiteral("deck_fx"), deckFx);
    root.insert(QStringLiteral("restore_to"), m_restoreTo);
    QFile file(m_file);
    if (!file.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
        return false;
    }
    file.write(QJsonDocument(root).toJson(QJsonDocument::Indented));
    return true;
}
