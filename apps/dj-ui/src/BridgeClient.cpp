#include "BridgeClient.h"

#include "PathRules.h"

#include <QDateTime>

#include <QCoreApplication>
#include <QElapsedTimer>
#include <QThread>

#include <QDebug>
#include <QDir>
#include <QDirIterator>
#include <QFile>
#include <QFileInfo>
#include <QGuiApplication>
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
    // Either separator: on Windows this path is spelled with backslashes, and a split on `/` alone never found the
    // folder it was looking for (owner's machine, 2026-11-19).
    return pathrules::insideDuplicates(directory);
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
    // The home folder, not a guess at where music lives: the DJ's files are arranged his own way, and a
    // list that opens in an empty `~/Music` looks broken. A remembered folder still wins over this.
    return QDir::homePath();
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
    switchMode(QStringLiteral("files"));
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

/// Where the shell writes its log, next to the engine's own.
QString uiLogPath()
{
    // Built by QDir, not by a slash: the same line on every platform, spelled the way the platform spells it.
    return QDir(QFileInfo(defaultCollectionRoot()).absolutePath()).filePath(QStringLiteral("logs/dj-ui.log"));
}

namespace {

/// How much detail the log keeps. `HAULER_LOG` chooses, as it does for the engine.
int uiLevel()
{
    static const int level = [] {
        const QString name = qEnvironmentVariable("HAULER_LOG").trimmed().toLower();
        if (name == QLatin1String("error")) {
            return 0;
        }
        if (name == QLatin1String("warn") || name == QLatin1String("warning")) {
            return 1;
        }
        if (name == QLatin1String("debug")) {
            return 3;
        }
        if (name == QLatin1String("trace")) {
            return 4;
        }
        return 2;
    }();
    return level;
}

/// Rotate at a fixed size: a session can run for hours at trace, and a log that fills the disk is its own incident.
constexpr qint64 kLogMaxBytes = 8 * 1024 * 1024;

} // namespace

/// One line from the shell, with the time it happened: stderr for a terminal that is there, a file for a
/// launcher on Windows that opens none. `level` is 0 error, 1 warn, 2 info, 3 debug, 4 trace.
void uiLogAt(int level, const QString &message)
{
    if (level > uiLevel()) {
        return;
    }
    static const char *tags[] = {"ERROR", "WARN", "info", "debug", "trace"};
    const QString tag = QString::fromLatin1(tags[qBound(0, level, 4)]);
    const QString stamp = QString::number(QDateTime::currentMSecsSinceEpoch());
    const QString line = QStringLiteral("%1 %2 dj-ui: %3").arg(stamp, tag, message);
    std::fprintf(stderr, "%s\n", qPrintable(line));
    const QString path = uiLogPath();
    QDir().mkpath(QFileInfo(path).absolutePath());
    const QFileInfo info(path);
    if (info.exists() && info.size() > kLogMaxBytes) {
        QFile::remove(path + QStringLiteral(".1"));
        QFile::rename(path, path + QStringLiteral(".1"));
    }
    QFile file(path);
    if (file.open(QIODevice::WriteOnly | QIODevice::Append | QIODevice::Text)) {
        file.write(line.toUtf8() + '\n');
    }
}

void uiLog(const QString &message)
{
    uiLogAt(2, message);
}

QString defaultCollectionRoot()
{
#ifdef Q_OS_WIN
    QString base = qEnvironmentVariable("APPDATA");
    if (base.isEmpty()) {
        base = QStandardPaths::writableLocation(QStandardPaths::GenericDataLocation);
    }
#else
    QString base = qEnvironmentVariable("XDG_DATA_HOME");
    if (base.isEmpty() || !QFileInfo(base).isAbsolute()) {
        base = QDir::homePath() + QStringLiteral("/.local/share");
    }
#endif
    // Spelled the way the platform spells paths: the engine compares roots with separators, and a root that
    // reads `C:\users\...\Roaming/hauler-dj/collection` invites a comparison that misses.
    return QDir::toNativeSeparators(QDir(base).filePath(QStringLiteral("hauler-dj/collection")));
}

QString FileBrowser::defaultCollectionRoot() const
{
    // One collection, one place, and a copy of the collection carries its .djmeta with it. The free function is
    // named with `::` on purpose: without it the name finds this very member and the call never returns.
    return ::defaultCollectionRoot();
}

QString FileBrowser::cursor() const
{
    if (m_mode == QLatin1String("playlists")) {
        return m_playlistsCursor;
    }
    return m_mode == QLatin1String("collection") ? m_collectionCursor : m_filesCursor;
}

void FileBrowser::setCursor(const QString &path)
{
    QString &remembered = m_mode == QLatin1String("playlists")
                              ? m_playlistsCursor
                              : (m_mode == QLatin1String("collection") ? m_collectionCursor
                                                                       : m_filesCursor);
    if (remembered == path) {
        return;
    }
    remembered = path;
    emit cursorChanged();
}

void FileBrowser::switchMode(const QString &mode)
{
    const QString before = cursor();
    m_mode = mode;
    if (cursor() != before) {
        emit cursorChanged();
    }
}

void FileBrowser::seedCursors(const QString &files, const QString &collection)
{
    m_filesCursor = files;
    m_collectionCursor = collection;
}

bool FileBrowser::revealRailPlace(const QString &place)
{
    if (place.isEmpty()) {
        return false;
    }
    if (place == QLatin1String("all")) {
        if (!m_expanded.contains(QStringLiteral("all"))) {
            m_expanded.append(QStringLiteral("all"));
            rebuildPlaylistTree();
        }
        return true;
    }
    QString folder;
    if (place.startsWith(QLatin1String("folder:"))) {
        folder = place.mid(7);
    } else if (place.startsWith(QLatin1String("playlist:"))) {
        // A playlist shows under its folder: the engine's last tree says which one holds it.
        const qlonglong id = place.mid(9).toLongLong();
        bool known = false;
        const QVariantList playlists = m_engineTree.value(QStringLiteral("playlists")).toList();
        for (const QVariant &item : playlists) {
            const QVariantMap found = item.toMap();
            if (found.value(QStringLiteral("id")).toLongLong() == id) {
                folder = found.value(QStringLiteral("folder")).toString();
                known = true;
                break;
            }
        }
        if (!known) {
            return false;
        }
    } else {
        return false;
    }
    if (folder.isEmpty()) {
        // A root playlist stands in the open: no folder to unfold.
        return true;
    }
    // Every ancestor has to be open for the node itself to be on show, and a playlist needs the folder it lives in
    // open as well — the leaf is a child of that node, not of its parent. A folder's own children stay as the DJ
    // left them.
    const bool leafInside = place.startsWith(QLatin1String("playlist:"));
    bool opened = false;
    const QStringList parts = folder.split(QLatin1Char('/'));
    QString path;
    const int depth = leafInside ? parts.size() : parts.size() - 1;
    for (int index = 0; index < depth; ++index) {
        path = path.isEmpty() ? parts.at(index) : path + QLatin1Char('/') + parts.at(index);
        const QString key = QStringLiteral("folder:") + path;
        if (!m_expanded.contains(key)) {
            m_expanded.append(key);
            opened = true;
        }
    }
    if (opened) {
        rebuildPlaylistTree();
    }
    return true;
}

int FileBrowser::rowOf(const QString &path) const
{
    // The row is counted in the list the DJ is looking at: a playlist's rows are its own, and a path
    // looked for among the collection's rows would never be found there (or worse, found at another
    // place).
    const QVariantList &rows = m_mode == QLatin1String("playlists") ? m_playlistRows : m_entries;
    for (int index = 0; index < rows.size(); ++index) {
        if (rows.at(index).toMap().value(QStringLiteral("path")).toString() == path) {
            return index;
        }
    }
    return -1;
}

/// How long a page question may stay unanswered before it is asked again, in milliseconds.
constexpr int kAnswerWatchdogMs = 1500;
/// How many times a question is asked again before its silence is accepted and written to the log.
constexpr int kAnswerWatchdogTries = 3;

/// How many times an empty answer to a page question is asked again before it is taken as the truth.
///
/// Four asks over roughly six seconds: shorter than a DJ's patience with a list that is filling, longer than the
/// scan of a folder that was just imported. After that the empty answer is the collection's own answer.
constexpr int EMPTY_ANSWER_RETRIES = 4;

