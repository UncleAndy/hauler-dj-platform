#include "BridgeClient.h"

#include <QCoreApplication>
#include <QElapsedTimer>
#include <QThread>

#include <QDebug>
#include <QDir>
#include <QDirIterator>
#include <QFile>
#include <QFileInfo>
#include <QFileInfoList>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonArray>
#include <QJsonObject>
#include <QJsonValue>
#include <QStandardPaths>
#include <QTimer>

#include <algorithm>
#include <cstring>

namespace {

// Mirrors MAX_FRAME_BYTES in crates/dj-bridge: refuse a liar's length before allocating.
constexpr quint32 kMaxFrameBytes = 1u << 20;

// A frame is a little-endian 32-bit length followed by that many bytes of JSON.
void appendFrame(QByteArray &out, const QByteArray &payload)
{
    const auto length = static_cast<quint32>(payload.size());
    char header[4];
    header[0] = static_cast<char>(length & 0xff);
    header[1] = static_cast<char>((length >> 8) & 0xff);
    header[2] = static_cast<char>((length >> 16) & 0xff);
    header[3] = static_cast<char>((length >> 24) & 0xff);
    out.append(header, 4);
    out.append(payload);
}

quint32 readLength(const char *bytes)
{
    const auto *raw = reinterpret_cast<const unsigned char *>(bytes);
    return static_cast<quint32>(raw[0]) | (static_cast<quint32>(raw[1]) << 8)
        | (static_cast<quint32>(raw[2]) << 16) | (static_cast<quint32>(raw[3]) << 24);
}

} // namespace

namespace {

// The folder the import parks a rejected copy in (`DUPLICATES_DIR` in crates/dj-library). Its files
// are not part of the collection until their owner deletes them, so the section does not list them.
bool insideDuplicates(const QString &directory)
{
    return directory.split(QLatin1Char('/')).contains(QStringLiteral("_duplicates"));
}

bool looksLikeAudio(const QString &name)
{
    static const QStringList suffixes = {QStringLiteral("wav"), QStringLiteral("flac"),
                                         QStringLiteral("mp3"), QStringLiteral("ogg"),
                                         QStringLiteral("m4a"), QStringLiteral("aiff"),
                                         QStringLiteral("aif")};
    return suffixes.contains(QFileInfo(name).suffix().toLower());
}

} // namespace

FileBrowser::FileBrowser(QObject *parent)
    : QObject(parent)
{
}

QString FileBrowser::defaultFolder() const
{
    const QString home = QDir::homePath();
    for (const QString &candidate : {QStringLiteral("/Music"), QStringLiteral("/Музыка")}) {
        if (QFileInfo(home + candidate).isDir()) {
            return home + candidate;
        }
    }
    return home;
}

void FileBrowser::open(const QString &path)
{
    load(path.isEmpty() ? defaultFolder() : path);
}

void FileBrowser::up()
{
    if (m_folder.isEmpty()) {
        load(defaultFolder());
        return;
    }
    QDir dir(m_folder);
    if (dir.cdUp()) {
        load(dir.absolutePath());
    }
}

void FileBrowser::load(const QString &path)
{
    QDir dir(path);
    m_error.clear();
    // A folder list is not a collection: the section's name, its filter and its count belong to the
    // other view and must not survive a step into a folder.
    m_mode = QStringLiteral("files");
    m_filter.clear();
    m_all.clear();
    m_total = 0;
    if (!dir.exists()) {
        m_error = QStringLiteral("%1 is not a directory").arg(path);
        emit changed();
        return;
    }
    m_folder = dir.absolutePath();

    QVariantList entries;
    // The way up is the first row of the list, the way every file manager on this desktop does it.
    // Root has no parent, so root has no such row.
    QDir parent(m_folder);
    if (parent.cdUp()) {
        entries.append(QVariantMap{{QStringLiteral("name"), QStringLiteral("..")},
                                   {QStringLiteral("path"), parent.absolutePath()},
                                   {QStringLiteral("dir"), true},
                                   {QStringLiteral("parent"), true}});
    }
    const QFileInfoList dirs = dir.entryInfoList(QDir::Dirs | QDir::NoDotAndDotDot, QDir::Name);
    for (const QFileInfo &info : dirs) {
        entries.append(QVariantMap{{QStringLiteral("name"), info.fileName()},
                                   {QStringLiteral("path"), info.absoluteFilePath()},
                                   {QStringLiteral("dir"), true}});
    }
    const QFileInfoList files = dir.entryInfoList(QDir::Files, QDir::Name);
    for (const QFileInfo &info : files) {
        if (!looksLikeAudio(info.fileName())) {
            continue;
        }
        entries.append(QVariantMap{{QStringLiteral("name"), info.fileName()},
                                   {QStringLiteral("path"), info.absoluteFilePath()},
                                   {QStringLiteral("dir"), false}});
    }
    m_entries = entries;
    m_filesFolder = m_folder;
    emit changed();
}

QString FileBrowser::defaultCollectionRoot() const
{
    // The tree the engine keeps its own data in ($XDG_DATA_HOME/hauler-dj): one collection, one place,
    // and a copy of the collection carries its .djmeta with it.
    const QString base = QStandardPaths::writableLocation(QStandardPaths::GenericDataLocation);
    return base + QStringLiteral("/hauler-dj/collection");
}

QString FileBrowser::cursor() const
{
    return m_mode == QLatin1String("collection") ? m_collectionCursor : m_filesCursor;
}

void FileBrowser::setCursor(const QString &path)
{
    QString &remembered = m_mode == QLatin1String("collection") ? m_collectionCursor : m_filesCursor;
    if (remembered == path) {
        return;
    }
    remembered = path;
    emit cursorChanged();
}

void FileBrowser::seedCursors(const QString &files, const QString &collection)
{
    m_filesCursor = files;
    m_collectionCursor = collection;
}

int FileBrowser::rowOf(const QString &path) const
{
    for (int index = 0; index < m_entries.size(); ++index) {
        if (m_entries.at(index).toMap().value(QStringLiteral("path")).toString() == path) {
            return index;
        }
    }
    return -1;
}

