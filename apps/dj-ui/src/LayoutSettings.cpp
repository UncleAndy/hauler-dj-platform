#include "LayoutSettings.h"

#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QJsonArray>
#include <QJsonDocument>
#include <QStorageInfo>
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

bool LayoutSettings::parseLayout(const QString &name, QString *arrangement, QString *edge)
{
    // `traktor` and `two-deck` are the names the first releases wrote; a file from them still opens.
    static const QHash<QString, QPair<QString, QString>> known = {
        {QStringLiteral("classic"), {QStringLiteral("classic"), QStringLiteral("centre")}},
        {QStringLiteral("traktor"), {QStringLiteral("classic"), QStringLiteral("centre")}},
        {QStringLiteral("duo"), {QStringLiteral("duo"), QStringLiteral("centre")}},
        {QStringLiteral("two-deck"), {QStringLiteral("duo"), QStringLiteral("centre")}},
        {QStringLiteral("stack-left"), {QStringLiteral("stack"), QStringLiteral("left")}},
        {QStringLiteral("stack-right"), {QStringLiteral("stack"), QStringLiteral("right")}},
        {QStringLiteral("stack-duo-left"), {QStringLiteral("stack-duo"), QStringLiteral("left")}},
        {QStringLiteral("stack-duo-right"), {QStringLiteral("stack-duo"), QStringLiteral("right")}},
        {QStringLiteral("studio-left"), {QStringLiteral("studio"), QStringLiteral("left")}},
        {QStringLiteral("studio-right"), {QStringLiteral("studio"), QStringLiteral("right")}},
        // The mixer folded away entirely: the decks take its room. For preparing a set, where the
        // crossfader is the one control nobody is going to touch.
        {QStringLiteral("stack-nomixer"), {QStringLiteral("stack"), QStringLiteral("none")}},
        {QStringLiteral("stack-duo-nomixer"), {QStringLiteral("stack-duo"), QStringLiteral("none")}},
        {QStringLiteral("studio-nomixer"), {QStringLiteral("studio"), QStringLiteral("none")}},
    };
    const auto found = known.constFind(name.trimmed().toLower());
    if (found == known.constEnd()) {
        return false;
    }
    *arrangement = found->first;
    *edge = found->second;
    return true;
}

QString LayoutSettings::canonicalName(const QString &arrangement, const QString &edge)
{
    if (arrangement == QStringLiteral("classic") || arrangement == QStringLiteral("duo")) {
        return arrangement;
    }
    // `none` is written as `nomixer`: the stored name is what the menu compares against, and `stack-none`
    // would leave the tick on no entry at all.
    return arrangement + (edge == QStringLiteral("none") ? QStringLiteral("-nomixer")
                                                         : QLatin1Char('-') + edge);
}

LayoutSettings::Numbers LayoutSettings::defaultsFor(const QString &arrangement)
{
    // Measured from the reference screenshots where there was one; the new arrangements start from
    // the same global strip and a browser that leaves the decks room for four (or two) instruments.
    Numbers numbers;
    if (arrangement == QStringLiteral("duo")) {
        numbers.deckRows = 1;
        numbers.browserPercent = 34.0;
        numbers.mixerPercent = 16.0;
    } else if (arrangement == QStringLiteral("stack")) {
        numbers.deckRows = 2;
        numbers.browserPercent = 24.0;
        // The mixer has to hold four law buttons of a readable width: at 13 % the row came out 116 px
        // and the narrowest button 26 px, under the 28 px the check calls a button (Э3, 2026-11-19).
        numbers.mixerPercent = 15.0;
    } else if (arrangement == QStringLiteral("stack-duo")) {
        numbers.deckRows = 1;
        numbers.browserPercent = 28.0;
        numbers.mixerPercent = 15.0;
    } else if (arrangement == QStringLiteral("studio")) {
        numbers.deckRows = 1;
        numbers.browserPercent = 30.0;
        numbers.mixerPercent = 16.0;
    }
    return numbers;
}