void FileBrowser::askAgainAfterEmpty()
{
    ++m_emptyRetries;
    // 400, 800, 1600, 3200 ms: an index that is building finishes inside this, and the last answer is taken as final.
    const int delay = 400 << (m_emptyRetries - 1);
    QTimer::singleShot(delay, this, [this]() { askPage(0); });
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
    // A saved root keeps the spelling it was written with, and a root written by an older build (`C:/…`) does not
    // compare equal to the engine's own paths (`C:\…`): the page then answers nothing while the files are right
    // there. Every root is spelled the way this platform spells paths before it is used or sent anywhere.
    QStringList spelled;
    for (const QString &root : roots) {
        const QString one = pathrules::spelled(root);
        if (!one.isEmpty() && !spelled.contains(one)) {
            spelled.append(one);
        }
    }
    QStringList listed = spelled.isEmpty() ? QStringList{defaultCollectionRoot()} : spelled;
    // A root that is no longer there is not a root: a temporary folder or a moved disk leaves a path that
    // can never list anything, and the DJ would see an empty collection with no way to know why. What
    // survives stays; when nothing survives, the default root takes over.
    {
        QStringList alive;
        for (const QString &root : listed) {
            if (QFileInfo::exists(root)) {
                alive.append(root);
            }
        }
        if (alive.isEmpty()) {
            alive.append(defaultCollectionRoot());
        }
        if (alive != listed) {
            uiLog(QStringLiteral("dropped %1 roots that are gone, opening %2")
                      .arg(listed.size() - alive.size())
                      .arg(alive.size()));
        }
        listed = alive;
    }
    // Same folders, same list: a refresh after an analysis must not blank the list while the answer is
    // on its way — the DJ sees his collection vanish and come back for no reason.
    const bool sameRoots = listed == m_roots;
    m_replacingInPlace = sameRoots;
    m_roots = listed;
    m_folder = listed.join(QStringLiteral(" | "));
    switchMode(QStringLiteral("collection"));
    m_pageOffset = 0;
    m_pageTotal = 0;
    // An answer that was already on its way belongs to the list that is being replaced. Holding it to the
    // number of the question that has just been asked is what keeps it out: without the bump it carries a
    // current id, lands after this clearing, and the fresh list stays empty because nothing asks again.
    ++m_pageQuery;
    m_pageAnsweredFor = 0;
    m_askedSignature.clear();
    m_genreList.clear();
    m_genreFallback.clear();
    m_censusAsked = false;
    m_emptyRetries = 0;
    m_missingAnswers = 0;
    // The interface does not walk the disks: the walk is how the database is prepared, not how it is
    // read. An empty answer here is an empty collection, and it is shown as such.
    if (!sameRoots) {
        setPaged(false);
        m_all.clear();
        m_total = 0;
    }
    // The section changes even when the roots do not: without this, coming back from Files leaves the
    // shell on the old section, which is exactly what the DJ saw.
    applyFilter();
    emit changed();
    if (m_bridge != nullptr && m_bridge->isConnected()) {
        // A line per opening: with two rounds of "nothing changed" it is worth knowing whether the shell
        // asked with a filter at all.
        uiLog(QStringLiteral("collection opened, %1 roots, %2 genres chosen")
                  .arg(m_roots.size())
                  .arg(m_genres.size()));
        // The genres are asked once per opening, whatever the filter: their answer travels in its own slot,
        // so it neither waits for the page nor erases it.
        if (!m_censusAsked) {
            m_censusAsked = true;
            askCensus();
        }
        askPage(0);
    }
}

QVariantList FileBrowser::rowsOf(const QVariantList &rows) const
{
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
        // When the track was last played, when the index knows: the column that marks a track as "played
        // recently" is decided on the row, against the window the engine publishes (DJP-MEDIA-004).
        const QVariant played = row.value(QStringLiteral("last_played_ms"));
        if (!played.isNull()) {
            made.insert(QStringLiteral("last_played_ms"), played);
        }
        const double bpm = row.value(QStringLiteral("bpm")).toDouble();
        if (bpm > 0.0) {
            made.insert(QStringLiteral("bpm"), bpm);
        }
        const QString musicalKey = row.value(QStringLiteral("musical_key")).toString();
        if (!musicalKey.isEmpty()) {
            made.insert(QStringLiteral("key"), musicalKey);
        }
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
    return tracks;
}

void FileBrowser::setPaged(bool paged)
{
    if (m_paged == paged) {
        return;
    }
    m_paged = paged;
    emit pagedChanged();
}

namespace {

/// The protocol's name for a column the settings call something else.
QString wireColumn(const QString &id)
{
    static const QHash<QString, QString> names{
        {QStringLiteral("artist"), QStringLiteral("Artist")},
        {QStringLiteral("album"), QStringLiteral("Album")},
        {QStringLiteral("title"), QStringLiteral("Title")},
        {QStringLiteral("bpm"), QStringLiteral("Bpm")},
        {QStringLiteral("key"), QStringLiteral("Key")},
        {QStringLiteral("time"), QStringLiteral("Time")},
    };
    return names.value(id, QString());
}

} // namespace

QString FileBrowser::pageSignature() const
{
    // What a list is, spelled out: an answer to a question with this same signature lists the same rows, whatever
    // else the shell has done in the meantime, and however many rows that question asked for.
    QString signature = m_filter + QLatin1Char('|') + m_genres.join(QLatin1Char(','))
        + QLatin1Char('|') + m_roots.join(QLatin1Char(','));
    for (const QVariant &entry : m_sort) {
        const QVariantMap key = entry.toMap();
        signature += key.value(QStringLiteral("column")).toString()
            + (key.value(QStringLiteral("ascending")).toBool() ? QStringLiteral("+") : QStringLiteral("-"))
            + QLatin1Char(',');
    }
    return signature;
}

void FileBrowser::askPage(int offset, int limit)
{
    if (m_bridge == nullptr) {
        return;
    }
    // The list is read whole. Paging was a courtesy to a library that might never be scrolled, but it cost
    // the DJ the end of the list: End stopped on the last row that happened to be loaded, and a run taken
    // with Shift covered only what was in hand. The total is known after the first answer, so the first
    // question asks for one page and every question after it asks for everything.
    if (limit <= 0) {
        limit = offset == 0 ? qMax(m_pageTotal, 200) : 200;
    }
    // The same question, still unanswered: say nothing. A panel that re-asks on every progress tick
    // floods the engine and makes the status flicker between "waiting for the engine" and ready.
    const QString signature = pageSignature();
    if (signature == m_askedSignature && m_pageAnsweredFor != m_pageQuery) {
        return;
    }
    if (signature != m_askedSignature) {
        // A new question: the previous one may have asked for the whole list under other filters, and that
        // says nothing about this one.
        m_wholeAsked = false;
    }
    m_askedSignature = signature;
    const int id = ++m_pageQuery;
    // Every question is remembered by its number: the answer carries the number, and the number alone says nothing
    // about what was asked.
    m_askedSignatures.insert(id, signature);
    while (m_askedSignatures.size() > 8) {
        auto oldest = m_askedSignatures.begin();
        for (auto it = m_askedSignatures.begin(); it != m_askedSignatures.end(); ++it) {
            if (it.key() < oldest.key()) {
                oldest = it;
            }
        }
        m_askedSignatures.erase(oldest);
    }
    m_droppedAnswers.remove(id);
    sendQuestion(id, offset, limit);
}