void FileBrowser::refreshCollection()
{
    if (m_mode != QLatin1String("collection")) {
        return;
    }
    // The roots are the ones the section was opened with, and the search is the DJ's: both stay.
    const QString filter = m_filter;
    openCollection(m_roots);
    if (!filter.isEmpty()) {
        setFilter(filter);
    }
}

void FileBrowser::openCollection(const QStringList &roots)
{
    const QStringList listed = roots.isEmpty() ? QStringList{defaultCollectionRoot()} : roots;
    m_roots = listed;
    m_folder = listed.join(QStringLiteral(" | "));
    m_mode = QStringLiteral("collection");
    m_pageOffset = 0;
    m_pageTotal = 0;
    m_pageAnsweredFor = 0;
    setPaged(false);
    // The interface does not walk the disks: the walk is how the database is prepared, not how it is
    // read. An empty answer here is an empty collection, and it is shown as such.
    m_all.clear();
    m_total = 0;
    applyFilter();
    emit changed();
    if (m_bridge != nullptr && m_bridge->isConnected()) {
        askPage(0);
    }
}

void FileBrowser::setPaged(bool paged)
{
    if (m_paged == paged) {
        return;
    }
    m_paged = paged;
    emit pagedChanged();
}

void FileBrowser::askPage(int offset)
{
    if (m_bridge == nullptr) {
        return;
    }
    const int id = ++m_pageQuery;
    QVariantMap query;
    query.insert(QStringLiteral("id"), id);
    QVariantList order;
    for (const QVariant &entry : m_sort) {
        const QVariantMap key = entry.toMap();
        QVariantMap asked;
        asked.insert(QStringLiteral("column"), key.value(QStringLiteral("column")));
        asked.insert(QStringLiteral("ascending"), key.value(QStringLiteral("ascending")));
        order.append(asked);
    }
    query.insert(QStringLiteral("order"), order);
    query.insert(QStringLiteral("genres"), m_genres);
    if (!m_filter.isEmpty()) {
        query.insert(QStringLiteral("search"), m_filter);
    }
    query.insert(QStringLiteral("roots"), m_roots);
    query.insert(QStringLiteral("offset"), offset);
    query.insert(QStringLiteral("limit"), 200);
    m_askedOffset = offset;
    m_bridge->queryCollection(query);
}

void FileBrowser::loadMore()
{
    if (!m_paged || m_bridge == nullptr || m_pageOffset <= 0 || m_pageOffset >= m_pageTotal) {
        return;
    }
    askPage(m_pageOffset);
}

void FileBrowser::takePage()
{
    if (m_bridge == nullptr || m_mode != QLatin1String("collection")) {
        return;
    }
    const QVariantMap answer = m_bridge->collectionPage();
    const int id = answer.value(QStringLiteral("id")).toInt();
    if (id != m_pageQuery || id == m_pageAnsweredFor) {
        return;
    }
    m_pageAnsweredFor = id;
    const QVariantList rows = answer.value(QStringLiteral("rows")).toList();
    const int total = answer.value(QStringLiteral("total")).toInt();
    // The page does not carry its offset back: the question does, and the id says which question.
    const int offset = m_askedOffset;
    if (offset == 0 && rows.isEmpty() && total == 0) {
        // The index is the source: it answered, and the answer is "nothing under these roots".
        setPaged(true);
        return;
    }
    QVariantList tracks;
    for (const QVariant &entry : rows) {
        const QVariantMap row = entry.toMap();
        const QString path = row.value(QStringLiteral("path")).toString();
        const double frames = row.value(QStringLiteral("duration_frames")).toDouble();
        const double rate = row.value(QStringLiteral("sample_rate")).toDouble();
        QVariantMap made{{QStringLiteral("name"), QFileInfo(path).completeBaseName()},
                         {QStringLiteral("path"), path},
                         {QStringLiteral("dir"), false},
                         {QStringLiteral("artist"), row.value(QStringLiteral("artist"))},
                         {QStringLiteral("album"), row.value(QStringLiteral("album"))},
                         {QStringLiteral("title"), row.value(QStringLiteral("title"))},
                         {QStringLiteral("fingerprint"), row.value(QStringLiteral("fingerprint"))}};
        made.insert(QStringLiteral("duration_ms"),
                    rate > 0.0 && frames > 0.0
                        ? static_cast<qint64>(frames * 1000.0 / rate + 0.5)
                        : 0);
        QStringList genres;
        const QString names = row.value(QStringLiteral("genre")).toString();
        for (const QString &genre : names.split(QStringLiteral(", "), Qt::SkipEmptyParts)) {
            genres.append(genre.trimmed());
        }
        made.insert(QStringLiteral("genres"), genres);
        tracks.append(made);
    }
    // Replacing or appending, the DJ's place stays: a cursor that jumps to the top on every batch is a
    // cursor he has to find again by hand.
    const QString at = cursor();
    if (offset == 0) {
        setPaged(true);
        m_all = tracks;
        m_pageOffset = tracks.size();
        m_pageTotal = total;
        m_total = total;
    } else {
        if (tracks.isEmpty()) {
            m_pageTotal = m_pageOffset;
            return;
        }
        m_all += tracks;
        m_pageOffset += tracks.size();
        m_pageTotal = total;
    }
    applyFilter();
    if (!at.isEmpty() && rowOf(at) >= 0) {
        setCursor(at);
    }
    emit genresChanged();
}

bool FileBrowser::entryIsFile(const QString &path) const
{
    return QFileInfo(path).isFile();
}

void FileBrowser::attachBridge(BridgeClient *bridge)
{
    m_bridge = bridge;
    if (bridge != nullptr) {
        connect(bridge, &BridgeClient::snapshotChanged, this, &FileBrowser::takePage);
    }
}

void FileBrowser::showFiles()
{
    load(m_filesFolder.isEmpty() ? defaultFolder() : m_filesFolder);
}

