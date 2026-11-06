#include "LayoutSettings.h"

#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QJsonDocument>
#include <QJsonObject>
#include <QStandardPaths>

#include <algorithm>

LayoutSettings::LayoutSettings(QObject *parent)
    : QObject(parent)
    , m_file(defaultFile())
{
}

QString LayoutSettings::defaultFile()
{
    const QString base = QStandardPaths::writableLocation(QStandardPaths::GenericConfigLocation);
    const QString directory = base.isEmpty() ? QDir::homePath() + QStringLiteral("/.config") : base;
    return directory + QStringLiteral("/hauler-dj/layout.json");
}

bool LayoutSettings::applyPreset(const QString &name)
{
    const QString normalised = name.trimmed().toLower();
    if (normalised == QStringLiteral("traktor")) {
        // Measured on the reference screenshots: a global strip across the top, two rows of decks
        // with the mixer between them, and the browser along the bottom.
        m_preset = normalised;
        m_deckRows = 2;
        m_globalPercent = 8.0;
        m_browserPercent = 28.5;
        m_mixerPercent = 14.5;
        normalise();
    } else if (normalised == QStringLiteral("two-deck")) {
        m_preset = normalised;
        m_deckRows = 1;
        m_globalPercent = 8.0;
        m_browserPercent = 34.0;
        m_mixerPercent = 16.0;
        normalise();
    } else {
        return false;
    }
    emit changed();
    return true;
}

void LayoutSettings::setDeckRows(int rows)
{
    const int bounded = std::clamp(rows, 1, 2);
    if (bounded == m_deckRows) {
        return;
    }
    m_deckRows = bounded;
    emit changed();
}

void LayoutSettings::setGlobalPercent(double percent)
{
    m_globalPercent = percent;
    normalise();
    emit changed();
}

void LayoutSettings::setBrowserPercent(double percent)
{
    m_browserPercent = percent;
    normalise();
    emit changed();
}

void LayoutSettings::setMixerPercent(double percent)
{
    // A mixer narrower than a fifth of the window cannot show two channel strips and a crossfader.
    m_mixerPercent = std::clamp(percent, 8.0, 40.0);
    emit changed();
}

bool LayoutSettings::setChannelOrder(const QString &order)
{
    // A permutation of A, B, C, D and nothing else: a repeated letter would draw one deck twice and
    // hide another, which is a layout that lies about the mixer.
    const QString normalised = order.trimmed().toUpper();
    if (normalised.size() != 4) {
        return false;
    }
    for (const QChar letter : {'A', 'B', 'C', 'D'}) {
        if (normalised.count(letter) != 1) {
            return false;
        }
    }
    if (normalised == m_channelOrder) {
        return true;
    }
    m_channelOrder = normalised;
    emit changed();
    return true;
}

void LayoutSettings::normalise()
{
    // The rows fill the window between them: the deck area is what the global strip and the browser
    // leave, so a dragged boundary moves one neighbour and never leaves a gap.
    m_globalPercent = std::clamp(m_globalPercent, 3.0, 25.0);
    m_browserPercent = std::clamp(m_browserPercent, 8.0, 100.0 - m_globalPercent - 20.0);
    m_deckRowsPercent = 100.0 - m_globalPercent - m_browserPercent;
}

void LayoutSettings::setFile(const QString &path)
{
    if (path == m_file) {
        return;
    }
    m_file = path;
    emit changed();
}

bool LayoutSettings::load()
{
    QFile file(m_file);
    if (!file.open(QIODevice::ReadOnly)) {
        return false;
    }
    const QJsonDocument document = QJsonDocument::fromJson(file.readAll());
    if (!document.isObject()) {
        return false;
    }
    const QJsonObject root = document.object();
    m_preset = root.value(QStringLiteral("preset")).toString(m_preset);
    m_deckRows = std::clamp(root.value(QStringLiteral("deck_rows")).toInt(m_deckRows), 1, 2);
    m_globalPercent = root.value(QStringLiteral("global_percent")).toDouble(m_globalPercent);
    m_browserPercent = root.value(QStringLiteral("browser_percent")).toDouble(m_browserPercent);
    m_mixerPercent = root.value(QStringLiteral("mixer_percent")).toDouble(m_mixerPercent);
    setChannelOrder(root.value(QStringLiteral("channel_order")).toString(m_channelOrder));
    normalise();
    emit changed();
    return true;
}

bool LayoutSettings::save() const
{
    const QFileInfo info(m_file);
    if (!QDir().mkpath(info.absolutePath())) {
        return false;
    }
    QJsonObject root;
    root.insert(QStringLiteral("preset"), m_preset);
    root.insert(QStringLiteral("deck_rows"), m_deckRows);
    root.insert(QStringLiteral("global_percent"), m_globalPercent);

    root.insert(QStringLiteral("browser_percent"), m_browserPercent);
    root.insert(QStringLiteral("mixer_percent"), m_mixerPercent);
    root.insert(QStringLiteral("channel_order"), m_channelOrder);
    QFile file(m_file);
    if (!file.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
        return false;
    }
    file.write(QJsonDocument(root).toJson(QJsonDocument::Indented));
    return true;
}