/// Sends one page question and arranges for its answer to be watched.
///
/// Separate from [`askPage`] because a question that is never answered has to be asked again with a new number: the
/// dedupe above deliberately refuses to repeat a question that is still outstanding, which is right for a panel that
/// re-asks on every tick and wrong for an answer that was lost.
void FileBrowser::sendQuestion(int id, int offset, int limit)
{
    // Every question is written down by its number before it leaves, this one included: the answer is matched by the
    // number and the signature together, and a question the book does not know is answered into the void.
    const QString signature = pageSignature();
    m_askedSignature = signature;
    m_askedSignatures.insert(id, signature);
    while (m_askedSignatures.size() > 8) {
        auto oldest = m_askedSignatures.begin();
        for (auto it = m_askedSignatures.begin(); it != m_askedSignatures.end(); ++it) {
            if (it.key() < oldest.key()) {
                oldest = it;
            }
        }
        m_askedSignatures.erase(oldest);
    }
    m_droppedAnswers.remove(id);
    QVariantMap query;
    query.insert(QStringLiteral("id"), id);
    QVariantList order;
    for (const QVariant &entry : m_sort) {
        const QVariantMap key = entry.toMap();
        QVariantMap asked;
        // The protocol names the columns as the enum does — `Artist`, `Bpm`, `Key` — and the list names
        // them as the settings do — `artist`, `bpm`, `key`. Sending the settings' spelling made the wire
        // refuse the question, so a sorted page was never asked for and the list only ever moved locally.
        asked.insert(QStringLiteral("column"), wireColumn(key.value(QStringLiteral("column")).toString()));
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
    query.insert(QStringLiteral("limit"), limit);
    m_askedOffset = offset;
    m_askedLimit = limit;
    m_bridge->queryCollection(query);
    // An answer that never comes leaves the list empty with nothing to click, and silence is not an answer the
    // shell can see. The engine answers on its own thread and drops an answer it cannot deliver, so a lost answer
    // is a real possibility (measured, 22.11: question 20 asked, question 18's answer dropped as stale, and the
    // list held nothing half a minute later). The watchdog asks the same question again — at most three times —
    // and then says in the log that the silence stood.
    const int watched = id;
    QTimer::singleShot(kAnswerWatchdogMs, this, [this, watched]() { watchAnswer(watched); });
}

void FileBrowser::watchAnswer(int id)
{
    if (id != m_pageQuery || m_pageAnsweredFor == id || m_bridge == nullptr) {
        return;
    }
    if (m_missingAnswers >= kAnswerWatchdogTries) {
        uiLog(QStringLiteral("question %1 was never answered; the list keeps its last answer (%2 rows)")
                  .arg(id)
                  .arg(m_all.size()));
        return;
    }
    ++m_missingAnswers;
    uiLog(QStringLiteral("question %1 was not answered in time; asking again (%2 of %3)")
              .arg(id)
              .arg(m_missingAnswers)
              .arg(kAnswerWatchdogTries));
    sendQuestion(++m_pageQuery, m_askedOffset, m_askedLimit);
}

void FileBrowser::askCensus()
{
    if (m_bridge == nullptr) {
        return;
    }
    // A question of its own, never mixed with a page: the engine answers it from the genre table into the
    // snapshot's own slot, so a page and a count cannot overwrite each other.
    QVariantMap query;
    // An id no page question reaches: page ids count up from one. It cannot be negative — the wire's id is
    // an unsigned number, and a message carrying -1 is refused whole, connection and all: the list then
    // sits empty with no error to show for it.
    query.insert(QStringLiteral("id"), 2000000000);
    query.insert(QStringLiteral("genres_only"), true);
    query.insert(QStringLiteral("roots"), m_roots);
    query.insert(QStringLiteral("offset"), 0);
    query.insert(QStringLiteral("limit"), 0);
    m_bridge->queryCollection(query);
}

void FileBrowser::takeGenres()
{
    if (m_bridge == nullptr || m_mode != QLatin1String("collection")) {
        return;
    }
    // The genres come as their own answer, in the snapshot's own slot: name and count, straight from the
    // index's genre table.
    const QVariantList counted = m_bridge->snapshot().value(QStringLiteral("genres")).toList();
    if (counted.isEmpty()) {
        return;
    }
    QVariantList boxes;
    for (const QVariant &entry : counted) {
        boxes.append(entry.toMap());
    }
    if (boxes == m_genreList) {
        return;
    }
    uiLog(QStringLiteral("the index knows %1 genres").arg(boxes.size()));
    m_genreList = boxes;
    publishGenres();
    // A genre restored from the layout may not be in this collection any more, or may now have no tracks:
    // keeping it filters the list to nothing, and the DJ sees an empty collection with a box ticked and no
    // idea why. The boxes are the truth, so a choice they do not offer is dropped and the list is asked
    // again without it.
    if (!m_genres.isEmpty()) {
        QStringList kept;
        for (const QString &chosen : std::as_const(m_genres)) {
            for (const QVariant &entry : std::as_const(boxes)) {
                const QVariantMap box = entry.toMap();
                if (box.value(QStringLiteral("name")).toString().compare(chosen, Qt::CaseInsensitive) == 0
                    && box.value(QStringLiteral("count")).toInt() > 0) {
                    kept.append(chosen);
                    break;
                }
            }
        }
        if (kept != m_genres) {
            uiLog(QStringLiteral("dropped genre filters the collection no longer offers"));
            m_genres = kept;
            emit genresChanged();
            applyFilter();
            askPage(0);
        }
    }
}

void FileBrowser::publishGenres()
{
    const QVariantList boxes = genreList();
    if (boxes == m_publishedGenres) {
        return;
    }
    m_publishedGenres = boxes;
    emit genresChanged();
}

void FileBrowser::loadMore()
{
    if (!m_paged || m_bridge == nullptr || m_pageOffset <= 0 || m_pageOffset >= m_pageTotal) {
        return;
    }
    // A list that is still waiting for its first page has nothing to continue: the view calls for more as
    // soon as it is at an end, which an empty list always is. Asking would outbid the first question, and
    // the answer that was already on its way would then be an answer to a question the shell had dropped.
    if (m_pageQuery != m_pageAnsweredFor) {
        return;
    }
    askPage(m_pageOffset);
}

namespace {

/// A string as the engine reads it: a null `QString` becomes JSON `null`, and the engine's op wants a
/// string — a folder of `""` is the root, a folder of `null` is a rejected command with no visible reason.
/// Every text going into an op passes through here.
QString spoken(const QString &value)
{
    return value.isNull() ? QStringLiteral("") : value;
}

/// A row of a collection page as the list draws it.
///
/// One shape for the collection and for a playlist (DJP-MEDIA-004): the table that draws the first
/// draws the second, with the same cells, the same tempo and the same key — a second delegate would be
/// a second list to keep in step.
QVariantMap rowFromPage(const QVariantMap &row)
{
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
    // When the track was last played, when the index knows: the column that marks a track as "played
    // recently" is decided on the row, against the window the engine publishes (DJP-MEDIA-004).
    const QVariant played = row.value(QStringLiteral("last_played_ms"));
    if (!played.isNull()) {
        made.insert(QStringLiteral("last_played_ms"), played);
    }
    // The tempo and the key the index holds: the table reads `bpm` and `key`, and a page that carried
    // only names would show empty cells for tracks the deck plays with both known.
    const double bpm = row.value(QStringLiteral("bpm")).toDouble();
    if (bpm > 0.0) {
        made.insert(QStringLiteral("bpm"), bpm);
    }
    const QString musicalKey = row.value(QStringLiteral("musical_key")).toString();
    if (!musicalKey.isEmpty()) {
        made.insert(QStringLiteral("key"), musicalKey);
    }
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
    // The list draws a lock beside the number when this track's grid is locked.
    made.insert(QStringLiteral("analysis_lock"),
                row.value(QStringLiteral("analysis_lock")).toBool());
    // And it draws the tempo in another colour when the grid holds more than one: the row arrives as a page, and
    // everything this function does not copy is a field the list can never see (owner's report, 26.11: the highlight
    // never appeared although the record and the page both said `variable_bpm`).
    made.insert(QStringLiteral("variable_bpm"),
                row.value(QStringLiteral("variable_bpm")).toBool());
    return made;
}

} // namespace

void FileBrowser::takePage()
{
    if (m_bridge == nullptr || m_mode != QLatin1String("collection")) {
        return;
    }
    // The entry is logged before anything is read: a crash that leaves the journal without this line did not happen
    // here, and one that leaves it as the last line happened in the reading.
    uiLogAt(4, QStringLiteral("page seen: asked %1, answered %2")
                   .arg(m_pageQuery)
                   .arg(m_pageAnsweredFor));
    const QVariantMap answer = m_bridge->collectionPage();
    const int id = answer.value(QStringLiteral("id")).toInt();
    if (answer.isEmpty()) {
        // The page is missing from the snapshot itself, and that is a different fault from an answer that carries
        // the wrong number: the engine may have answered, and the shell never saw it. The keys of the snapshot say
        // what did arrive — once a second, so a stream of identical snapshots stays readable.
        static qint64 lastComplaint = 0;
        const qint64 now = QDateTime::currentMSecsSinceEpoch();
        if (now - lastComplaint > 1000) {
            lastComplaint = now;
            uiLogAt(1, QStringLiteral("the snapshot carries no page: keys [%1], revision %2")
                           .arg(m_bridge->snapshot().keys().join(QStringLiteral(", ")))
                           .arg(m_bridge->snapshot().value(QStringLiteral("revision")).toInt()));
        }
        // The list is empty on the screen and the page is not in the snapshot: ask again, once a second, rather than
        // sit in front of nothing. The dedupe below would refuse a repeat of the same question, so the signature is
        // cleared first — this is not a duplicate, it is a question that was never answered in what came back.
        if (now - m_lastRepairAsk > 1000) {
            m_lastRepairAsk = now;
            m_askedSignature.clear();
            uiLogAt(2, QStringLiteral("the list has no page: asking again (question %1)").arg(m_pageQuery + 1));
            QTimer::singleShot(0, this, [this]() { askPage(0); });
        }
        return;
    }
    // An answer is taken when it belongs to the list on screen, and "belongs" is a question of what was asked, not
    // of which number is the highest. The shell re-opens the section on several notifications, each opening asks
    // again, and the engine answers the questions it was given in order — so an answer carrying an older number was
    // thrown away while the newest one was still on its way, and the list stayed empty over a correct answer. What
    // decides is the signature: same roots, same filter, same genres, same order.
    const QString askedFor = m_askedSignatures.value(id);
    const bool sameQuestion = !askedFor.isEmpty() && askedFor == pageSignature();
    if (id == 2000000000 || id == m_pageAnsweredFor || !sameQuestion) {
        // Why an answer was not taken: the numbers are the whole story, and a list that stays empty is
        // otherwise indistinguishable from a question that was never answered. Once per answer, not once per
        // snapshot: the engine republishes the same slot twenty times a second, and a log of that says nothing.
        const bool once = !m_droppedAnswers.contains(id);
        m_droppedAnswers.insert(id);
        if (id != 0 && id != m_pageAnsweredFor && once) {
            uiLog(QStringLiteral("answer %1 dropped: asked %2, already had %3, mode %4, same question %5")
                      .arg(id)
                      .arg(m_pageQuery)
                      .arg(m_pageAnsweredFor)
                      .arg(m_mode)
                      .arg(sameQuestion ? QStringLiteral("yes") : QStringLiteral("no")));
        }
        return;
    }
    m_pageAnsweredFor = id;
    const QVariantList rows = answer.value(QStringLiteral("rows")).toList();
    const int total = answer.value(QStringLiteral("total")).toInt();
    // The answer as the list received it: a question in the log without an answer next to it says the
    // engine stayed silent, and an answer with no rows says the index is empty for those folders.
    uiLog(QStringLiteral("answer %1: %2 rows of %3, mode %4, roots [%5], filter \"%6\"")
              .arg(id)
              .arg(rows.size())
              .arg(total)
              .arg(m_mode)
              .arg(m_roots.join(QStringLiteral(" | ")), m_filter));
    // The page does not carry its offset back: the question does, and the id says which question.
    const int offset = m_askedOffset;
    if (offset == 0 && rows.isEmpty() && total == 0) {
        // The index is the source: it answered, and the answer is "nothing under these roots". The list is
        // emptied here rather than left as it was — a stale list from the previous section is not an answer —
        // and the one retry runs: the index may be building itself from the folders at this very moment.
        setPaged(true);
        m_all.clear();
        m_pageOffset = 0;
        m_pageTotal = 0;
        m_total = 0;
        applyFilter();
        publishGenres();
        if (m_emptyRetries < EMPTY_ANSWER_RETRIES && !m_roots.isEmpty()) {
            askAgainAfterEmpty();
        }
        return;
    }
    QVariantList tracks;
    for (const QVariant &entry : rows) {
        tracks.append(rowFromPage(entry.toMap()));
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
        // The whole list was replaced, and a view handed a new model starts at the top: the count says so, and
        // the shell puts the DJ's place back. Only a refresh of the section already on screen has a place to
        // put back — a section being opened puts its remembered row in the middle instead.
        if (m_replacingInPlace) {
            ++m_pageGeneration;
        }
    } else {
        if (tracks.isEmpty()) {
            m_pageTotal = m_pageOffset;
            return;
        }
        m_all += tracks;
        m_pageOffset += tracks.size();
        m_pageTotal = total;
    }
    // The census taken from rows in hand is only good while nothing is filtered: the rows in hand are the
    // rows the filter kept, and a genre missing from it is a box the DJ cannot untick.
    if (m_genres.isEmpty() && m_genreList.isEmpty()) {
        m_genreFallback = censusFromRows();
    }
    applyFilter();
    refreshLocked();
    if (!at.isEmpty() && rowOf(at) >= 0) {
        setCursor(at);
    }
    // The boxes are republished only when they say something new: a page answer usually says nothing about the
    // genre list, and rebuilding it on every answer destroys the delegates under the DJ's finger.
    publishGenres();
    // The census question is asked once, and it can be answered before the index has a single row (it
    // answers zero). A page that came back with rows is the proof the index is alive: ask again, once per
    // page, until it answers with genres.
    if (m_genreList.isEmpty() && m_censusAsked) {
        m_censusAsked = false;
        askCensus();
    }
    // The first question of a list asked for one page, because the total was not known when it went out. Now
    // it is: ask again for all of it, once. A DJ who opens a collection sees every row of it, and End has an
    // end to stand on.
    if (offset == 0 && m_pageOffset > 0 && m_pageOffset < m_pageTotal && !m_wholeAsked
        && m_askedLimit < m_pageTotal) {
        m_wholeAsked = true;
        askPage(0, m_pageTotal);
    }
    if (offset == 0 && tracks.isEmpty() && m_emptyRetries < EMPTY_ANSWER_RETRIES && !m_roots.isEmpty()) {
        // The index can be empty on the first question because it is being built from the folders at that very
        // moment. Retrying once turned "an empty collection" into "a collection that took a moment" — but an index
        // that is still building answers the retry empty as well, and then the list stayed empty until someone
        // pressed refresh (measured, 22.11). The ladder below is the honest version: a few asks, each further
        // apart, and then the answer stands.
        askAgainAfterEmpty();
    }
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
        connect(bridge, &BridgeClient::snapshotChanged, this, &FileBrowser::takeGenres);
        connect(bridge, &BridgeClient::snapshotChanged, this, &FileBrowser::takePlaylists);
        // The section can be chosen before the handshake finishes — the shell opens on Files, but a DJ who
        // clicks Collection at once would get a list that never asked anything and never fills. When the
        // engine comes up, the open collection asks again.
        connect(bridge, &BridgeClient::connectedChanged, this, [this, bridge]() {
            if (bridge->isConnected() && m_mode == QLatin1String("collection") && m_all.isEmpty()
                && !m_roots.isEmpty()) {
                m_censusAsked = false;
                askCensus();
                askPage(0);
            }
        });
    }
}