void FileBrowser::rememberFolder(const QString &folder)
{
    if (folder.isEmpty()) {
        return;
    }
    // A track taken out of the collection lives in the collection: its folder is not where the DJ was
    // browsing, and the file list must stay where he left it. In the file list a load does move the
    // list — that is how browsing follows what the DJ takes out of a folder he just opened.
    if (m_mode != QLatin1String("files")) {
        return;
    }
    load(folder);
}

void FileBrowser::collect(const QStringList &roots)
{
    QVariantList tracks;
    QSet<QString> seen;
    for (const QString &root : roots) {
        QDirIterator walker(root, QDir::Files | QDir::NoDotAndDotDot, QDirIterator::Subdirectories);
        while (walker.hasNext()) {
            walker.next();
            const QFileInfo info = walker.fileInfo();
        // The metadata folder holds records, not music, and a scan that lists it lists the wrong tree.
            if (info.dir().dirName() == QLatin1String(".djmeta")
                || insideDuplicates(info.dir().absolutePath()) || !looksLikeAudio(info.fileName())) {
                continue;
            }
            // One track, one row: roots may overlap, and a collection listed twice is not two
            // collections.
            if (seen.contains(info.absoluteFilePath())) {
                continue;
            }
            seen.insert(info.absoluteFilePath());
            // A collection row is about the track, not about the file: artist, album, title, how long
            // it plays, its tempo and its key. The file name is none of those and is not shown.
            QString artist;
            QString album;
            QString title;
            QStringList genres;
            qint64 durationMs = 0;
            QVariantMap entry{{QStringLiteral("name"), info.completeBaseName()},
                              {QStringLiteral("path"), info.absoluteFilePath()},
                              {QStringLiteral("dir"), false}};
            const QString record =
                info.dir().absolutePath() + QStringLiteral("/.djmeta/") + info.fileName()
                + QStringLiteral(".json");
            QFile meta(record);
            if (meta.open(QIODevice::ReadOnly)) {
                const QJsonObject object = QJsonDocument::fromJson(meta.readAll()).object();
                const QString key = object.value(QStringLiteral("key")).toString();
                if (!key.isEmpty()) {
                    entry.insert(QStringLiteral("key"), key);
                }
                const double bpm = object.value(QStringLiteral("beatgrid"))
                                       .toObject()
                                       .value(QStringLiteral("bpm"))
                                       .toDouble();
                if (bpm > 0.0) {
                    entry.insert(QStringLiteral("bpm"), bpm);
                }
                // The names the import filed the copy under; an older record has none of them.
                const QJsonObject names = object.value(QStringLiteral("tags")).toObject();
                artist = names.value(QStringLiteral("artist")).toString();
                album = names.value(QStringLiteral("album")).toString();
                title = names.value(QStringLiteral("title")).toString();
                // The genres the record kept, already split: the filter reads them from here, so it
                // works on the same walk that builds the list.
                for (const QJsonValue &value : names.value(QStringLiteral("genres")).toArray()) {
                    const QString genre = value.toString().trimmed();
                    if (!genre.isEmpty()) {
                        genres.append(genre);
                    }
                }
                // Frames and the rate that gives them meaning: the length without decoding again.
                const QJsonObject print = object.value(QStringLiteral("fingerprint")).toObject();
                const double rate = print.value(QStringLiteral("sample_rate")).toDouble();
                const double frames = print.value(QStringLiteral("duration_frames")).toDouble();
                if (rate > 0.0 && frames > 0.0) {
                    durationMs = static_cast<qint64>(frames * 1000.0 / rate + 0.5);
                }
            }
            // The import names the copy `<artist> - <title>`, so a record written before the names
            // travelled still says most of what a row needs.
            if (artist.isEmpty() && title.isEmpty()) {
                const QString stem = info.completeBaseName();
                const int dash = stem.indexOf(QStringLiteral(" - "));
                if (dash > 0) {
                    artist = stem.left(dash);
                    title = stem.mid(dash + 3);
                } else {
                    title = stem;
                }
            }
            entry.insert(QStringLiteral("artist"), artist);
            entry.insert(QStringLiteral("album"), album);
            entry.insert(QStringLiteral("title"), title);
            entry.insert(QStringLiteral("duration_ms"), durationMs);
            entry.insert(QStringLiteral("genres"), genres);
            tracks.append(entry);
        }
    }
    std::sort(tracks.begin(), tracks.end(), [](const QVariant &left, const QVariant &right) {
        const QString a = left.toMap().value(QStringLiteral("name")).toString();
        const QString b = right.toMap().value(QStringLiteral("name")).toString();
        return QString::localeAwareCompare(a, b) < 0;
    });
    m_all = tracks;
    // A filter that no longer matches anything after a refresh would show an empty list with no chip
    // to blame; the chips are rebuilt from the same rows, so the DJ sees what is left.
    emit genresChanged();
    m_total = tracks.size();
    // A refresh reads the disks again: the sort the DJ chose outlives it.
    applySort();
}

void FileBrowser::setFilter(const QString &text)
{
    if (text == m_filter) {
        return;
    }
    m_filter = text;
    applyFilter();
    // The list is a window over the index now: a narrower question is a new question.
    if (m_paged) {
        askPage(0);
    }
}

int BridgeClient::queryCollection(const QVariantMap &query)
{
    // The command body the protocol spells: `{"QueryCollection": {"query": …}}`. The page identity and
    // the sort keys travel as they are — the engine speaks the same words, and a translation here would
    // be a second vocabulary to keep in step.
    QJsonObject body;
    body.insert(QStringLiteral("query"), QJsonObject::fromVariantMap(query));
    QJsonObject command;
    command.insert(QStringLiteral("QueryCollection"), body);
    return sendCommand(QString(), command);
}

QVariantMap BridgeClient::collectionPage() const
{
    const QVariant value = m_snapshot.value(QStringLiteral("collection"));
    if (!value.isValid() || value.isNull()) {
        return QVariantMap();
    }
    return value.toMap();
}

