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

namespace {

/// `[0, 1]` as TOML writes a pair, and nothing at all for anything else.
QList<int> parsePair(const QVariant &value)
{
    const QString text = value.toString().trimmed();
    if (!text.startsWith(QLatin1Char('[')) || !text.endsWith(QLatin1Char(']'))) {
        return {};
    }
    QList<int> numbers;
    const QStringList parts = text.mid(1, text.size() - 2).split(QLatin1Char(','));
    for (const QString &part : parts) {
        bool ok = false;
        const int number = part.trimmed().toInt(&ok);
        if (!ok) {
            return {};
        }
        numbers << number;
    }
    return numbers;
}

} // namespace

int SettingsModel::channelAt(const QString &path, int side) const
{
    // A switched-off cue has no channels, whatever the pair still says: the window shows "None" because that is what
    // the engine will do.
    if (path.startsWith(QStringLiteral("engine:audio.headphones."))) {
        const QVariant enabled = value(QStringLiteral("engine:audio.headphones.enabled"));
        if (enabled.isValid() && !enabled.toBool()) {
            return -1;
        }
    }
    const QList<int> pair = parsePair(value(path));
    if (pair.isEmpty() && path == QStringLiteral("engine:audio.master")) {
        // Nothing in the file yet: the main output is on 0/1, which is the route plan's own default, and the window has
        // to show that rather than "None" (which the main output can never be).
        return side;
    }
    return side >= 0 && side < pair.size() ? pair.at(side) : -1;
}

bool SettingsModel::applyHeadphoneMode(const QString &mode)
{
    // The mode itself is part of the decision: a caller that asks for a mode and does not store it leaves the window
    // reading the old one (found by the check, 21.11).
    bool stored = set(QStringLiteral("engine:audio.headphones.mode"), mode);
    if (mode == QStringLiteral("device")) {
        // A device of its own: the engine opens a second stream there, so the channels it can take are that device's, and
        // the mode is the DJ's — his device is kept, and its own channels follow.
        const QVariantMap answer = resolveCue(mode);
        const QString wanted = answer.value(QStringLiteral("device")).toString();
        if (wanted.isEmpty()) {
            return stored;
        }
        const int left = answer.value(QStringLiteral("left")).toInt();
        const int right = answer.value(QStringLiteral("right")).toInt();
        if (left < 0 || right < 0) {
            return set(QStringLiteral("engine:audio.headphones.enabled"), false) && stored;
        }
        bool ok = stored;
        ok = set(QStringLiteral("engine:audio.headphones.pair"), QStringLiteral("[%1, %2]").arg(left).arg(right)) && ok;
        return set(QStringLiteral("engine:audio.headphones.enabled"), true) && ok;
    }
    const QVariantMap answer = resolveCue(mode);
    if (answer.isEmpty()) {
        return false;
    }
    const QString device = answer.value(QStringLiteral("device")).toString();
    const int left = answer.value(QStringLiteral("left")).toInt();
    const int right = answer.value(QStringLiteral("right")).toInt();
    bool ok = stored;
    if (!device.isEmpty()) {
        ok = set(QStringLiteral("engine:audio.headphones.device"), device) && ok;
    }
    if (left < 0 || right < 0) {
        // No spare pair: the cue is off, and the channels keep their numbers so that turning the mode back on does not
        // have to be re-entered by hand.
        return set(QStringLiteral("engine:audio.headphones.enabled"), false) && ok;
    }
    ok = set(QStringLiteral("engine:audio.headphones.pair"), QStringLiteral("[%1, %2]").arg(left).arg(right)) && ok;
    ok = set(QStringLiteral("engine:audio.headphones.enabled"), true) && ok;
    return ok;
}

bool SettingsModel::setChannelAt(const QString &path, int side, int channel)
{
    if (side < 0 || side > 1) {
        return false;
    }
    QList<int> pair = parsePair(value(path));
    while (pair.size() < 2) {
        pair << pair.size();
    }
    if (channel < 0) {
        // No channel means no route: for the cue that is the "None" the window shows when the interface has no spare
        // pair, and a pair with a missing side is not something the engine could open.
        if (path.startsWith(QStringLiteral("engine:audio.headphones."))) {
            return set(QStringLiteral("engine:audio.headphones.enabled"), false);
        }
        return false;
    }
    pair[side] = channel;
    const QString text = QStringLiteral("[%1, %2]").arg(pair.at(0)).arg(pair.at(1));
    if (!set(path, text)) {
        return false;
    }
    // Writing a channel means the cue is wanted: the switch that turned it off has to come back on, or the value would
    // sit in the file doing nothing.
    if (path.startsWith(QStringLiteral("engine:audio.headphones."))) {
        set(QStringLiteral("engine:audio.headphones.enabled"), true);
    }
    return true;
}

QVariantMap SettingsModel::deviceShape(const QString &device) const
{
    QVariantMap shape;
    const QString answer = m_engine.ask({QStringLiteral("--check-device"), device});
    const QStringList fields = answer.trimmed().split(QLatin1Char('\t'));
    if (fields.size() < 4) {
        return shape;
    }
    shape.insert(QStringLiteral("rate"), fields.at(0).toInt());
    shape.insert(QStringLiteral("bits"), fields.at(1).toInt());
    shape.insert(QStringLiteral("channels"), fields.at(2).toInt());
    shape.insert(QStringLiteral("format"), fields.at(3));
    return shape;
}

QVariantMap SettingsModel::resolveCue(const QString &mode) const
{
    QVariantMap answer;
    const QString text = m_engine.ask({QStringLiteral("--resolve-cue"), mode, QStringLiteral("--settings"),
                                       m_engine.file()});
    const QStringList fields = text.trimmed().split(QLatin1Char('\t'));
    if (fields.size() < 3) {
        return answer;
    }
    answer.insert(QStringLiteral("device"), fields.at(0));
    answer.insert(QStringLiteral("left"), fields.at(1) == QStringLiteral("none") ? -1 : fields.at(1).toInt());
    answer.insert(QStringLiteral("right"), fields.at(2) == QStringLiteral("none") ? -1 : fields.at(2).toInt());
    return answer;
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

bool SettingsModel::headphoneDeviceDiffers() const
{
    const QString main = value(QStringLiteral("engine:audio.device")).toString().trimmed();
    const QString phones = value(QStringLiteral("engine:audio.headphones.device")).toString().trimmed();
    // An empty main name means "the system's choice", which is not a name to compare against: only two named interfaces
    // can be told apart, and anything else is "not distinct" — the main check already covers that device.
    if (main.isEmpty() || phones.isEmpty()) {
        return false;
    }
    return main.compare(phones, Qt::CaseInsensitive) != 0;
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
    if (fields.size() < 4) {
        // The engine answers a refusal on stderr, and that text is the explanation.
        return answer.trimmed();
    }
    // Characteristics, not the name: the DJ already sees which device he asked about, and what he needs to know is what
    // the interface will give him (owner's decision, 21.11).
    return QStringLiteral("%1 Hz, %2-bit %3, %4 channels")
        .arg(fields.at(0), fields.at(1), fields.at(3), fields.at(2));
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