void LayoutSettings::storeNumbers()
{
    m_numbers.insert(m_arrangement,
                     Numbers{m_deckRows, m_globalPercent, m_browserPercent, m_mixerPercent});
}

void LayoutSettings::takeNumbers(const QString &arrangement)
{
    const Numbers numbers = m_numbers.contains(arrangement) ? m_numbers.value(arrangement)
                                                            : defaultsFor(arrangement);
    m_deckRows = numbers.deckRows;
    m_globalPercent = numbers.globalPercent;
    m_browserPercent = numbers.browserPercent;
    m_mixerPercent = numbers.mixerPercent;
}

bool LayoutSettings::setLayout(const QString &name)
{
    const QString normalised = name.trimmed().toLower();
    if (normalised == QStringLiteral("custom")) {
        // Not a choice of layout but a state of the numbers: the arrangement stays, the name says the
        // DJ has moved a splitter in it.
        if (m_preset == normalised) {
            return true;
        }
        m_preset = normalised;
        emit changed();
        return true;
    }
    QString arrangement;
    QString edge;
    if (!parseLayout(normalised, &arrangement, &edge)) {
        return false;
    }
    // The layout being left keeps its numbers, dragged or not.
    storeNumbers();
    m_arrangement = arrangement;
    m_mixerEdge = edge;
    takeNumbers(arrangement);
    m_preset = canonicalName(arrangement, edge);
    normalise();
    emit changed();
    scheduleSave();
    return true;
}

bool LayoutSettings::applyPreset(const QString &name)
{
    return setLayout(name);
}