QVariantList FileBrowser::genreList() const
{
    // First spelling wins, counting ignores case: `House` and `house` are one chip with two tracks.
    QMap<QString, QString> spelling;
    QMap<QString, int> counts;
    for (const QVariant &entry : m_all) {
        for (const QString &genre : entry.toMap().value(QStringLiteral("genres")).toStringList()) {
            const QString key = genre.toLower();
            if (!counts.contains(key)) {
                spelling.insert(key, genre);
            }
            counts[key] += 1;
        }
    }
    QVariantList list;
    for (auto it = counts.cbegin(); it != counts.cend(); ++it) {
        QVariantMap item;
        item.insert(QStringLiteral("name"), spelling.value(it.key()));
        item.insert(QStringLiteral("count"), it.value());
        list.append(item);
    }
    return list;
}

void FileBrowser::setGenreFilter(const QStringList &genres)
{
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
    if (clean == m_genres) {
        return;
    }
    m_genres = clean;
    emit genresChanged();
    emit genresPicked();
    applyFilter();
    // A new order or a new chip set is a new question for the index.
    if (m_paged) {
        askPage(0);
    }
}

void FileBrowser::applyFilter()
{
    const bool byGenre = !m_genres.isEmpty();
    if (m_filter.isEmpty() && !byGenre) {
        m_entries = m_all;
        emit changed();
        return;
    }
    QVariantList kept;
    const QString needle = m_filter.toLower();
    for (const QVariant &entry : std::as_const(m_all)) {
        const QVariantMap fields = entry.toMap();
        if (byGenre) {
            // One genre is enough: several chips are a union, not an intersection — a DJ looking for
            // house and techno wants both, not the tracks that are somehow both at once.
            bool carried = false;
            for (const QString &genre : fields.value(QStringLiteral("genres")).toStringList()) {
                for (const QString &wanted : m_genres) {
                    if (genre.compare(wanted, Qt::CaseInsensitive) == 0) {
                        carried = true;
                        break;
                    }
                }
                if (carried) {
                    break;
                }
            }
            if (!carried) {
                continue;
            }
        }
        if (!needle.isEmpty()) {
            // A DJ searches for what he knows: the artist, the album, the title — and the file name for
            // the tracks that came in without tags.
            bool hit = false;
            for (const char *field : {"artist", "album", "title", "name"}) {
                if (fields.value(QLatin1String(field)).toString().toLower().contains(needle)) {
                    hit = true;
                    break;
                }
            }
            if (!hit) {
                continue;
            }
        }
        kept.append(entry);
    }
    m_entries = kept;
    emit changed();
}

namespace {

/// Whether a row has nothing to say for this column: such rows go last whichever way the sort runs.
bool emptyFor(const QVariantMap &row, const QString &column)
{
    if (column == QStringLiteral("time")) {
        return !(row.value(QStringLiteral("duration_ms")).toDouble() > 0.0);
    }
    if (column == QStringLiteral("bpm")) {
        return !(row.value(QStringLiteral("bpm")).toDouble() > 0.0);
    }
    return row.value(column).toString().trimmed().isEmpty();
}

/// The Camelot order of a key: `2d` before `10d`, `8d` before `8m`. A key nobody can read is not one.
int camelot(const QString &key, bool *read)
{
    const QString text = key.trimmed();
    if (text.size() < 2) {
        *read = false;
        return 0;
    }
    // Both notations a DJ meets: Traktor's `8d`/`8m` and Camelot's `8A`/`8B`, in either case. A key
    // written in capitals is the same key, and sorting it as text puts 10d before 2d.
    const QChar letter = text.at(text.size() - 1).toLower();
    bool ok = false;
    const int wheel = text.left(text.size() - 1).toInt(&ok);
    const bool minor = letter == QLatin1Char('m') || letter == QLatin1Char('b');
    const bool major = letter == QLatin1Char('d') || letter == QLatin1Char('a');
    if (!ok || wheel <= 0 || wheel > 12 || !(minor || major)) {
        *read = false;
        return 0;
    }
    *read = true;
    return wheel * 2 + (minor ? 1 : 0);
}

/// The order of two rows in one column, ascending.
int compareColumn(const QVariantMap &left, const QVariantMap &right, const QString &column)
{
    const auto three = [](double a, double b) { return a < b ? -1 : (a > b ? 1 : 0); };
    if (column == QStringLiteral("time")) {
        return three(left.value(QStringLiteral("duration_ms")).toDouble(),
                     right.value(QStringLiteral("duration_ms")).toDouble());
    }
    if (column == QStringLiteral("bpm")) {
        // Two decimals, no more: the measurement carries arbitrary precision, and a column that is
        // never equal to its neighbour is a column that never lets the next key decide. The column
        // shows whole numbers, so nothing visible is lost by comparing hundredths.
        const auto hundredths = [](const QVariantMap &row) {
            return qRound64(row.value(QStringLiteral("bpm")).toDouble() * 100.0);
        };
        return three(static_cast<double>(hundredths(left)),
                     static_cast<double>(hundredths(right)));
    }
    if (column == QStringLiteral("key")) {
        bool leftRead = false;
        bool rightRead = false;
        const int leftKey = camelot(left.value(QStringLiteral("key")).toString(), &leftRead);
        const int rightKey = camelot(right.value(QStringLiteral("key")).toString(), &rightRead);
        if (leftRead && rightRead) {
            return leftKey < rightKey ? -1 : (leftKey > rightKey ? 1 : 0);
        }
        // A key in a notation the wheel cannot read is compared as the text it is.
    }
    const QString a = left.value(column).toString().toLower();
    const QString b = right.value(column).toString().toLower();
    return QString::localeAwareCompare(a, b);
}

/// The order of two rows under the whole list of keys, the first key deciding.
int compareRows(const QVariantMap &left, const QVariantMap &right, const QVariantList &keys)
{
    for (const QVariant &entry : keys) {
        const QVariantMap key = entry.toMap();
        const QString column = key.value(QStringLiteral("column")).toString();
        const bool ascending = key.value(QStringLiteral("ascending")).toBool();
        const bool leftEmpty = emptyFor(left, column);
        const bool rightEmpty = emptyFor(right, column);
        // An empty cell is not a value: it sits at the bottom whichever way the column runs.
        if (leftEmpty != rightEmpty) {
            return leftEmpty ? 1 : -1;
        }
        if (leftEmpty && rightEmpty) {
            continue;
        }
        const int order = compareColumn(left, right, column);
        if (order != 0) {
            return ascending ? order : -order;
        }
    }
    // Equal rows keep a stable order: a list that reshuffles between them jumps under the hand.
    return QString::localeAwareCompare(left.value(QStringLiteral("name")).toString().toLower(),
                                       right.value(QStringLiteral("name")).toString().toLower());
}

} // namespace