void FileBrowser::rebuildPlaylistTree()
{
    if (m_bridge == nullptr) {
        return;
    }
    // The tree, as the owner asked for it: `All` first, then the top-level folders, then the playlists
    // that sit in no folder. Nothing is unfolded on the way in — a node shows what it holds when the DJ
    // clicks it, one level at a time (owner's decision, 15.11.2026). The tree comes from the engine's
    // last answer, kept by `takePlaylists`, so a toggle needs no round trip.
    const QVariantMap tree = m_engineTree;
    const QVariantList folders = tree.value(QStringLiteral("folders")).toList();
    const QVariantList playlists = tree.value(QStringLiteral("playlists")).toList();
    QVariantList flat;
    auto leaf = [](const QVariantMap &playlist, int depth) {
        QVariantMap row = playlist;
        row.insert(QStringLiteral("kind"), QStringLiteral("playlist"));
        row.insert(QStringLiteral("depth"), depth);
        row.insert(QStringLiteral("open"), false);
        return row;
    };
    auto node = [](const QString &kind, const QString &name, const QString &folder, int depth,
                   bool open) {
        QVariantMap row;
        row.insert(QStringLiteral("kind"), kind);
        row.insert(QStringLiteral("name"), name);
        row.insert(QStringLiteral("folder"), folder);
        row.insert(QStringLiteral("id"), static_cast<qlonglong>(0));
        row.insert(QStringLiteral("depth"), depth);
        row.insert(QStringLiteral("tracks"), 0);
        row.insert(QStringLiteral("open"), open);
        return row;
    };
    const auto isOpen = [&](const QString &key) { return m_expanded.contains(key); };
    const bool allOpen = isOpen(QStringLiteral("all"));
    flat.append(node(QStringLiteral("all"), QStringLiteral("All"), QString(), 0, allOpen));
    // `All` opens one level too: the flat view of every playlist, nothing deeper.
    if (allOpen) {
        for (const QVariant &item : playlists) {
            flat.append(leaf(item.toMap(), 1));
        }
    }

    // The folders, a level at a time: a folder shows what it holds only when the DJ opens it, and a folder
    // inside it waits for its own click — the tree never unfolds itself (owner's decision, 15.11.2026).
    std::function<void(const QString &, int)> emitFolder = [&](const QString &path, int depth) {
        const bool open = isOpen(QStringLiteral("folder:") + path);
        flat.append(node(QStringLiteral("folder"), path.section(QLatin1Char('/'), -1), path, depth, open));
        if (!open) {
            return;
        }
        for (const QVariant &item : playlists) {
            const QVariantMap found = item.toMap();
            if (found.value(QStringLiteral("folder")).toString() == path) {
                flat.append(leaf(found, depth + 1));
            }
        }
        const QString prefix = path + QLatin1Char('/');
        for (const QVariant &folder : folders) {
            const QString child = folder.toString();
            if (!child.startsWith(prefix) || child.mid(prefix.size()).contains(QLatin1Char('/'))) {
                continue;
            }
            emitFolder(child, depth + 1);
        }
    };
    for (const QVariant &folder : folders) {
        const QString path = folder.toString();
        if (path.isEmpty() || path.contains(QLatin1Char('/'))) {
            continue;  // a folder with a parent is shown by that parent
        }
        emitFolder(path, 0);
    }

    // The playlists with no folder stand in the root, after the folders.
    for (const QVariant &item : playlists) {
        const QVariantMap found = item.toMap();
        if (found.value(QStringLiteral("folder")).toString().isEmpty()) {
            flat.append(leaf(found, 0));
        }
    }

    if (flat != m_playlistTree) {
        m_playlistTree = flat;
        emit playlistsChanged();
    }
}

void FileBrowser::togglePlaylistNode(const QString &kind, const QString &folder)
{
    const QString key = kind == QLatin1String("all") ? QStringLiteral("all")
                                                     : QStringLiteral("folder:") + folder;
    if (m_expanded.contains(key)) {
        m_expanded.removeAll(key);
    } else {
        m_expanded.append(key);
    }
    rebuildPlaylistTree();
}

void FileBrowser::takePlaylists()
{
    if (m_bridge == nullptr) {
        return;
    }
    // The page belongs to the open playlist, and that playlist's row may be hidden in a closed folder: a
    // change is judged by the engine's tree, not by the rows on the screen, or an edit to a closed playlist
    // leaves the list showing the state before it (owner's report, 15.11.2026).
    const QVariantMap engineTree = m_bridge->playlistTree();
    const bool libraryMoved = engineTree != m_engineTree;
    m_engineTree = engineTree;
    rebuildPlaylistTree();
    if (libraryMoved && m_mode == QLatin1String("playlists") && m_playlistId != 0) {
        askPlaylistPage();
    }

    // The page of the playlist the DJ opened: its own cell, and its own marker — an answer to another
    // question is not this playlist's rows.
    const QVariantMap page = m_bridge->playlistPage();
    if (page.isEmpty() || page.value(QStringLiteral("id")).toULongLong() != m_playlistQuestion) {
        return;
    }
    QVariantList tracks;
    for (const QVariant &entry : page.value(QStringLiteral("rows")).toList()) {
        tracks.append(rowFromPage(entry.toMap()));
    }
    const int total = page.value(QStringLiteral("total")).toInt();
    if (tracks != m_playlistRows || total != m_playlistTotal) {
        m_playlistRows = tracks;
        m_playlistTotal = total;
        refreshLocked();
        emit playlistPageChanged();
        // A playlist page is always the one already open: the DJ's place in it is his.
        ++m_pageGeneration;
    }
}

void FileBrowser::askPlaylistPage()
{
    if (m_bridge == nullptr) {
        return;
    }
    ++m_playlistQuestion;
    QVariantMap query;
    query.insert(QStringLiteral("id"), static_cast<qulonglong>(m_playlistQuestion));
    query.insert(QStringLiteral("playlist"), static_cast<qlonglong>(m_playlistId));
    if (!m_filter.trimmed().isEmpty()) {
        query.insert(QStringLiteral("search"), m_filter.trimmed());
    }
    // One wide page: a set is a few dozen rows, and a list that pages under the DJ's hands mid-set is
    // worse than a long one.
    query.insert(QStringLiteral("limit"), 5000);
    m_bridge->queryPlaylist(query);
}

void FileBrowser::showPlaylists()
{
    switchMode(QStringLiteral("playlists"));
    emit changed();
    if (m_playlistName.isEmpty()) {
        m_playlistName = QStringLiteral("All");
    }
    m_engineTree = m_bridge != nullptr ? m_bridge->playlistTree() : QVariantMap();
    rebuildPlaylistTree();
    askPlaylistPage();
}

void FileBrowser::openPlaylist(qint64 id, const QString &name)
{
    if (m_playlistId == id && m_playlistName == name) {
        return;
    }
    m_playlistId = id;
    m_playlistName = name;
    // A cursor from another playlist is not a row of this one.
    m_playlistsCursor.clear();
    m_playlistRows.clear();
    m_playlistTotal = 0;
    emit playlistPageChanged();
    emit playlistChanged();
    // `All` and a folder are places in the tree, not playlists: the right side is for tracks, and there
    // are none until the DJ picks a playlist.
    if (id != 0) {
        askPlaylistPage();
    }
}

void FileBrowser::refreshPlaylist()
{
    if (m_playlistId != 0) {
        askPlaylistPage();
    }
}

void FileBrowser::playlistCreate(const QString &name, const QString &folder)
{
    if (m_bridge == nullptr) {
        return;
    }
    QVariantMap create;
    create.insert(QStringLiteral("name"), name);
    create.insert(QStringLiteral("folder"), spoken(folder));
    QVariantMap op;
    op.insert(QStringLiteral("Create"), create);
    m_bridge->editPlaylists(op);
}