void LayoutSettings::setPreset(const QString &name)
{
    // The shell writes `custom` here when a splitter is dropped; a name from the menu goes through
    // setLayout, so the numbers of the layout being entered are taken along with the name.
    const QString normalised = name.trimmed().toLower();
    if (normalised == QStringLiteral("custom")) {
        if (normalised != m_preset) {
            m_preset = normalised;
            emit changed();
        }
        return;
    }
    if (normalised == m_preset) {
        return;
    }
    (void)setLayout(normalised);
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

QStringList LayoutSettings::normalisedRoots(const QStringList &roots)
{
    QStringList normalised;
    for (const QString &root : roots) {
        const QString trimmed = root.trimmed();
        if (trimmed.isEmpty()) {
            continue;
        }
        // Absolute: the shell's working directory is not the DJ's business, and a relative root would
        // point somewhere else after a start from another folder. Unique: a root listed twice would
        // list its tracks twice.
        const QString absolute = QFileInfo(trimmed).absoluteFilePath();
        if (!normalised.contains(absolute)) {
            normalised.append(absolute);
        }
    }
    return normalised;
}

void LayoutSettings::setCollectionRoots(const QStringList &roots)
{
    const QStringList normalised = normalisedRoots(roots);
    if (normalised == m_collectionRoots) {
        return;
    }
    m_collectionRoots = normalised;
    emit changed();
    // Not written here: the layout file is written when the shell exits, so a root adopted by
    // default lands there too instead of only the one typed on the command line.
    scheduleSave();
}

void LayoutSettings::setCollectionGenres(const QStringList &genres)
{
    // The names are kept as the collection spells them; the comparison is what ignores case. A genre
    // named twice is one chip, because two chips that filter the same rows are a lie about the choice.
    QStringList clean;
    for (const QString &genre : genres) {
        const QString name = genre.trimmed();
        if (name.isEmpty()) {
            continue;
        }
        bool known = false;
        for (const QString &kept : std::as_const(clean)) {
            if (kept.compare(name, Qt::CaseInsensitive) == 0) {
                known = true;
                break;
            }
        }
        if (!known) {
            clean.append(name);
        }
    }
    if (clean == m_collectionGenres) {
        return;
    }
    m_collectionGenres = clean;
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

void LayoutSettings::setSnapGrid(bool on)
{
    if (m_snapGrid == on) {
        return;
    }
    m_snapGrid = on;
    emit changed();
    scheduleSave();
}

void LayoutSettings::setQuantizeGrid(bool on)
{
    if (m_quantizeGrid == on) {
        return;
    }
    m_quantizeGrid = on;
    emit changed();
    scheduleSave();
}

void LayoutSettings::setScrubAudible(bool on)
{
    if (m_scrubAudible == on) {
        return;
    }
    m_scrubAudible = on;
    emit changed();
    scheduleSave();
}

QString LayoutSettings::roomiestRoot() const
{
    QString best;
    qint64 most = -1;
    for (const QString &path : m_collectionRoots) {
        // A root that does not exist yet is judged by the closest parent that does: the import will
        // create it, and its disk is the one that matters.
        QString probe = path;
        while (!probe.isEmpty() && !QFileInfo(probe).exists()) {
            const QString parent = QFileInfo(probe).absolutePath();
            if (parent == probe) {
                break;
            }
            probe = parent;
        }
        const QStorageInfo storage(probe);
        const qint64 free = storage.isValid() ? storage.bytesAvailable() : -1;
        if (free > most) {
            most = free;
            best = path;
        }
    }
    return best;
}

namespace {
/// The columns a collection list starts with, in this order. A DJ who never touched them sees the
/// table the import was designed for.
struct ColumnDefault {
    const char *id;
    double width;
};
constexpr ColumnDefault kColumnDefaults[] = {
    {"artist", 0.22}, {"album", 0.20}, {"title", 0.30},
    {"time", 0.08},   {"bpm", 0.07},   {"key", 0.05},
    {"recent", 0.05},
};
} // namespace

void LayoutSettings::normaliseColumns()
{
    QVector<QPair<QString, double>> kept;
    for (const auto &column : m_columns) {
        bool known = false;
        for (const ColumnDefault &entry : kColumnDefaults) {
            if (column.first == QLatin1String(entry.id)) {
                known = true;
                break;
            }
        }
        if (!known) {
            continue;
        }
        bool duplicate = false;
        for (const auto &other : kept) {
            if (other.first == column.first) {
                duplicate = true;
                break;
            }
        }
        if (duplicate) {
            continue;
        }
        kept.append({column.first, qBound(0.04, column.second, 0.60)});
    }
    for (const ColumnDefault &entry : kColumnDefaults) {
        const QString id = QString::fromLatin1(entry.id);
        bool present = false;
        for (const auto &other : kept) {
            if (other.first == id) {
                present = true;
                break;
            }
        }
        if (!present) {
            kept.append({id, entry.width});
        }
    }
    m_columns = kept;
}

QVariantList LayoutSettings::collectionColumns() const
{
    QVariantList list;
    for (const auto &column : m_columns) {
        list.append(QVariantMap{{QStringLiteral("id"), column.first},
                                {QStringLiteral("width"), column.second}});
    }
    return list;
}

void LayoutSettings::setCollectionColumnWidth(const QString &id, double width)
{
    for (auto &column : m_columns) {
        if (column.first == id) {
            // A column narrower than this is a column nobody can read; a wider one hides the rest.
            column.second = qBound(0.04, width, 0.60);
            emit changed();
            return;
        }
    }
}

void LayoutSettings::moveCollectionColumn(const QString &id, int toIndex)
{
    for (int index = 0; index < m_columns.size(); ++index) {
        if (m_columns.at(index).first != id) {
            continue;
        }
        const QPair<QString, double> column = m_columns.takeAt(index);
        // Dropping to the right of where the column was shifts it by one, because it left the row.
        const int target = qBound(0, toIndex > index ? toIndex - 1 : toIndex, m_columns.size());
        m_columns.insert(target, column);
        emit changed();
        return;
    }
}

void LayoutSettings::resetCollectionColumns()
{
    m_columns.clear();
    normaliseColumns();
    emit changed();
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
    // The numbers of every layout, written by this build; a file without them keeps the single set
    // it has, and that set belongs to the layout the file names.
    m_numbers.clear();
    const QJsonObject storedNumbers = root.value(QStringLiteral("numbers")).toObject();
    for (auto entry = storedNumbers.begin(); entry != storedNumbers.end(); ++entry) {
        const QJsonObject numbers = entry.value().toObject();
        Numbers read;
        read.deckRows = std::clamp(numbers.value(QStringLiteral("deck_rows")).toInt(2), 1, 2);
        read.globalPercent = numbers.value(QStringLiteral("global_percent")).toDouble(8.0);
        read.browserPercent = numbers.value(QStringLiteral("browser_percent")).toDouble(28.5);
        read.mixerPercent = numbers.value(QStringLiteral("mixer_percent")).toDouble(14.5);
        m_numbers.insert(entry.key(), read);
    }
    {
        QString arrangement;
        QString edge;
        const QString storedEdge = root.value(QStringLiteral("mixer_edge")).toString();
        if (parseLayout(m_preset, &arrangement, &edge)) {
            m_arrangement = arrangement;
            m_mixerEdge = storedEdge.isEmpty() ? edge : storedEdge;
            // A file from the previous build names the layout `two-deck`; it reads, but the shell and
            // the menu speak the current names.
            m_preset = canonicalName(arrangement, m_mixerEdge);
        } else if (root.contains(QStringLiteral("arrangement"))) {
            // A name this build does not know but an arrangement it does: keep the arrangement.
            m_arrangement = root.value(QStringLiteral("arrangement")).toString(m_arrangement);
            m_mixerEdge = storedEdge.isEmpty() ? m_mixerEdge : storedEdge;
        }
        if (m_numbers.isEmpty() || !m_numbers.contains(m_arrangement)) {
            m_numbers.insert(m_arrangement,
                             Numbers{m_deckRows, m_globalPercent, m_browserPercent, m_mixerPercent});
        }
        const auto parsed = parseLayout(m_preset, &arrangement, &edge);
        if (!parsed && m_preset != QStringLiteral("custom")) {
            // A file from a build that had other names, or a hand-edited one: fall back rather than
            // open a shell with numbers no layout owns.
            m_preset = QStringLiteral("custom");
        }
    }
    m_browserFont = std::clamp(root.value(QStringLiteral("browser_font")).toInt(m_browserFont), 8, 20);
    m_browserRailWidth =
        std::clamp(root.value(QStringLiteral("browser_rail_width")).toInt(m_browserRailWidth), 72, 260);
    const double span = root.value(QStringLiteral("waveform_span")).toDouble(m_waveformSpan);
    m_snapGrid = root.value(QStringLiteral("snap_grid")).toBool(m_snapGrid);
    m_quantizeGrid = root.value(QStringLiteral("quantize_grid")).toBool(m_quantizeGrid);
    m_scrubAudible = root.value(QStringLiteral("scrub_audible")).toBool(m_scrubAudible);
    m_waveformSpan = span <= 0.0 ? 0.0 : std::clamp(span, 0.0025, 1.0);
    setChannelOrder(root.value(QStringLiteral("channel_order")).toString(m_channelOrder));
    // Several roots are the normal case: one collection may live on several disks. A file written by
    // a build that knew one root still reads, and keeps its root.
    QStringList roots;
    const QJsonValue stored = root.value(QStringLiteral("collection_roots"));
    if (stored.isArray()) {
        for (const QJsonValue &value : stored.toArray()) {
            roots.append(value.toString());
        }
    } else if (root.contains(QStringLiteral("collection_root"))) {
        roots.append(root.value(QStringLiteral("collection_root")).toString());
    }
    m_collectionRoots = normalisedRoots(roots);
    // The chips the DJ left on. A file without them is the whole collection, which is what an empty
    // list means — no migration and no marker.
    m_collectionGenres.clear();
    const QJsonValue storedGenres = root.value(QStringLiteral("collection_genres"));
    if (storedGenres.isArray()) {
        for (const QJsonValue &value : storedGenres.toArray()) {
            const QString genre = value.toString().trimmed();
            if (!genre.isEmpty()) {
                m_collectionGenres.append(genre);
            }
        }
    }
    // The columns the file knows, in the order it has them; anything missing joins at the end.
    // The defaults are what a file without a setting means, so the file's own columns replace them
    // rather than joining them: six defaults plus six saved is still six, and the defaults would win.
    m_columns.clear();
    const QJsonValue storedColumns = root.value(QStringLiteral("collection_columns"));
    if (storedColumns.isArray()) {
        for (const QJsonValue &value : storedColumns.toArray()) {
            const QJsonObject column = value.toObject();
            const QString id = column.value(QStringLiteral("id")).toString();
            if (!id.isEmpty()) {
                m_columns.append({id, column.value(QStringLiteral("width")).toDouble()});
            }
        }
    }
    normaliseColumns();
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
    root.insert(QStringLiteral("arrangement"), m_arrangement);
    root.insert(QStringLiteral("mixer_edge"), m_mixerEdge);
    // Every layout keeps its own numbers; the active one is written twice — inside the map and in the
    // flat keys, so a build that only knows the flat keys still opens the shell as the DJ left it.
    QHash<QString, Numbers> numbers = m_numbers;
    numbers.insert(m_arrangement,
                   Numbers{m_deckRows, m_globalPercent, m_browserPercent, m_mixerPercent});
    QJsonObject storedNumbers;
    for (auto entry = numbers.begin(); entry != numbers.end(); ++entry) {
        QJsonObject one;
        one.insert(QStringLiteral("deck_rows"), entry->deckRows);
        one.insert(QStringLiteral("global_percent"), entry->globalPercent);
        one.insert(QStringLiteral("browser_percent"), entry->browserPercent);
        one.insert(QStringLiteral("mixer_percent"), entry->mixerPercent);
        storedNumbers.insert(entry.key(), one);
    }
    root.insert(QStringLiteral("numbers"), storedNumbers);
    root.insert(QStringLiteral("deck_rows"), m_deckRows);
    root.insert(QStringLiteral("global_percent"), m_globalPercent);

    root.insert(QStringLiteral("browser_percent"), m_browserPercent);
    root.insert(QStringLiteral("mixer_percent"), m_mixerPercent);
    root.insert(QStringLiteral("channel_order"), m_channelOrder);
    root.insert(QStringLiteral("browser_font"), m_browserFont);
    root.insert(QStringLiteral("browser_rail_width"), m_browserRailWidth);
    QJsonArray roots;
    for (const QString &path : m_collectionRoots) {
        roots.append(path);
    }
    root.insert(QStringLiteral("collection_roots"), roots);
    QJsonArray genres;
    for (const QString &genre : m_collectionGenres) {
        genres.append(genre);
    }
    root.insert(QStringLiteral("collection_genres"), genres);
    QJsonArray columns;
    for (const auto &column : m_columns) {
        QJsonObject entry;
        entry.insert(QStringLiteral("id"), column.first);
        entry.insert(QStringLiteral("width"), column.second);
        columns.append(entry);
    }
    root.insert(QStringLiteral("collection_columns"), columns);
    root.insert(QStringLiteral("waveform_span"), m_waveformSpan);
    root.insert(QStringLiteral("snap_grid"), m_snapGrid);
    root.insert(QStringLiteral("quantize_grid"), m_quantizeGrid);
    root.insert(QStringLiteral("scrub_audible"), m_scrubAudible);
    QFile file(m_file);
    if (!file.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
        return false;
    }
    file.write(QJsonDocument(root).toJson(QJsonDocument::Indented));
    return true;
}