void FileBrowser::sortCollection(const QString &column, bool shift)
{
    if (column.isEmpty()) {
        return;
    }
    int existing = -1;
    for (int index = 0; index < m_sort.size(); ++index) {
        if (m_sort.at(index).toMap().value(QStringLiteral("column")).toString() == column) {
            existing = index;
            break;
        }
    }
    QVariantList next;
    if (shift) {
        // Shift adds a key: the column the list is already sorted by stays first, and this one decides
        // between the rows that the first could not.
        next = m_sort;
        if (existing >= 0) {
            QVariantMap key = next.at(existing).toMap();
            key.insert(QStringLiteral("ascending"), !key.value(QStringLiteral("ascending")).toBool());
            next[existing] = key;
        } else {
            next.append(QVariantMap{{QStringLiteral("column"), column},
                                    {QStringLiteral("ascending"), true}});
        }
    } else if (m_sort.size() == 1 && existing == 0) {
        // The same column twice in a row turns it around.
        QVariantMap key = m_sort.first().toMap();
        key.insert(QStringLiteral("ascending"), !key.value(QStringLiteral("ascending")).toBool());
        next.append(key);
    } else {
        next.append(QVariantMap{{QStringLiteral("column"), column},
                                {QStringLiteral("ascending"), true}});
    }
    m_sort = next;
    emit sortChanged();
    applySort();
}

void FileBrowser::applySort()
{
    if (m_sort.isEmpty()) {
        return;
    }
    std::sort(m_all.begin(), m_all.end(), [this](const QVariant &left, const QVariant &right) {
        return compareRows(left.toMap(), right.toMap(), m_sort) < 0;
    });
    applyFilter();
    // A new order or a new chip set is a new question for the index.
    if (m_paged) {
        askPage(0);
    }
}

BridgeClient::BridgeClient(QObject *parent)
    : QObject(parent)
{
    connect(&m_socket, &QLocalSocket::connected, this, &BridgeClient::onConnected);
    connect(&m_socket, &QLocalSocket::disconnected, this, &BridgeClient::onDisconnected);
    connect(&m_socket, &QLocalSocket::readyRead, this, &BridgeClient::onReadyRead);
#if QT_VERSION >= QT_VERSION_CHECK(6, 5, 0)
    connect(&m_socket, &QLocalSocket::errorOccurred, this, &BridgeClient::onSocketError);
#else
    connect(&m_socket, QOverload<QLocalSocket::LocalSocketError>::of(&QLocalSocket::error),
            this, &BridgeClient::onSocketError);
#endif
    setStatus(QStringLiteral("offline"));
}

QString BridgeClient::homeFolder() const
{
    const QString home = QDir::homePath();
    const QString music = home + QStringLiteral("/Music");
    if (QFileInfo(music).isDir()) {
        return music;
    }
    const QString localised = home + QStringLiteral("/Музыка");
    if (QFileInfo(localised).isDir()) {
        return localised;
    }
    return home;
}

void BridgeClient::connectToEngine(const QString &path)
{
    if (m_socket.state() != QLocalSocket::UnconnectedState) {
        m_socket.abort();
    }
    m_buffer.clear();
    m_handshakeSent = false;
    setConnected(false);
    setStatus(QStringLiteral("connecting"));
    m_socket.connectToServer(path);
}

void BridgeClient::onConnected()
{
    QJsonObject hello;
    QJsonObject protocol;
    protocol.insert(QStringLiteral("protocol"), QStringLiteral("command-v1"));
    hello.insert(QStringLiteral("Hello"), protocol);
    sendJson(hello);
    m_handshakeSent = true;
    setStatus(QStringLiteral("handshaking"));
}

void BridgeClient::onDisconnected()
{
    if (m_shuttingDown) {
        return;
    }
    setConnected(false);
    setStatus(QStringLiteral("disconnected"));
}

void BridgeClient::onSocketError(QLocalSocket::LocalSocketError error)
{
    Q_UNUSED(error);
    setConnected(false);
    setStatus(QStringLiteral("error: %1").arg(m_socket.errorString()));
    emit failed(m_socket.errorString());
}

void BridgeClient::onReadyRead()
{
    m_buffer.append(m_socket.readAll());

    // One frame at a time, and only when the whole payload is here.
    while (m_buffer.size() >= 4) {
        const quint32 length = readLength(m_buffer.constData());
        if (length > kMaxFrameBytes) {
            setStatus(QStringLiteral("error: frame of %1 bytes refused").arg(length));
            emit failed(QStringLiteral("the engine announced an implausible frame"));
            m_socket.abort();
            return;
        }
        if (static_cast<quint32>(m_buffer.size()) < 4 + length) {
            return; // wait for the rest
        }
        const QByteArray payload = m_buffer.mid(4, static_cast<int>(length));
        m_buffer.remove(0, 4 + static_cast<int>(length));

        QJsonParseError parseError{};
        const QJsonDocument document = QJsonDocument::fromJson(payload, &parseError);
        if (parseError.error != QJsonParseError::NoError || !document.isObject()) {
            emit failed(QStringLiteral("the engine sent invalid JSON: %1").arg(parseError.errorString()));
            continue;
        }
        handleMessage(document.object());
    }
}

