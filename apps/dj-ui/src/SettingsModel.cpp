#include "SettingsModel.h"

#include "EngineSettings.h"
#include "LayoutSettings.h"
#include "SessionState.h"
#include "UiSettings.h"

#include <QCoreApplication>
#include <QMetaObject>
#include <QPixmap>
#include <QStandardPaths>
#include <QStringList>

SettingsModel::SettingsModel(LayoutSettings &layout, UiSettings &ui, SessionState &session,
                             EngineSettings &engine, QObject *parent)
    : QObject(parent)
    , m_layout(layout)
    , m_ui(ui)
    , m_session(session)
    , m_engine(engine)
{
}

QObject *SettingsModel::ownerOf(const QString &path, QString *name) const
{
    const int colon = path.indexOf(QLatin1Char(':'));
    if (colon <= 0) {
        return nullptr;
    }
    const QString prefix = path.left(colon);
    const QString rest = path.mid(colon + 1);
    *name = rest;
    if (prefix == QStringLiteral("layout")) {
        return &m_layout;
    }
    if (prefix == QStringLiteral("ui")) {
        return &m_ui;
    }
    if (prefix == QStringLiteral("session")) {
        return &m_session;
    }
    return nullptr;
}

QVariant SettingsModel::value(const QString &path) const
{
    if (path.startsWith(QStringLiteral("engine:"))) {
        return m_engine.value(path.mid(7));
    }
    if (path == QStringLiteral("session:deckFx")) {
        return m_session.deckFx();
    }
    QString name;
    QObject *owner = ownerOf(path, &name);
    if (owner == nullptr) {
        return {};
    }
    return owner->property(name.toUtf8().constData());
}

bool SettingsModel::set(const QString &path, const QVariant &value)
{
    if (path.startsWith(QStringLiteral("engine:"))) {
        const bool stored = m_engine.store(path.mid(7), value);
        if (stored) {
            emit changed();
        }
        return stored;
    }
    // The deck's effect is remembered by name, not as a property: the session stores a map.
    if (path.startsWith(QStringLiteral("session:deckFx."))) {
        const bool stored = m_session.rememberDeckFx(path.mid(16), value.toString());
        if (stored) {
            emit changed();
        }
        return stored;
    }
    QString name;
    QObject *owner = ownerOf(path, &name);
    if (owner == nullptr) {
        return false;
    }
    const QMetaObject *meta = owner->metaObject();
    const int index = meta->indexOfProperty(name.toUtf8().constData());
    if (index < 0) {
        return false;
    }
    const bool written = owner->setProperty(name.toUtf8().constData(), value);
    if (written) {
        emit changed();
    }
    return written;
}

QString SettingsModel::deckFx(const QString &deck) const
{
    return m_session.deckFx().value(deck).toString();
}

bool SettingsModel::needsRestart(const QString &path) const
{
    return path.startsWith(QStringLiteral("engine:"));
}

QString SettingsModel::fileOf(const QString &path) const
{
    if (path.startsWith(QStringLiteral("engine:"))) {
        return m_engine.file();
    }
    if (path.startsWith(QStringLiteral("layout:")) || path.startsWith(QStringLiteral("ui:"))) {
        return m_layout.file();
    }
    if (path.startsWith(QStringLiteral("session:"))) {
        return m_session.file();
    }
    return {};
}

QString SettingsModel::report() const
{
    QStringList lines;
    lines << QStringLiteral("hauler-dj %1").arg(QCoreApplication::applicationVersion());
    lines << QStringLiteral("protocol bridge-v1 / state-v1");
    lines << QStringLiteral("qt %1").arg(QString::fromLatin1(qVersion()));
    lines << QStringLiteral("settings (engine): %1").arg(m_engine.file());
    lines << QStringLiteral("layout: %1").arg(m_layout.file());
    lines << QStringLiteral("session: %1").arg(m_session.file());
    lines << QStringLiteral("sample rate: %1").arg(m_engine.value(QStringLiteral("audio.sample_rate")).toString());
    lines << QStringLiteral("block frames: %1").arg(m_engine.value(QStringLiteral("audio.block_frames")).toString());
    lines << QStringLiteral("device: %1").arg(m_engine.value(QStringLiteral("audio.device")).toString());
    return lines.join(QLatin1Char('\n'));
}