void FileBrowser::playlistRename(qint64 id, const QString &name)
{
    if (m_bridge == nullptr) {
        return;
    }
    QVariantMap rename;
    rename.insert(QStringLiteral("playlist"), static_cast<qlonglong>(id));
    rename.insert(QStringLiteral("name"), name);
    QVariantMap op;
    op.insert(QStringLiteral("Rename"), rename);
    m_bridge->editPlaylists(op);
    if (m_playlistId == id) {
        m_playlistName = name;
        emit playlistChanged();
    }
}

void FileBrowser::playlistDelete(qint64 id)
{
    if (m_bridge == nullptr) {
        return;
    }
    QVariantMap remove;
    remove.insert(QStringLiteral("playlist"), static_cast<qlonglong>(id));
    QVariantMap op;
    op.insert(QStringLiteral("Delete"), remove);
    m_bridge->editPlaylists(op);
    if (m_playlistId == id) {
        openPlaylist(0, QStringLiteral("All"));
    }
}

void FileBrowser::playlistMove(qint64 id, const QString &folder)
{
    if (m_bridge == nullptr) {
        return;
    }
    QVariantMap move;
    move.insert(QStringLiteral("playlist"), static_cast<qlonglong>(id));
    move.insert(QStringLiteral("folder"), spoken(folder));
    QVariantMap op;
    op.insert(QStringLiteral("Move"), move);
    m_bridge->editPlaylists(op);
}

QVariantList FileBrowser::playlistMenu() const
{
    if (m_bridge == nullptr) {
        return {};
    }
    const QVariantMap tree = m_bridge->playlistTree();
    const QVariantList folders = tree.value(QStringLiteral("folders")).toList();
    const QVariantList playlists = tree.value(QStringLiteral("playlists")).toList();
    QVariantList rows;
    auto playlist = [](const QVariantMap &found) {
        QVariantMap row = found;
        // The tree hands a playlist over without a kind: the rail adds one while it draws, and so does the
        // menu — a row with no kind reads as a heading, and the menu offered folders and nothing to add to.
        row.insert(QStringLiteral("kind"), QStringLiteral("playlist"));
        return row;
    };
    // One flat list, every playlist once, in the order the rail shows: the root first, then each folder's.
    // The menu is the place to add tracks, not to browse the folder structure, so the folders' names stay
    // out of it (owner's decision, 24.11). The tree repeats a playlist under `All` and under its folder;
    // the menu never does.
    for (const QVariant &item : playlists) {
        const QVariantMap found = item.toMap();
        if (found.value(QStringLiteral("folder")).toString().isEmpty()) {
            rows.append(playlist(found));
        }
    }
    for (const QVariant &folder : folders) {
        const QString path = folder.toString();
        for (const QVariant &item : playlists) {
            const QVariantMap found = item.toMap();
            if (found.value(QStringLiteral("folder")).toString() == path) {
                rows.append(playlist(found));
            }
        }
    }
    return rows;
}

QVariantList FileBrowser::trackRows(const QStringList &paths) const
{
    QVariantList rows;
    for (const QString &path : paths) {
        for (const QVariant &entry : m_all) {
            const QVariantMap row = entry.toMap();
            if (row.value(QStringLiteral("path")).toString() != path) {
                continue;
            }
            const QString fingerprint = row.value(QStringLiteral("fingerprint")).toString();
            if (fingerprint.isEmpty()) {
                continue;
            }
            rows.append(QVariantMap{{QStringLiteral("fingerprint"), fingerprint},
                                    {QStringLiteral("path"), path}});
            break;
        }
    }
    return rows;
}

void FileBrowser::playlistAddTracks(qint64 playlist, const QVariantList &tracks)
{
    if (m_bridge == nullptr || playlist == 0) {
        return;
    }
    QVariantMap add;
    add.insert(QStringLiteral("playlist"), static_cast<qlonglong>(playlist));
    add.insert(QStringLiteral("tracks"), tracks);
    QVariantMap op;
    op.insert(QStringLiteral("AddTracks"), add);
    m_bridge->editPlaylists(op);
}

void FileBrowser::playlistRemoveRows(const QVariantList &positions)
{
    if (m_bridge == nullptr || m_playlistId == 0 || positions.isEmpty()) {
        return;
    }
    QVariantMap drop;
    drop.insert(QStringLiteral("playlist"), static_cast<qlonglong>(m_playlistId));
    drop.insert(QStringLiteral("positions"), positions);
    QVariantMap op;
    op.insert(QStringLiteral("RemoveRows"), drop);
    m_bridge->editPlaylists(op);
}

void FileBrowser::playlistReorder(int from, int to)
{
    if (m_bridge == nullptr || m_playlistId == 0 || from < 0 || to < 0 || from == to) {
        return;
    }
    QVariantMap reorder;
    reorder.insert(QStringLiteral("playlist"), static_cast<qlonglong>(m_playlistId));
    reorder.insert(QStringLiteral("from"), from);
    reorder.insert(QStringLiteral("to"), to);
    QVariantMap op;
    op.insert(QStringLiteral("Reorder"), reorder);
    m_bridge->editPlaylists(op);
}

void FileBrowser::playlistRemoveDuplicates()
{
    if (m_bridge == nullptr || m_playlistId == 0) {
        return;
    }
    QVariantMap dups;
    dups.insert(QStringLiteral("playlist"), static_cast<qlonglong>(m_playlistId));
    QVariantMap op;
    op.insert(QStringLiteral("RemoveDuplicates"), dups);
    m_bridge->editPlaylists(op);
}

void FileBrowser::playlistFolderCreate(const QString &path)
{
    if (m_bridge == nullptr || path.trimmed().isEmpty()) {
        return;
    }
    QVariantMap folder;
    folder.insert(QStringLiteral("path"), spoken(path.trimmed()));
    QVariantMap op;
    op.insert(QStringLiteral("FolderCreate"), folder);
    m_bridge->editPlaylists(op);
}

void FileBrowser::playlistFolderRename(const QString &from, const QString &to)
{
    if (m_bridge == nullptr || from.trimmed().isEmpty() || to.trimmed().isEmpty()) {
        return;
    }
    QVariantMap folder;
    folder.insert(QStringLiteral("from"), spoken(from.trimmed()));
    folder.insert(QStringLiteral("to"), spoken(to.trimmed()));
    QVariantMap op;
    op.insert(QStringLiteral("FolderRename"), folder);
    m_bridge->editPlaylists(op);
}