void BridgeClient::handleMessage(const QJsonObject &message)
{
    if (message.contains(QStringLiteral("Welcome"))) {
        const QJsonObject welcome = message.value(QStringLiteral("Welcome")).toObject();
        const QString protocol = welcome.value(QStringLiteral("protocol")).toString();
        const QString stateProtocol = welcome.value(QStringLiteral("state_protocol")).toString();
        m_epoch = static_cast<qulonglong>(welcome.value(QStringLiteral("engine_epoch")).toDouble());
        if (stateProtocol != QStringLiteral("state-v1")) {
            setStatus(QStringLiteral("error: engine speaks %1").arg(stateProtocol));
            emit failed(QStringLiteral("incompatible state protocol %1").arg(stateProtocol));
            m_socket.abort();
            return;
        }
        setConnected(true);
        setStatus(QStringLiteral("%1 · %2").arg(protocol, stateProtocol));
        return;
    }

    if (message.contains(QStringLiteral("Snapshot"))) {
        m_snapshot = message.value(QStringLiteral("Snapshot")).toObject().toVariantMap();
        emit snapshotChanged();
        return;
    }

    if (message.contains(QStringLiteral("Ack"))) {
        const QJsonObject ack = message.value(QStringLiteral("Ack")).toObject();
        const int id = static_cast<int>(ack.value(QStringLiteral("command_id")).toDouble());
        const QJsonObject status = ack.value(QStringLiteral("status")).toObject();
        const bool applied = status.contains(QStringLiteral("Applied"));
        const bool rejected = status.contains(QStringLiteral("Rejected"));
        QString reason;
        if (rejected) {
            const QJsonValue value = status.value(QStringLiteral("Rejected"));
            reason = value.toObject().value(QStringLiteral("reason")).toString();
            if (reason.isEmpty()) {
                reason = value.toVariant().toString();
            }
        }
        if (m_pendingCommands > 0) {
            --m_pendingCommands;
        }
        if (m_pendingCommands == 0 && m_connected) {
            setStatus(QStringLiteral("connected"));
        }
        emit acked(id, applied, rejected, reason);
        return;
    }

    if (message.contains(QStringLiteral("Pong"))) {
        return;
    }

    if (message.contains(QStringLiteral("Error"))) {
        const QString detail = message.value(QStringLiteral("Error"))
                                   .toObject()
                                   .value(QStringLiteral("message"))
                                   .toString();
        setStatus(QStringLiteral("error: %1").arg(detail));
        emit failed(detail);
    }
}

void BridgeClient::sendJson(const QJsonObject &message)
{
    QByteArray frame;
    appendFrame(frame, QJsonDocument(message).toJson(QJsonDocument::Compact));
    m_socket.write(frame);
    m_socket.flush();
}

int BridgeClient::sendCommand(const QString &deck, const QJsonObject &command)
{
    // The deck travels inside the command itself; the argument is here so a call site reads as "this is
    // a command for deck A" rather than as a bare object, and the engine takes the target from the
    // command body it receives.
    Q_UNUSED(deck)
    const int id = m_nextCommandId++;
    QJsonObject scheduled;
    scheduled.insert(QStringLiteral("id"), id);
    // Zero means "as soon as possible"; the engine applies it at the current sample and says where.
    scheduled.insert(QStringLiteral("effective_sample"), 0);
    scheduled.insert(QStringLiteral("command"), command);

    QJsonObject message;
    message.insert(QStringLiteral("Command"), scheduled);
    sendJson(message);
    ++m_pendingCommands;
    setStatus(QStringLiteral("waiting for the engine"));
    return id;
}

int BridgeClient::play(const QString &deck, bool enabled)
{
    QJsonObject options;
    options.insert(QStringLiteral("deck"), deck);
    options.insert(QStringLiteral("enabled"), enabled);
    QJsonObject command;
    command.insert(QStringLiteral("Play"), options);
    return sendCommand(deck, command);
}

int BridgeClient::metronome(const QString &deck, bool enabled)
{
    QJsonObject options;
    options.insert(QStringLiteral("deck"), deck);
    options.insert(QStringLiteral("enabled"), enabled);
    QJsonObject command;
    command.insert(QStringLiteral("Metronome"), options);
    return sendCommand(deck, command);
}

int BridgeClient::cue(const QString &deck, const QString &action, int slot)
{
    // The engine's CueAction is an externally tagged enum: Set(3) is {"Set": 3}.
    QJsonObject options;
    options.insert(QStringLiteral("deck"), deck);
    options.insert(QStringLiteral("action"), QJsonObject{{action, slot}});
    QJsonObject command;
    command.insert(QStringLiteral("Cue"), options);
    return sendCommand(deck, command);
}

int BridgeClient::importPaths(const QStringList &paths, const QString &root, bool parallel)
{
    QJsonArray list;
    for (const QString &path : paths) {
        list.append(path);
    }
    QJsonObject options;
    options.insert(QStringLiteral("paths"), list);
    options.insert(QStringLiteral("root"), root);
    options.insert(QStringLiteral("parallel"), parallel);
    QJsonObject command;
    command.insert(QStringLiteral("Import"), options);
    return sendCommand(QString(), command);
}

int BridgeClient::cancelImport()
{
    QJsonObject command;
    command.insert(QStringLiteral("ImportCancel"), QJsonObject());
    return sendCommand(QString(), command);
}

int BridgeClient::answerImport(const QString &answer)
{
    // The words are the protocol's own: a misspelling reaches the engine as a refused command rather
    // than as a silent no-op here.
    QJsonObject options;
    options.insert(QStringLiteral("answer"), answer);
    QJsonObject command;
    command.insert(QStringLiteral("ImportAnswer"), options);
    return sendCommand(QString(), command);
}

int BridgeClient::analyzeTrack(const QStringList &paths)
{
    int sent = 0;
    for (const QString &path : paths) {
        QJsonObject options;
        options.insert(QStringLiteral("path"), path);
        QJsonObject command;
        command.insert(QStringLiteral("AnalyzeTrack"), options);
        sendCommand(QString(), command);
        ++sent;
    }
    return sent;
}

int BridgeClient::analyzePaths(const QStringList &paths, bool parallel)
{
    QJsonArray list;
    for (const QString &path : paths) {
        list.append(path);
    }
    QJsonObject options;
    options.insert(QStringLiteral("paths"), list);
    options.insert(QStringLiteral("parallel"), parallel);
    QJsonObject command;
    command.insert(QStringLiteral("AnalyzeTracks"), options);
    return sendCommand(QString(), command);
}

