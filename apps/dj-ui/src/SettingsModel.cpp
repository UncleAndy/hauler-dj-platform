#include "SettingsModel.h"

#include "EngineSettings.h"
#include "LayoutSettings.h"
#include "SessionState.h"
#include "UiSettings.h"

#include <QClipboard>
#include <QCoreApplication>
#include <QGuiApplication>
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

QStringList SettingsModel::devices() const
{
    QStringList names;
    const QString answer = m_engine.ask({QStringLiteral("--list-devices")});
    const QStringList lines = answer.split(QLatin1Char('\n'), Qt::SkipEmptyParts);
    for (const QString &line : lines) {
        // name, channels, rate, and a star for the default one.
        const QString name = line.section(QLatin1Char('\t'), 0, 0).trimmed();
        if (!name.isEmpty()) {
            names << name;
        }
    }
    return names;
}

QStringList SettingsModel::devicePairs(const QString &device) const
{
    QStringList pairs;
    // Routes, not pairs: the swapped order is a real choice, and a list without it would send the DJ back to a text
    // field for the one thing a setting is for.
    const QString answer = m_engine.ask({QStringLiteral("--device-routes"), device});
    const QStringList lines = answer.split(QLatin1Char('\n'), Qt::SkipEmptyParts);
    for (const QString &line : lines) {
        const QString pair = line.trimmed();
        // `[0, 1]` is what the settings file wants; anything else would be the engine's complaint, not a pair.
        if (pair.startsWith(QLatin1Char('[')) && pair.endsWith(QLatin1Char(']'))) {
            pairs << pair;
        }
    }
    return pairs;
}

QString SettingsModel::describeDevice(const QString &name) const
{
    const QString answer = m_engine.ask({QStringLiteral("--check-device"), name});
    if (answer.isEmpty()) {
        return QStringLiteral("no answer");
    }
    const QStringList fields = answer.trimmed().split(QLatin1Char('\t'));
    if (fields.size() < 3) {
        // The engine answers a refusal on stderr, and that text is the explanation.
        return answer.trimmed();
    }
    const QString channels = fields.size() > 3 ? QStringLiteral("%1 (%2 native)").arg(fields.at(1), fields.at(2))
                                               : fields.at(1);
    const QString rate = fields.size() > 3 ? fields.at(3) : fields.at(2);
    return QStringLiteral("%1 — %2 channels, %3 Hz").arg(fields.at(0), channels, rate);
}

QVariant SettingsModel::value(const QString &path) const
{
    if (path.startsWith(QStringLiteral("engine:"))) {
        return m_engine.value(path.mid(7));
    }
    if (path == QStringLiteral("ui:themeSet")) {
        return m_ui.themeSet();
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
    if (path == QStringLiteral("ui:themeSet")) {
        const bool applied = m_ui.applyThemeSet(value.toString());
        if (applied) {
            emit changed();
        }
        return applied;
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

void SettingsModel::copyReport() const
{
    if (QClipboard *clipboard = QGuiApplication::clipboard()) {
        clipboard->setText(report());
    }
}