void FileBrowser::playlistFolderDelete(const QString &path)
{
    if (m_bridge == nullptr || path.trimmed().isEmpty()) {
        return;
    }
    QVariantMap folder;
    folder.insert(QStringLiteral("path"), spoken(path.trimmed()));
    QVariantMap op;
    op.insert(QStringLiteral("FolderDelete"), folder);
    m_bridge->editPlaylists(op);
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
                QDir(info.dir().absolutePath()).filePath(QStringLiteral(".djmeta/%1").arg(info.fileName()))
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
    // A playlist is narrowed in the index too: the search never grows the list it came from, and a
    // playlist the DJ filtered is not the whole playlist.
    if (m_mode == QLatin1String("playlists") && m_playlistId != 0) {
        askPlaylistPage();
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
    // The question as it leaves the shell: an empty list is either a question with no rows in it or a question
    // that never arrived, and only the log can tell those apart.
    uiLog(QStringLiteral("question %1: offset %2, limit %3, roots [%4], search \"%5\"")
              .arg(query.value(QStringLiteral("id")).toInt())
              .arg(query.value(QStringLiteral("offset")).toInt())
              .arg(query.value(QStringLiteral("limit")).toInt())
              .arg(query.value(QStringLiteral("roots")).toStringList().join(QStringLiteral(" | ")),
                   query.value(QStringLiteral("search")).toString()));
    return sendCommand(QString(), command);
}

int BridgeClient::queryPlaylist(const QVariantMap &query)
{
    QJsonObject body;
    body.insert(QStringLiteral("query"), QJsonObject::fromVariantMap(query));
    QJsonObject command;
    command.insert(QStringLiteral("QueryPlaylist"), body);
    return sendCommand(QString(), command);
}

int BridgeClient::editPlaylists(const QVariantMap &op)
{
    QJsonObject body;
    body.insert(QStringLiteral("op"), QJsonObject::fromVariantMap(op));
    QJsonObject command;
    command.insert(QStringLiteral("Playlists"), body);
    return sendCommand(QString(), command);
}

QVariantMap BridgeClient::playlistTree() const
{
    const QVariant value = m_snapshot.value(QStringLiteral("playlists"));
    if (!value.isValid() || value.isNull()) {
        return QVariantMap();
    }
    return value.toMap();
}

QVariantMap BridgeClient::playlistPage() const
{
    const QVariant value = m_snapshot.value(QStringLiteral("playlist_page"));
    if (!value.isValid() || value.isNull()) {
        return QVariantMap();
    }
    return value.toMap();
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
    if (!m_genreList.isEmpty()) {
        // The index answered with name and count: no counting here.
        return m_genreList;
    }
    // Before the index answers — or where there is no index — the boxes come from the count taken while
    // nothing was filtered. Counting the rows in hand now would count the rows the filter kept.
    return m_genreFallback;
}

QVariantList FileBrowser::censusFromRows() const
{
    // First spelling wins, counting ignores case: `House` and `house` are one box with two tracks.
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

bool FileBrowser::isLocked(const QString &path) const
{
    return m_lockedPaths.contains(path);
}

void FileBrowser::setLocked(const QString &path, bool on)
{

    // A lock that goes nowhere looks the same as a lock that does nothing: the path and the direction are the whole
    // story on a machine we cannot watch.
    uiLog(QStringLiteral("lock %1 asked for %2")
              .arg(on ? QStringLiteral("on") : QStringLiteral("off"),
                   path.isEmpty() ? QStringLiteral("an empty path") : path));    if (path.isEmpty()) {
        return;
    }
    // The row keeps the value too: the next page answer replaces the flags, and a press that only lives in
    // this list would be undone by a refresh the DJ did not ask for.
    const auto patch = [&](QVariantList &rows) {
        for (QVariant &entry : rows) {
            QVariantMap row = entry.toMap();
            if (row.value(QStringLiteral("path")).toString() == path) {
                row.insert(QStringLiteral("analysis_lock"), on);
                entry = row;
            }
        }
    };
    const bool had = m_lockedPaths.contains(path);
    if (on == had) {
        return;
    }
    if (on) {
        m_lockedPaths.append(path);
        m_pressedLocked.append(path);
    } else {
        m_lockedPaths.removeAll(path);
        m_pressedLocked.removeAll(path);
    }
    // The rows in hand too, and without `changed`: the shell draws the flag from `lockedPaths`, so the list
    // keeps its identity and the DJ's window stays where it is. Only the page keeps the value for the next
    // rebuild.
    patch(m_all);
    patch(m_entries);
    patch(m_playlistRows);
    emit lockChanged();
}

void FileBrowser::refreshLocked()
{
    QStringList found;
    const auto collect = [&](const QVariantList &rows) {
        for (const QVariant &entry : rows) {
            const QVariantMap row = entry.toMap();
            if (row.value(QStringLiteral("analysis_lock")).toBool()) {
                const QString path = row.value(QStringLiteral("path")).toString();
                if (!found.contains(path)) {
                    found.append(path);
                }
            }
        }
    };
    collect(m_all);
    collect(m_playlistRows);
    // The press outranks an answer that predates it: the row keeps the flag the DJ set, and the index is
    // believed as soon as it agrees.
    for (const QString &pressed : m_pressedLocked) {
        bool said = false;
        const auto force = [&](QVariantList &rows) {
            for (QVariant &entry : rows) {
                QVariantMap row = entry.toMap();
                if (row.value(QStringLiteral("path")).toString() != pressed) {
                    continue;
                }
                if (row.value(QStringLiteral("analysis_lock")).toBool()) {
                    said = true;
                } else {
                    row.insert(QStringLiteral("analysis_lock"), true);
                    entry = row;
                }
            }
        };
        force(m_all);
        force(m_entries);
        force(m_playlistRows);
        if (said) {
            m_pressedLocked.removeAll(pressed);
        }
        if (!found.contains(pressed)) {
            found.append(pressed);
        }
    }
    if (found == m_lockedPaths) {
        return;
    }
    const bool grew = found.size() > m_lockedPaths.size();
    m_lockedPaths = found;
    uiLog(QStringLiteral("the list now carries %1 locked row(s)%2")
              .arg(found.size())
              .arg(grew && !found.isEmpty() ? QStringLiteral(", last %1").arg(found.last())
                                            : QString()));
    emit lockChanged();
}

void FileBrowser::applyFilter()
{
    // The index answered this question already: genres and the search went to SQL, and the rows that came
    // back are the answer. Filtering them again in memory is not a safety net — it drops rows whose genre
    // text is empty while the link table knows the genre, and the list then looks empty for a filter that
    // did match.
    if (m_paged) {
        m_entries = m_all;
        emit changed();
        return;
    }
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

void FileBrowser::clearCollectionSort()
{
    if (m_sort.isEmpty()) {
        return;
    }
    // Nothing decides the order any more. The keys go, the header loses its mark, and the list is built
    // again in the order its own source gives: the index when there is one, the walk when there is not.
    // Emptying the keys alone would leave the rows in the order of a sort that no longer exists — a list
    // sorted by a column with no arrow over it.
    m_sort.clear();
    emit sortChanged();
    if (m_paged) {
        askPage(0);
        return;
    }
    collect(m_roots);
    // The view reads the filtered list, and collect() fills the unfiltered one: without this line the
    // rows keep the order the sort left them in, and the reset is invisible.
    applyFilter();
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
    m_retryTimer.setSingleShot(true);
    connect(&m_retryTimer, &QTimer::timeout, this, &BridgeClient::attemptConnect);
#ifdef Q_OS_WIN
    connect(&m_socket, &QTcpSocket::connected, this, &BridgeClient::onConnected);
    connect(&m_socket, &QTcpSocket::disconnected, this, &BridgeClient::onDisconnected);
    connect(&m_socket, &QTcpSocket::readyRead, this, &BridgeClient::onReadyRead);
    connect(&m_socket, &QTcpSocket::errorOccurred, this,
            [this](QAbstractSocket::SocketError) { onSocketError(); });
#else
    connect(&m_socket, &QLocalSocket::connected, this, &BridgeClient::onConnected);
    connect(&m_socket, &QLocalSocket::disconnected, this, &BridgeClient::onDisconnected);
    connect(&m_socket, &QLocalSocket::readyRead, this, &BridgeClient::onReadyRead);
#if QT_VERSION >= QT_VERSION_CHECK(6, 5, 0)
    connect(&m_socket, &QLocalSocket::errorOccurred, this, &BridgeClient::onSocketError);
#else
    connect(&m_socket, QOverload<QLocalSocket::LocalSocketError>::of(&QLocalSocket::error),
            this, &BridgeClient::onSocketError);
#endif
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

void BridgeClient::setEndpoint(const QString &endpoint)
{
    if (m_endpoint == endpoint) {
        return;
    }
    m_endpoint = endpoint;
    emit endpointChanged();
}

void BridgeClient::connectToEngine(const QString &path)
{
    setEndpoint(path);
    // The environment, once per process: without it two installations of one version are indistinguishable, and a
    // report about «no engine» cannot be checked against the name the engine actually took.
    if (!m_reportedOnce) {
        m_reportedOnce = true;
        uiLog(QStringLiteral("shell %1 from %2 (file time %3)")
                  .arg(QCoreApplication::applicationVersion(),
                       QCoreApplication::applicationFilePath(),
                       QFileInfo(QCoreApplication::applicationFilePath())
                           .lastModified()
                           .toString(Qt::ISODate)));
        uiLog(QStringLiteral("qt %1, platform %2, pid %3")
                  .arg(QString::fromLatin1(qVersion()), QGuiApplication::platformName())
                  .arg(QCoreApplication::applicationPid()));
        uiLog(QStringLiteral("cwd %1").arg(QDir::currentPath()));
        uiLog(QStringLiteral("collection by default %1").arg(defaultCollectionRoot()));
        uiLog(QStringLiteral("log level %1 (HAULER_LOG=%2)")
                  .arg(uiLevel())
                  .arg(qEnvironmentVariable("HAULER_LOG", QStringLiteral("(unset)"))));
        for (const char *name : {"APPDATA", "LOCALAPPDATA", "USERPROFILE", "USERNAME", "XDG_DATA_HOME",
                                 "XDG_CONFIG_HOME", "XDG_RUNTIME_DIR", "ORT_DYLIB_PATH"}) {
            const QString value = qEnvironmentVariable(name);
            if (!value.isEmpty()) {
                uiLog(QStringLiteral("env %1=%2").arg(QString::fromLatin1(name), value));
            }
        }
    }
    // The build, not just the version: two installs of one version look the same in a log, and «is this the file
    // I just installed?» is the first question when a fix seems not to have arrived.
    const QString exe = QCoreApplication::applicationFilePath();
    uiLog(QStringLiteral("shell %1 from %2 (file time %3): asking for the engine on %4 (log %5)")
              .arg(QCoreApplication::applicationVersion(), exe,
                   QFileInfo(exe).lastModified().toString(Qt::ISODate), path, uiLogPath()));
    m_wantConnection = true;
    m_retryAttempts = 0;
    m_retryTimer.stop();
    attemptConnect();
}

void BridgeClient::attemptConnect()
{
    if (m_shuttingDown || !m_wantConnection) {
        return;
    }
#ifdef Q_OS_WIN
    if (m_socket.state() != QAbstractSocket::UnconnectedState) {
        m_socket.abort();
    }
#else
    if (m_socket.state() != QLocalSocket::UnconnectedState) {
        m_socket.abort();
    }
#endif
    m_buffer.clear();
    m_handshakeSent = false;
    setConnected(false);
    setStatus(QStringLiteral("connecting"));
#ifdef Q_OS_WIN
    // The endpoint is a file: the engine binds a port of the loopback interface and writes the address there. An
    // engine that is not up yet has no file, and that is a retry, not a fault.
    QFile endpoint(m_endpoint);
    if (!endpoint.exists() || !endpoint.open(QIODevice::ReadOnly | QIODevice::Text)) {
        uiLog(QStringLiteral("attempt %1: no engine address file yet at %2").arg(m_retryAttempts + 1).arg(m_endpoint));
        scheduleReconnect();
        return;
    }
    const QString address = QString::fromUtf8(endpoint.readAll()).trimmed();
    endpoint.close();
    const int colon = address.lastIndexOf(QLatin1Char(':'));
    bool portOk = false;
    const int port = colon > 0 ? address.mid(colon + 1).toInt(&portOk) : 0;
    if (!portOk || port <= 0) {
        uiLog(QStringLiteral("attempt %1: the address file says \"%2\", which is not an address")
                  .arg(m_retryAttempts + 1)
                  .arg(address));
        scheduleReconnect();
        return;
    }
    uiLog(QStringLiteral("attempt %1: connecting to %2 on port %3")
              .arg(m_retryAttempts + 1)
              .arg(address.left(colon))
              .arg(port));
    m_socket.connectToHost(QHostAddress::LocalHost, static_cast<quint16>(port));
#else
    uiLog(QStringLiteral("attempt %1: connecting to %2").arg(m_retryAttempts + 1).arg(m_endpoint));
    m_socket.connectToServer(m_endpoint);
#endif
}

void BridgeClient::scheduleReconnect()
{
    if (m_shuttingDown || !m_wantConnection || m_retryTimer.isActive()) {
        return;
    }
    ++m_retryAttempts;
    // The first seconds are the common case: the launcher starts the engine and the shell together, and the
    // engine needs a moment to open its pipe. After that the tries thin out — an engine that is still absent
    // half a minute later is a real fault, and the terminal should say so once, not fifty times.
    const int delay = m_retryAttempts <= 40 ? 250 : 2000;
    uiLog(QStringLiteral("no engine on %1 yet (attempt %2), trying again in %3 ms")
              .arg(m_endpoint)
              .arg(m_retryAttempts)
              .arg(delay));
    m_retryTimer.start(delay);
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
    uiLog(QStringLiteral("socket connected to %1; hello sent").arg(m_endpoint));
}

void BridgeClient::onDisconnected()
{
    if (m_shuttingDown) {
        return;
    }
    setConnected(false);
    setStatus(QStringLiteral("disconnected"));
    // An engine that went away is worth meeting again: it may be restarting.
    scheduleReconnect();
}

#ifdef Q_OS_WIN
void BridgeClient::onSocketError()
#else
void BridgeClient::onSocketError(QLocalSocket::LocalSocketError error)
#endif
{
#ifndef Q_OS_WIN
    Q_UNUSED(error);
#endif
    setConnected(false);
    setStatus(QStringLiteral("error: %1").arg(m_socket.errorString()));
    uiLog(QStringLiteral("socket error on %1: %2").arg(m_endpoint, m_socket.errorString()));
    // The greeting is reported as a failure only when the connection had been established: a refused first
    // attempt is the ordinary race with a starting engine, not news.
    if (m_handshakeSent) {
        emit failed(m_socket.errorString());
    }
    scheduleReconnect();
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
        uiLog(QStringLiteral("engine answered: protocol %1, state %2, epoch %3")
                  .arg(protocol, stateProtocol)
                  .arg(m_epoch));
        if (stateProtocol != QStringLiteral("state-v1")) {
            setStatus(QStringLiteral("error: engine speaks %1").arg(stateProtocol));
            emit failed(QStringLiteral("incompatible state protocol %1").arg(stateProtocol));
            m_socket.abort();
            return;
        }
        setConnected(true);
        setStatus(QStringLiteral("%1 · %2").arg(protocol, stateProtocol));
        // The row the window placed before the link was up: the engine hears it now, once.
        if (!m_browserCursorWanted.isEmpty() && m_browserCursorWanted != m_browserCursor) {
            const QString wanted = m_browserCursorWanted;
            m_browserCursorWanted.clear();
            browserCursorTo(wanted);
        }
        return;
    }

    if (message.contains(QStringLiteral("Snapshot"))) {
        m_snapshot = message.value(QStringLiteral("Snapshot")).toObject().toVariantMap();
        // The browser's place, said only when it moves: a signal on every snapshot would redraw the list forty times a
        // second and fight the DJ's own scrolling.
        const QVariantMap browser = m_snapshot.value(QStringLiteral("browser")).toMap();
        const bool open = browser.value(QStringLiteral("open"), true).toBool();
        const QString cursor = browser.value(QStringLiteral("cursor")).toString();
        const int stepSeq = browser.value(QStringLiteral("step_seq")).toInt();
        const int step = browser.value(QStringLiteral("step")).toInt();
        const int placeSeq = browser.value(QStringLiteral("place_seq")).toInt();
        const int placeStep = browser.value(QStringLiteral("place_step")).toInt();
        const int placeToggleSeq = browser.value(QStringLiteral("place_toggle_seq")).toInt();
        if (open != m_browserOpen || cursor != m_browserCursor || stepSeq != m_browserStepSeq
            || step != m_browserStep || placeSeq != m_browserPlaceSeq || placeStep != m_browserPlaceStep
            || placeToggleSeq != m_browserPlaceToggleSeq) {
            m_browserOpen = open;
            m_browserCursor = cursor;
            m_browserStepSeq = stepSeq;
            m_browserStep = step;
            m_browserPlaceSeq = placeSeq;
            m_browserPlaceStep = placeStep;
            m_browserPlaceToggleSeq = placeToggleSeq;
            emit browserChanged();
        }
        // The snapshot says which file each deck holds, and the engine is the one that knows. A client
        // that was started against a running engine (or restarted while the decks keep their tracks)
        // has no path of its own; without this the lock and the grid edits went out with an empty path
        // and the engine refused them with "No such file or directory" (owner's report, 2026-11-19).
        const QVariantList decks = m_snapshot.value(QStringLiteral("decks")).toList();
        for (const QVariant &entry : decks) {
            const QVariantMap deck = entry.toMap();
            const QString id = deck.value(QStringLiteral("deck")).toString();
            const QString path = deck.value(QStringLiteral("path")).toString();
            if (!id.isEmpty() && !path.isEmpty()) {
                m_deckPaths.insert(id, path);
            }
            // The deck's own story, told when it changes: a load that never leaves `Loading` and a deck that says
            // `Ready` over silence look the same on the screen and are told apart here.
            const QString load = deck.value(QStringLiteral("load")).toString();
            const QString peaks = QStringLiteral("%1/%2")
                                      .arg(deck.value(QStringLiteral("peak_left")).toDouble(), 0, 'f', 3)
                                      .arg(deck.value(QStringLiteral("peak_right")).toDouble(), 0, 'f', 3);
            const QString signature = id + QLatin1Char('|') + load + QLatin1Char('|')
                + path + QLatin1Char('|') + peaks;
            if (!id.isEmpty() && m_deckSignatures.value(id) != signature) {
                m_deckSignatures.insert(id, signature);
                uiLogAt(load == QLatin1String("Ready") ? 2 : 3,
                        QStringLiteral("deck %1: %2, path %3, peaks %4, revision %5")
                            .arg(id, load, path.isEmpty() ? QStringLiteral("(none)") : path, peaks)
                            .arg(m_snapshot.value(QStringLiteral("revision")).toInt()));
            }
        }
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
        if (rejected) {
            uiLog(QStringLiteral("command %1 (%2) refused: %3")
                      .arg(id)
                      .arg(m_commandKinds.value(id, QStringLiteral("?")))
                      .arg(reason));
        }
        m_commandKinds.remove(id);
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
    m_commandKinds.insert(id, command.keys().value(0));
    sendJson(message);
    ++m_pendingCommands;
    setStatus(QStringLiteral("waiting for the engine"));
    return id;
}

int BridgeClient::sendUnit(const QString &name)
{
    const int id = m_nextCommandId++;
    QJsonObject scheduled;
    scheduled.insert(QStringLiteral("id"), id);
    scheduled.insert(QStringLiteral("effective_sample"), 0);
    scheduled.insert(QStringLiteral("command"), name);
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

QString FileBrowser::fileName(const QString &path) const
{
    return pathrules::name(path);
}

void BridgeClient::note(const QString &message)
{
    uiLogAt(2, QStringLiteral("panel: ") + message);
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

int BridgeClient::rescan(const QStringList &roots)
{
    QJsonObject options;
    QJsonArray folders;
    for (const QString &folder : roots) {
        folders.append(folder);
    }
    options.insert(QStringLiteral("roots"), folders);
    QJsonObject command;
    command.insert(QStringLiteral("Rescan"), options);
    const int id = sendCommand(QString(), command);
    setStatus(QStringLiteral("reading the collection again"));
    return id;
}

int BridgeClient::downloadModel(const QString &variant)
{
    QJsonObject options;
    options.insert(QStringLiteral("variant"), variant);
    QJsonObject op;
    op.insert(QStringLiteral("Download"), options);
    QJsonObject command;
    command.insert(QStringLiteral("Model"), QJsonObject{{QStringLiteral("op"), op}});
    const int id = sendCommand(QString(), command);
    setStatus(tr("downloading the %1 model").arg(variant));
    return id;
}

int BridgeClient::removeModel(const QString &variant)
{
    QJsonObject options;
    options.insert(QStringLiteral("variant"), variant);
    QJsonObject op;
    op.insert(QStringLiteral("Delete"), options);
    QJsonObject command;
    command.insert(QStringLiteral("Model"), QJsonObject{{QStringLiteral("op"), op}});
    const int id = sendCommand(QString(), command);
    setStatus(tr("removing the %1 model").arg(variant));
    return id;
}

void FileBrowser::rescanCollection()
{
    if (m_bridge == nullptr) {
        return;
    }
    m_bridge->rescan(m_roots);
}

int BridgeClient::cancelImport()
{
    const int id = sendUnit(QStringLiteral("ImportCancel"));
    // The DJ pressed stop and must see that he was heard, not a window that keeps claiming work: the
    // batch stops at the next honest point, and a file's analysis now sees the flag too.
    setStatus(QStringLiteral("stopping the batch"));
    return id;
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
    const int id = sendUnit(QStringLiteral("AnalyzeCancel"));
    // The stop is heard at once and honest about what it can promise: the files already being decoded are
    // finished and written, and nothing new starts. A window that keeps counting without a word looks like a
    // button that does nothing, which is how "Cancel does not cancel" is reported.
    setStatus(QStringLiteral("stopping the analysis"));
    return id;
}

bool BridgeClient::browserOpen() const
{
    return m_browserOpen;
}

QString BridgeClient::browserCursor() const
{
    return m_browserCursor;
}

int BridgeClient::browserStepSeq() const
{
    return m_browserStepSeq;
}

int BridgeClient::browserStep() const
{
    return m_browserStep;
}

int BridgeClient::browserPlaceSeq() const
{
    return m_browserPlaceSeq;
}

int BridgeClient::browserPlaceStep() const
{
    return m_browserPlaceStep;
}

int BridgeClient::browserPlaceToggleSeq() const
{
    return m_browserPlaceToggleSeq;
}

void BridgeClient::browserShow(bool open)
{
    QJsonObject options;
    options.insert(QStringLiteral("open"), open);
    QJsonObject command;
    command.insert(QStringLiteral("BrowserShow"), options);
    sendQuiet(command);
}

void BridgeClient::sendQuiet(const QJsonObject &command)
{
    // No status line and no pending counter: the browser's place changes as often as the DJ moves the cursor, and a
    // status write per move redraws the whole window — which is what kept a list from settling where it was put
    // (measured: `--scroll-check` read the content still gliding, 491 -> 473).
    QJsonObject scheduled;
    scheduled.insert(QStringLiteral("id"), m_nextCommandId++);
    scheduled.insert(QStringLiteral("effective_sample"), 0);
    scheduled.insert(QStringLiteral("command"), command);
    QJsonObject message;
    message.insert(QStringLiteral("Command"), scheduled);
    sendJson(message);
}

void BridgeClient::browserCursorTo(const QString &path)
{
    // The row the window is on, told once: the engine echoes it back in every snapshot, and sending it again on every
    // echo would be a command per snapshot.
    if (path == m_browserCursor) {
        return;
    }
    // Kept until the engine answers: the window places its cursor before the socket is up, and a command written into a
    // closed socket is a command that never happened. The panel's press then had no row to load (owner's report, 25.11).
    m_browserCursorWanted = path;
    if (!m_connected) {
        return;
    }
    QJsonObject options;
    options.insert(QStringLiteral("path"), path);
    QJsonObject command;
    command.insert(QStringLiteral("BrowserCursor"), options);
    sendQuiet(command);
}

int BridgeClient::load(const QString &deck, const QString &path)
{
    uiLog(QStringLiteral("load into %1: %2 (connected: %3)")
              .arg(deck, path)
              .arg(m_connected ? QStringLiteral("yes") : QStringLiteral("no")));
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

int BridgeClient::setAnalysisLock(const QString &deck, bool enabled)
{
    // The engine writes the flag beside the file and into the collection; it knows which file each deck
    // holds, so a client that reconnected sends the deck and the path it remembers.
    QJsonObject payload;
    payload.insert(QStringLiteral("deck"), deck);
    payload.insert(QStringLiteral("path"), m_deckPaths.value(deck));
    payload.insert(QStringLiteral("enabled"), enabled);
    QJsonObject command;
    command.insert(QStringLiteral("SetAnalysisLock"), payload);
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

int BridgeClient::shiftGrid(const QString &deck, qlonglong frame, qlonglong frames)
{
    // Кадр приходит от руки, а не из снапшота: снапшот на кадр экрана отстаёт, и правка тогда попадает
    // в соседнюю секцию.
    QJsonObject inner;
    inner.insert(QStringLiteral("frame"), QJsonValue::fromVariant(frame));
    inner.insert(QStringLiteral("frames"), QJsonValue::fromVariant(frames));
    QJsonObject edit;
    edit.insert(QStringLiteral("ShiftStretch"), inner);
    return sendGridEdit(deck, edit);
}

int BridgeClient::snapStretch(const QString &deck, qlonglong frame)
{
    QJsonObject inner;
    inner.insert(QStringLiteral("frame"), QJsonValue::fromVariant(frame));
    QJsonObject edit;
    edit.insert(QStringLiteral("SnapStretch"), inner);
    return sendGridEdit(deck, edit);
}

int BridgeClient::levelStretch(const QString &deck, qlonglong frame, double factor)
{
    if (!(factor > 0.0)) {
        return -1;
    }
    QJsonObject inner;
    inner.insert(QStringLiteral("frame"), QJsonValue::fromVariant(frame));
    inner.insert(QStringLiteral("factor"), factor);
    QJsonObject edit;
    edit.insert(QStringLiteral("LevelStretch"), inner);
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

int BridgeClient::putHandOn(const QString &deck, bool held)
{
    handLastMs_.remove(deck);
    handSpeed_.remove(deck);
    handHeld_[deck] = held;
    QJsonObject options;
    options.insert(QStringLiteral("deck"), deck);
    options.insert(QStringLiteral("speed"), 0.0);
    options.insert(QStringLiteral("held"), held);
    QJsonObject command;
    command.insert(QStringLiteral("Hand"), options);
    return sendCommand(deck, command);
}

int BridgeClient::keepHand(const QString &deck)
{
    if (!handHeld_.value(deck, false)) {
        return -1;
    }
    QJsonObject options;
    options.insert(QStringLiteral("deck"), deck);
    options.insert(QStringLiteral("speed"), handSpeed_.value(deck, 0.0));
    options.insert(QStringLiteral("held"), true);
    QJsonObject command;
    command.insert(QStringLiteral("Hand"), options);
    return sendCommand(deck, command);
}

int BridgeClient::setHandSpeed(const QString &deck, double speed)
{
    // A mouse emits hundreds of moves a second and the engine has one block every five milliseconds: an
    // update that arrives inside the same block would be applied and overwritten unseen. Coalescing here
    // keeps the wire quiet and costs nothing the ear can hear.
    if (!handHeld_.value(deck, false)) {
        return -1;
    }
    const qint64 now = QDateTime::currentMSecsSinceEpoch();
    const qint64 last = handLastMs_.value(deck, 0);
    if (last != 0 && now - last < 5) {
        return 0;
    }
    const double previous = handSpeed_.value(deck, 0.0);
    if (qAbs(speed - previous) < qAbs(speed) * 0.02) {
        return 0;
    }
    handLastMs_[deck] = now;
    handSpeed_[deck] = speed;
    QJsonObject options;
    options.insert(QStringLiteral("deck"), deck);
    options.insert(QStringLiteral("speed"), speed);
    options.insert(QStringLiteral("held"), true);
    QJsonObject command;
    command.insert(QStringLiteral("Hand"), options);
    return sendCommand(deck, command);
}

int BridgeClient::setGridMetre(const QString &deck, int beats)
{
    if (beats < 2 || beats > 8) {
        return -1;
    }
    QJsonObject inner;
    inner.insert(QStringLiteral("beats"), beats);
    QJsonObject edit;
    edit.insert(QStringLiteral("Metre"), inner);
    return sendGridEdit(deck, edit);
}

int BridgeClient::splitGrid(const QString &deck, qlonglong frame)
{
    QJsonObject inner;
    inner.insert(QStringLiteral("frame"), QJsonValue::fromVariant(frame));
    QJsonObject edit;
    edit.insert(QStringLiteral("Split"), inner);
    return sendGridEdit(deck, edit);
}

int BridgeClient::dropGridStretch(const QString &deck, qlonglong frame)
{
    QJsonObject inner;
    inner.insert(QStringLiteral("frame"), QJsonValue::fromVariant(frame));
    QJsonObject edit;
    edit.insert(QStringLiteral("DropStretch"), inner);
    return sendGridEdit(deck, edit);
}

int BridgeClient::setGridStretchTempo(const QString &deck, qlonglong frame, double bpm)
{
    if (!(bpm > 0.0)) {
        return -1;
    }
    QJsonObject inner;
    inner.insert(QStringLiteral("frame"), QJsonValue::fromVariant(frame));
    inner.insert(QStringLiteral("bpm"), bpm);
    QJsonObject edit;
    edit.insert(QStringLiteral("StretchTempo"), inner);
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

int BridgeClient::setPanelPresent(bool present)
{
    QJsonObject options;
    options.insert(QStringLiteral("present"), present);
    QJsonObject command;
    command.insert(QStringLiteral("PanelPresent"), options);
    return sendCommand(QString(), command);
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
    // "cut:7" is the cut-in law with the fader opening at seven percent: the number travels with the law, so
    // the law is an object on the wire rather than a name. Everything else is its own name.
    const QString name = curve.trimmed().toLower();
    if (name.startsWith(QStringLiteral("cut:"))) {
        bool ok = false;
        const int percent = name.mid(4).toInt(&ok);
        if (ok && percent >= 0 && percent <= 100) {
            QJsonObject cut;
            cut.insert(QStringLiteral("cut_in"), percent);
            QJsonObject law;
            law.insert(QStringLiteral("cut"), cut);
            options.insert(QStringLiteral("curve"), law);
            QJsonObject command;
            command.insert(QStringLiteral("SetCrossfaderCurve"), options);
            return sendCommand(QStringLiteral("mixer"), command);
        }
    }
    options.insert(QStringLiteral("curve"), name);
    QJsonObject command;
    command.insert(QStringLiteral("SetCrossfaderCurve"), options);
    return sendCommand(QStringLiteral("mixer"), command);
}

int BridgeClient::setHamster(bool on)
{
    QJsonObject options;
    options.insert(QStringLiteral("on"), on);
    QJsonObject command;
    command.insert(QStringLiteral("SetHamster"), options);
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

int BridgeClient::setChannelCue(const QString &deck, bool on)
{
    QJsonObject options;
    options.insert(QStringLiteral("deck"), deck);
    options.insert(QStringLiteral("on"), on);
    QJsonObject command;
    command.insert(QStringLiteral("SetChannelCue"), options);
    return sendCommand(deck, command);
}

int BridgeClient::setCueGain(double gain)
{
    QJsonObject options;
    options.insert(QStringLiteral("linear_gain"), gain);
    QJsonObject command;
    command.insert(QStringLiteral("SetCueGain"), options);
    return sendCommand(QStringLiteral("phones"), command);
}

int BridgeClient::setCueMix(double mix)
{
    QJsonObject options;
    options.insert(QStringLiteral("mix"), mix);
    QJsonObject command;
    command.insert(QStringLiteral("SetCueMix"), options);
    return sendCommand(QStringLiteral("phones"), command);
}

int BridgeClient::setDeckFx(const QString &deck, const QString &kind, double amount, bool enabled)
{
    QJsonObject patch;
    patch.insert(QStringLiteral("kind"), kind);
    patch.insert(QStringLiteral("amount"), amount);
    patch.insert(QStringLiteral("enabled"), enabled);
    QJsonObject options;
    options.insert(QStringLiteral("deck"), deck);
    options.insert(QStringLiteral("patch"), patch);
    QJsonObject command;
    command.insert(QStringLiteral("SetDeckFx"), options);
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
    m_wantConnection = false;
    m_retryTimer.stop();
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