int BridgeClient::cancelAnalysis()
{
    QJsonObject command;
    command.insert(QStringLiteral("AnalyzeCancel"), QJsonObject());
    return sendCommand(QString(), command);
}

int BridgeClient::load(const QString &deck, const QString &path)
{
    // Remembered before the command is sent and regardless of its fate: the folder is where the
    // person was looking, which stays true even if the engine refuses the track.
    if (m_loadFolder && !path.isEmpty()) {
        m_loadFolder(QFileInfo(path).absolutePath());
    }
    QJsonObject options;
    options.insert(QStringLiteral("deck"), deck);
    options.insert(QStringLiteral("path"), path);
    QJsonObject command;
    command.insert(QStringLiteral("Load"), options);
    if (!path.isEmpty()) {
        m_deckPaths.insert(deck, path);
    }
    return sendCommand(deck, command);
}

int BridgeClient::analyzeTempo(const QString &deck)
{
    // Only a file this client loaded can be measured: the path is what the analysis reads, and a guess
    // here would write one track's numbers into another track's metadata.
    const QString path = m_deckPaths.value(deck);
    if (path.isEmpty()) {
        return -1;
    }
    QJsonObject options;
    options.insert(QStringLiteral("deck"), deck);
    options.insert(QStringLiteral("path"), path);
    QJsonObject command;
    command.insert(QStringLiteral("AnalyzeTempo"), options);
    return sendCommand(deck, command);
}

int BridgeClient::sendGridEdit(const QString &deck, const QJsonValue &edit)
{
    // The engine writes the edit beside the file and knows which file each deck holds. A client that
    // reconnected to a running engine never loaded anything and has no path of its own: it sends the
    // edit anyway, and the deck answers for the file. Refusing here made every grid button dead after
    // the UI was restarted.
    const QString path = m_deckPaths.value(deck);
    QJsonObject payload;
    payload.insert(QStringLiteral("deck"), deck);
    payload.insert(QStringLiteral("path"), path);
    payload.insert(QStringLiteral("edit"), edit);
    QJsonObject command;
    command.insert(QStringLiteral("EditGrid"), payload);
    return sendCommand(deck, command);
}

int BridgeClient::shiftGrid(const QString &deck, qlonglong frames)
{
    QJsonObject inner;
    inner.insert(QStringLiteral("frames"), QJsonValue::fromVariant(frames));
    QJsonObject edit;
    edit.insert(QStringLiteral("Shift"), inner);
    return sendGridEdit(deck, edit);
}

int BridgeClient::snapGrid(const QString &deck, qlonglong frame)
{
    QJsonObject inner;
    inner.insert(QStringLiteral("frame"), QJsonValue::fromVariant(frame));
    QJsonObject edit;
    edit.insert(QStringLiteral("Here"), inner);
    return sendGridEdit(deck, edit);
}

int BridgeClient::setGridTempo(const QString &deck, double bpm)
{
    if (!(bpm > 0.0)) {
        return -1;
    }
    QJsonObject inner;
    inner.insert(QStringLiteral("bpm"), bpm);
    QJsonObject edit;
    edit.insert(QStringLiteral("Tempo"), inner);
    return sendGridEdit(deck, edit);
}

int BridgeClient::doubleGrid(const QString &deck)
{
    return sendGridEdit(deck, QJsonValue(QStringLiteral("Double")));
}

int BridgeClient::halveGrid(const QString &deck)
{
    return sendGridEdit(deck, QJsonValue(QStringLiteral("Half")));
}

int BridgeClient::eject(const QString &deck)
{
    QJsonObject options;
    options.insert(QStringLiteral("deck"), deck);
    QJsonObject command;
    command.insert(QStringLiteral("Eject"), options);
    m_deckPaths.remove(deck);
    return sendCommand(deck, command);
}

int BridgeClient::seek(const QString &deck, qulonglong frame)
{
    QJsonObject options;
    options.insert(QStringLiteral("deck"), deck);
    options.insert(QStringLiteral("frame"), static_cast<double>(frame));
    QJsonObject command;
    command.insert(QStringLiteral("SeekFrames"), options);
    return sendCommand(deck, command);
}

int BridgeClient::setLoop(const QString &deck, bool enabled, qulonglong start, qulonglong end)
{
    QJsonObject options;
    options.insert(QStringLiteral("deck"), deck);
    if (enabled) {
        QJsonObject region;
        region.insert(QStringLiteral("start"), static_cast<double>(start));
        region.insert(QStringLiteral("end"), static_cast<double>(end));
        options.insert(QStringLiteral("region"), region);
    } else {
        options.insert(QStringLiteral("region"), QJsonValue::Null);
    }
    QJsonObject command;
    command.insert(QStringLiteral("SetLoop"), options);
    return sendCommand(deck, command);
}

int BridgeClient::loopBeats(const QString &deck, int beats)
{
    QJsonObject options;
    options.insert(QStringLiteral("deck"), deck);
    options.insert(QStringLiteral("beats"), beats);
    QJsonObject command;
    command.insert(QStringLiteral("LoopBeats"), options);
    return sendCommand(deck, command);
}

int BridgeClient::moveLoop(const QString &deck, int beats)
{
    QJsonObject options;
    options.insert(QStringLiteral("deck"), deck);
    options.insert(QStringLiteral("beats"), beats);
    QJsonObject command;
    command.insert(QStringLiteral("LoopMove"), options);
    return sendCommand(deck, command);
}

int BridgeClient::beatJump(const QString &deck, double beats, bool quantize)
{
    QJsonObject options;
    options.insert(QStringLiteral("deck"), deck);
    options.insert(QStringLiteral("beats"), beats);
    options.insert(QStringLiteral("quantize"), quantize);
    QJsonObject command;
    command.insert(QStringLiteral("BeatJump"), options);
    return sendCommand(deck, command);
}

int BridgeClient::nudge(const QString &deck, double amount)
{
    QJsonObject options;
    options.insert(QStringLiteral("deck"), deck);
    options.insert(QStringLiteral("amount"), amount);
    QJsonObject command;
    command.insert(QStringLiteral("Nudge"), options);
    return sendCommand(deck, command);
}

