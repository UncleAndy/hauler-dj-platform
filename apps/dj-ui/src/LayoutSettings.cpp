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
    m_saveTimer = new QTimer(this);
    m_saveTimer->setSingleShot(true);
    m_saveTimer->setInterval(400);
    connect(m_saveTimer, &QTimer::timeout, this, [this] { save(); });
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

void LayoutSettings::setBrowserFont(int pixels)
{
    // Small enough to fit a long file name in the list, large enough to read from a booth: outside
    // that, the browser is unusable and the setting would be a way to break it.
    const int bounded = std::clamp(pixels, 8, 20);
    if (bounded == m_browserFont) {
        return;
    }
    m_browserFont = bounded;
    emit changed();
    scheduleSave();
}

void LayoutSettings::setBrowserRailWidth(int pixels)
{
    const int bounded = std::clamp(pixels, 72, 260);
    if (bounded == m_browserRailWidth) {
        return;
    }
    m_browserRailWidth = bounded;
    emit changed();
    scheduleSave();
}

void LayoutSettings::setWaveformSpan(double span)
{
    // Zero is the "never set" marker; anything else has to be a span the deck can draw.
    const double bounded = span <= 0.0 ? 0.0 : std::clamp(span, 0.0025, 1.0);
    if (qFuzzyCompare(bounded + 1.0, m_waveformSpan + 1.0)) {
        return;
    }
    m_waveformSpan = bounded;
    emit changed();
    scheduleSave();
}

void LayoutSettings::scheduleSave()
{
    if (m_saveTimer->isActive()) {
        m_saveTimer->stop();
    }
    m_saveTimer->start();
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
    m_browserFont = std::clamp(root.value(QStringLiteral("browser_font")).toInt(m_browserFont), 8, 20);
    m_browserRailWidth =
        std::clamp(root.value(QStringLiteral("browser_rail_width")).toInt(m_browserRailWidth), 72, 260);
    const double span = root.value(QStringLiteral("waveform_span")).toDouble(m_waveformSpan);
    m_waveformSpan = span <= 0.0 ? 0.0 : std::clamp(span, 0.0025, 1.0);
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
    root.insert(QStringLiteral("browser_font"), m_browserFont);
    root.insert(QStringLiteral("browser_rail_width"), m_browserRailWidth);
    root.insert(QStringLiteral("waveform_span"), m_waveformSpan);
    QFile file(m_file);
    if (!file.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
        return false;
    }
    file.write(QJsonDocument(root).toJson(QJsonDocument::Indented));
    return true;
}