int BridgeClient::setSlip(const QString &deck, bool enabled)
{
    QJsonObject options;
    options.insert(QStringLiteral("deck"), deck);
    options.insert(QStringLiteral("enabled"), enabled);
    QJsonObject command;
    command.insert(QStringLiteral("SetSlip"), options);
    return sendCommand(deck, command);
}

int BridgeClient::setReverse(const QString &deck, bool enabled)
{
    QJsonObject options;
    options.insert(QStringLiteral("deck"), deck);
    options.insert(QStringLiteral("enabled"), enabled);
    QJsonObject command;
    command.insert(QStringLiteral("SetReverse"), options);
    return sendCommand(deck, command);
}

int BridgeClient::setSync(const QString &deck, const QString &mode)
{
    QJsonObject options;
    options.insert(QStringLiteral("deck"), deck);
    options.insert(QStringLiteral("mode"), mode);
    QJsonObject command;
    command.insert(QStringLiteral("Sync"), options);
    return sendCommand(deck, command);
}

int BridgeClient::setMaster(const QString &deck)
{
    QJsonObject options;
    options.insert(QStringLiteral("deck"), deck);
    QJsonObject command;
    command.insert(QStringLiteral("Master"), options);
    return sendCommand(deck, command);
}

int BridgeClient::clearMaster()
{
    // Nobody leads. The decks that followed stop following and keep playing: a tempo a hand set is a
    // decision, not a state to undo.
    // A command with no fields is a bare name on the wire: serde writes a unit variant as `null`, and a
    // reader that is handed `{}` rejects it as unknown.
    QJsonObject command;
    command.insert(QStringLiteral("MasterOff"), QJsonValue::Null);
    return sendCommand(QString(), command);
}

int BridgeClient::setCrossfader(double position)
{
    QJsonObject options;
    options.insert(QStringLiteral("position"), position);
    QJsonObject command;
    command.insert(QStringLiteral("SetCrossfader"), options);
    return sendCommand(QString(), command);
}

int BridgeClient::setMasterGain(double gain)
{
    QJsonObject options;
    options.insert(QStringLiteral("linear_gain"), gain);
    QJsonObject command;
    command.insert(QStringLiteral("SetMasterGain"), options);
    return sendCommand(QStringLiteral("mixer"), command);
}

int BridgeClient::setCrossfaderCurve(const QString &curve)
{
    QJsonObject options;
    options.insert(QStringLiteral("curve"), curve);
    QJsonObject command;
    command.insert(QStringLiteral("SetCrossfaderCurve"), options);
    return sendCommand(QStringLiteral("mixer"), command);
}

int BridgeClient::setChannelGain(const QString &deck, double gain)
{
    QJsonObject options;
    options.insert(QStringLiteral("deck"), deck);
    options.insert(QStringLiteral("linear_gain"), gain);
    QJsonObject command;
    command.insert(QStringLiteral("SetChannelGain"), options);
    return sendCommand(deck, command);
}

int BridgeClient::setTempo(const QString &deck, double ratio)
{
    QJsonObject options;
    options.insert(QStringLiteral("deck"), deck);
    options.insert(QStringLiteral("ratio"), ratio);
    QJsonObject command;
    command.insert(QStringLiteral("SetTempoRatio"), options);
    return sendCommand(deck, command);
}

int BridgeClient::setLoopMode(const QString &deck, bool enabled)
{
    QJsonObject options;
    options.insert(QStringLiteral("deck"), deck);
    options.insert(QStringLiteral("enabled"), enabled);
    QJsonObject command;
    command.insert(QStringLiteral("SetLoopMode"), options);
    return sendCommand(deck, command);
}

int BridgeClient::setPadLoop(const QString &deck, int slot)
{
    QJsonObject options;
    options.insert(QStringLiteral("deck"), deck);
    options.insert(QStringLiteral("slot"), slot);
    QJsonObject command;
    command.insert(QStringLiteral("SetPadLoop"), options);
    return sendCommand(deck, command);
}

int BridgeClient::setCueMeta(const QString &deck, int slot, const QString &label,
                             const QString &color)
{
    QJsonObject options;
    options.insert(QStringLiteral("deck"), deck);
    options.insert(QStringLiteral("slot"), slot);
    // An empty string means "leave this half alone": the two halves of a pad are set independently.
    if (!label.isEmpty()) {
        options.insert(QStringLiteral("label"), label);
    }
    if (!color.isEmpty()) {
        options.insert(QStringLiteral("color"), color);
    }
    QJsonObject command;
    command.insert(QStringLiteral("SetCueMeta"), options);
    return sendCommand(deck, command);
}

int BridgeClient::setChannelFader(const QString &deck, double position)
{
    QJsonObject options;
    options.insert(QStringLiteral("deck"), deck);
    options.insert(QStringLiteral("position"), position);
    QJsonObject command;
    command.insert(QStringLiteral("SetChannelFader"), options);
    return sendCommand(deck, command);
}

int BridgeClient::setChannelEq(const QString &deck, const QString &band, double gainDb)
{
    QJsonObject options;
    options.insert(QStringLiteral("deck"), deck);
    options.insert(QStringLiteral("band"), band);
    options.insert(QStringLiteral("gain_db"), gainDb);
    QJsonObject command;
    command.insert(QStringLiteral("SetChannelEq"), options);
    return sendCommand(deck, command);
}

BridgeClient::~BridgeClient()
{
    // Before the members go: a socket that is still open emits disconnected() from its own destructor,
    // and the slot would then touch a client mid-destruction and emit into a QML scene that has already
    // been torn down. That is the heap corruption seen when the window is closed right after the start.
    m_shuttingDown = true;
    disconnect(&m_socket, nullptr, this, nullptr);
    m_socket.abort();
}

void BridgeClient::setStatus(const QString &status)
{
    if (m_shuttingDown || m_status == status) {
        return;
    }
    m_status = status;
    emit statusChanged();
}

void BridgeClient::setConnected(bool connected)
{
    if (m_shuttingDown || m_connected == connected) {
        return;
    }
    m_connected = connected;
    emit connectedChanged();
}
