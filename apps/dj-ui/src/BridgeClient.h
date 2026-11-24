// The UI's only way to the engine (ADR-0003: the UI is a separate process).
//
// The shell owns no domain state: it renders the last snapshot the engine published and forwards
// user intent as commands. A button is only allowed to look "pressed" because a snapshot said so,
// never because the click happened (DJP-ARCH-004).

#pragma once

#include <QByteArray>
#include <QJsonObject>
#include <QLocalSocket>
#ifdef Q_OS_WIN
#include <QHostAddress>
#include <QTcpSocket>
#endif
#include <QObject>
#include <QTimer>

#include <functional>
#include <QString>
#include <QVariantMap>

/// The collection the engine keeps when the DJ has named no folder: `<data>/hauler-dj/collection`.
///
/// The engine answers this question from `dj_paths`: `%APPDATA%` on Windows, `$XDG_DATA_HOME` or
/// `~/.local/share` on Linux. Qt's `GenericDataLocation` answers `%LOCALAPPDATA%` on Windows — a different
/// folder — and a shell that points the import at one place while the list reads another shows an empty
/// collection over a full folder.
QString defaultCollectionRoot();

/// One line from the shell: `level` is 0 error, 1 warn, 2 info, 3 debug, 4 trace, and `HAULER_LOG` decides how
/// much is kept. The file is rotated at 8 MB, one previous copy is kept.
void uiLogAt(int level, const QString &message);
// Where the shell writes its journal: the fault writer puts its note beside it.
QString uiLogPath();

// Lists a directory for the load dialog, or the whole collection for the Collection section.
//
// Written by hand because both alternatives fail here: Qt's FileDialog is the GTK/portal dialog
// (a missing gsettings schema aborts the process) and Qt.labs.folderlistmodel is not built into
// this Qt. QDir cannot crash the UI and needs nothing installed.
class BridgeClient;

class FileBrowser : public QObject
{
    Q_OBJECT
    Q_PROPERTY(QString folder READ folder NOTIFY changed)
    Q_PROPERTY(QVariantList entries READ entries NOTIFY changed)
    /// How many times the rows were replaced rather than appended.
    ///
    /// A view handed a new model starts at the top, and the DJ's place has to be put back; the count is how
    /// the shell tells a replacement from an append, on the same `changed` signal it already listens to.
    Q_PROPERTY(int pageGeneration READ pageGeneration NOTIFY changed)
    Q_PROPERTY(QString error READ error NOTIFY changed)
    // "files" while the list is one folder, "collection" while it is the collection under its root.
    Q_PROPERTY(QString mode READ mode NOTIFY changed)
    // What the DJ typed into the collection's search field; empty shows every track.
    Q_PROPERTY(QString filter READ filter NOTIFY changed)
    // How many tracks the collection holds before the filter: the header says both numbers, so an
    // empty search result cannot pass for an empty collection.
    Q_PROPERTY(int total READ total NOTIFY changed)
    // Where the cursor stands in the list that is showing: the panel puts the highlight there when
    // the list comes back, and a row that is gone simply has no highlight.
    Q_PROPERTY(QString cursor READ cursor NOTIFY cursorChanged)
    // The collection's sort: one or more keys, the first deciding. The header writes it, the rows obey.
    Q_PROPERTY(QVariantList collectionSort READ collectionSort NOTIFY sortChanged)

public:
    explicit FileBrowser(QObject *parent = nullptr);

    QString folder() const { return m_folder; }
    QVariantList entries() const { return m_entries; }
    int pageGeneration() const { return m_pageGeneration; }
    QString error() const { return m_error; }
    QString mode() const { return m_mode; }
    QString filter() const { return m_filter; }
    /// The remembered row of the list that is showing.
    QString cursor() const;
    /// The remembered rows, per place: the file list, the collection and the playlists are three
    /// different places, and a cursor from one of them is not a row of another.
    QString filesCursor() const { return m_filesCursor; }
    QString collectionCursor() const { return m_collectionCursor; }
    QString playlistsCursor() const { return m_playlistsCursor; }
    /// Remember where the cursor stands in the list that is showing.
    Q_INVOKABLE void setCursor(const QString &path);
    /// Seed both places at startup, before either list has been built.
    Q_INVOKABLE void seedCursors(const QString &files, const QString &collection);
    /// Where `path` sits in the list that is showing; -1 when it is not there any more.
    Q_INVOKABLE int rowOf(const QString &path) const;
    /// The genres the collection carries, each with how many tracks have it: `{name, count}`.
    ///
    /// Read from the same records the list is built from, so a genre appears when a track that carries
    /// it does — no second source and nothing to keep in step.
    /// True when the rows come from the index and arrive in batches: the header says so, because a list
    /// that is a window over the database is a different thing from a list walked off the disks.
    Q_PROPERTY(bool paged READ paged NOTIFY pagedChanged)
    bool paged() const { return m_paged; }
    Q_PROPERTY(QVariantList genres READ genreList NOTIFY genresChanged)
    Q_PROPERTY(QStringList genreFilter READ genreFilter WRITE setGenreFilter NOTIFY genresChanged)
    Q_INVOKABLE QVariantList genreList() const;
    /// Narrow the collection to these genres: a track is kept when it carries any of them. An empty
    /// list is the whole collection, which is what "reset" means.
    Q_INVOKABLE void setGenreFilter(const QStringList &genres);
    QStringList genreFilter() const { return m_genres; }
    /// The collection's sort keys, in order: `{column, ascending}`.
    QVariantList collectionSort() const { return m_sort; }
    /// Sort the collection by `column`. A plain click makes it the only key and turns it around on a
    /// second click; `shift` adds it as the next key, so two columns can decide together.
    Q_INVOKABLE void sortCollection(const QString &column, bool shift);
    /// Drop every sort key: the collection goes back to the order its source gives it — the index when
    /// there is one, the walk of the disks when there is not. It is the way out of a sort set by
    /// accident, and the only way to a list that no column decides.
    Q_INVOKABLE void clearCollectionSort();
    int total() const { return m_total; }

    // Show `path`, or the default music folder when it is empty.
    Q_INVOKABLE void open(const QString &path);
    Q_INVOKABLE void up();
    Q_INVOKABLE QString defaultFolder() const;
    /// Show the collection under `roots`: every audio file below each of them, named as the import
    /// named it, with the tempo and the key its metadata records. Empty means the default root.
    Q_INVOKABLE void openCollection(const QStringList &roots);
    /// The list reached its end: append the next batch of the index, a page at a time.
    Q_INVOKABLE void loadMore();
    /// Read the collection again from the files: what the Rescan command does, for the DJ's menu.
    Q_INVOKABLE void rescanCollection();
    /// Where the collection lives until the DJ names another root: `$XDG_DATA_HOME/hauler-dj/collection`,
    /// the same tree the engine keeps its metadata in.
    Q_INVOKABLE QString defaultCollectionRoot() const;
    /// Back to the folder the file list was showing.
    Q_INVOKABLE void showFiles();
    /// Attach the engine's client: from then on the collection is read as a page the engine builds, and
    /// the walk over `.djmeta` stays the path for a shell that runs without an engine.
    Q_INVOKABLE void attachBridge(BridgeClient *bridge);
    /// Read the collection again without losing the search: an analysis that lands while the DJ is
    /// looking at a filtered list must not throw the list away.
    Q_INVOKABLE void refreshCollection();

    /// Asks the current page question again after an empty answer, further apart each time.
    void askAgainAfterEmpty();

    /// Sends one page question, with its number, and watches for its answer.
    void sendQuestion(int id, int offset, int limit);

    /// Asks a question that got no answer again, until the tries run out and the silence is written down.
    void watchAnswer(int id);
    /// Remember where a load came from: the file list opens there next time, and a load made from the
    /// collection does not throw the list out of the collection.
    Q_INVOKABLE void rememberFolder(const QString &folder);
    /// Keep only the collection's tracks whose name contains `text`, in either case.
    Q_INVOKABLE void setFilter(const QString &text);
    /// True when `path` is a file rather than a folder: a batch of one file needs no confirmation.
    Q_INVOKABLE bool entryIsFile(const QString &path) const;
    // The file or folder name of a path, on either platform: a contract the panel asks for instead of cutting the
    // string on a slash of its own.
    Q_INVOKABLE QString fileName(const QString &path) const;

    // --- The lock as the list wears it ---
    //
    // A lock press is one flag on one track, and the shell knows both the track and the flag the moment the
    // engine confirms them. Asking the whole page again for that flag replaces every row, and a view handed a
    // new model starts at the top: the DJ sees his list jerk for a change he made himself. So the flag lives
    // here, beside the rows, and the rows are touched only when the index itself answers.
    Q_PROPERTY(QStringList lockedPaths READ lockedPaths NOTIFY lockChanged)
    QStringList lockedPaths() const { return m_lockedPaths; }
    /// Whether the list draws this track as locked. The rows carry the flag from the index; a press made in
    /// the shell lands here until the index says the same.
    Q_INVOKABLE bool isLocked(const QString &path) const;
    /// Take the flag from a confirmed press: the row changes, the page does not.
    Q_INVOKABLE void setLocked(const QString &path, bool on);

    // --- Playlists (DJP-MEDIA-004) ---
    //
    // The tree is a flat list of rows for the panel to draw, `All` first and a folder above what it holds:
    // `{kind: "all"|"folder"|"playlist", name, folder, id, depth, tracks, count}`.
    Q_PROPERTY(QVariantList playlistTree READ playlistTree NOTIFY playlistsChanged)
    /// The rows of the open playlist, in the shape the collection's table already draws.
    Q_PROPERTY(QVariantList playlistRows READ playlistRows NOTIFY playlistPageChanged)
    Q_PROPERTY(int playlistTotal READ playlistTotal NOTIFY playlistPageChanged)
    /// Which playlist is open, and its name: the panel's header says it, the commands use it.
    Q_PROPERTY(qint64 playlistId READ playlistId NOTIFY playlistChanged)
    Q_PROPERTY(QString playlistName READ playlistName NOTIFY playlistChanged)
    /// True while the list on the right is a playlist rather than the collection.
    Q_PROPERTY(bool inPlaylists READ inPlaylists NOTIFY changed)
    /// The playlists as the collection's menu offers them: every playlist **once**, grouped by its folder,
    /// each folder under a heading of its own — `{kind: "heading"|"playlist", name, folder, id, tracks}`.
    ///
    /// The tree in the rail cannot serve here: it repeats a playlist under `All` and under its folder and
    /// it carries the `All` node itself, so a menu built from it offered the same playlist twice and a row
    /// that did nothing (owner's report, 15.11.2026).
    Q_PROPERTY(QVariantList playlistMenu READ playlistMenu NOTIFY playlistsChanged)

public:
    QVariantList playlistTree() const { return m_playlistTree; }
    QVariantList playlistMenu() const;
    QVariantList playlistRows() const { return m_playlistRows; }
    int playlistTotal() const { return m_playlistTotal; }
    qint64 playlistId() const { return m_playlistId; }
    QString playlistName() const { return m_playlistName; }
    bool inPlaylists() const { return m_mode == QLatin1String("playlists"); }
    /// Move the list to the playlists: the tree is drawn, and the playlist the DJ had open comes back.
    Q_INVOKABLE void showPlaylists();
    /// Open one playlist: `0` is `All`, which shows every track the playlists hold, in one flat list.
    Q_INVOKABLE void openPlaylist(qint64 id, const QString &name);
    /// Ask for the open playlist again, keeping the search.
    Q_INVOKABLE void refreshPlaylist();
    /// Open or close a node of the tree: `All` and a folder hold the playlists under them, and a tree
    /// that cannot be closed is a list with arrows drawn on it (owner's decision, 15.11.2026).
    Q_INVOKABLE void togglePlaylistNode(const QString &kind, const QString &folder);
    Q_INVOKABLE void playlistCreate(const QString &name, const QString &folder);
    Q_INVOKABLE void playlistRename(qint64 id, const QString &name);
    Q_INVOKABLE void playlistDelete(qint64 id);
    Q_INVOKABLE void playlistMove(qint64 id, const QString &folder);
    /// The rows of the tracks at these paths, as `{fingerprint, path}`: what a playlist needs, since the
    /// fingerprint is a track's identity and the path is only where it sits today.
    Q_INVOKABLE QVariantList trackRows(const QStringList &paths) const;
    /// Add these tracks to the open playlist: each row is `{fingerprint, path}`.
    Q_INVOKABLE void playlistAddTracks(qint64 playlist, const QVariantList &tracks);
    /// Drop these rows of the open playlist, by their place in it.
    Q_INVOKABLE void playlistRemoveRows(const QVariantList &positions);
    /// Move a row of the open playlist: a drag, and only the order changes.
    Q_INVOKABLE void playlistReorder(int from, int to);
    /// Drop the repeats of a track from the open playlist, keeping the first of each.
    Q_INVOKABLE void playlistRemoveDuplicates();
    Q_INVOKABLE void playlistFolderCreate(const QString &path);
    Q_INVOKABLE void playlistFolderRename(const QString &from, const QString &to);
    Q_INVOKABLE void playlistFolderDelete(const QString &path);

signals:
    void changed();
    /// The lock flags moved: the rows keep their identity, so only the cells that wear the flag repaint.
    void lockChanged();
    /// The folders or the playlists moved: the panel redraws the tree.
    void playlistsChanged();
    /// The open playlist has new rows.
    void playlistPageChanged();
    /// Another playlist was opened.
    void playlistChanged();
    /// The chips have new counts, or the filter itself moved: the panel redraws them.
    void genresChanged();
    /// The DJ picked other chips. Kept apart from `genresChanged` on purpose: a reload also moves the
    /// chips, and writing the layout from that signal closes a circle — layout `changed` re-opens the
    /// collection, which reloads the chips.
    void genresPicked();
    /// The list started or stopped being a window over the index.
    void pagedChanged();
    /// The remembered row moved. Its own signal: `changed()` rebuilds the list, and a rebuild resets
    /// the very index the cursor is about.
    void cursorChanged();
    void sortChanged();

private:
    /// Read the tree and the open playlist's page out of the snapshot the engine published.
    void takePlaylists();
    /// Rebuild the flat rows from the tree and the nodes the DJ closed.
    void rebuildPlaylistTree();
    /// Ask for the open playlist's rows, with the search the DJ typed.
    void askPlaylistPage();
    void load(const QString &path);

    QVariantList m_playlistTree;
    QVariantList m_playlistRows;
    /// The tracks the list draws as locked: rebuilt from the rows whenever the index answers, and patched by
    /// a press until then.
    QStringList m_lockedPaths;
    /// The flags the shell set itself: a page answer that was already on its way when the DJ pressed says
    /// nothing about that press, and must not undo it. A path leaves this list as soon as the index says
    /// the same thing.
    QStringList m_pressedLocked;
    /// Keep the flags in step with the rows in hand.
    void refreshLocked();
    int m_playlistTotal = 0;
    qint64 m_playlistId = 0;
    QString m_playlistName;
    /// The marker of the question this panel last asked: an answer carrying another one belongs to another
    /// question, and is not the rows of this playlist.
    quint64 m_playlistQuestion = 0;
    /// The tree the engine last published. Kept apart from the rows on the screen: a change in it may be
    /// invisible (the playlist that changed sits in a closed folder) and still needs the page re-asked.
    QVariantMap m_engineTree;
    /// The nodes the DJ opened: `all`, or `folder:<path>`. Nothing is open until it is asked for — a tree
    /// that opens every level at once is a list with arrows drawn on it (owner's decision, 15.11.2026).
    QStringList m_expanded;
    void collect(const QStringList &roots);
    void applyFilter();
    void applySort();
    /// Set the source and tell the header, if it changed.
    void setPaged(bool paged);
    /// Take the engine's page if the snapshot carries the answer this list is waiting for.
    void takePage();
    /// Tell the panel about the boxes, once per actual change.
    void publishGenres();
    /// Read the genre answer out of the snapshot and refresh the boxes.
    void takeGenres();
    /// Ask the index for the rows at `offset`: one door for the first page and for the next ones.
    void askPage(int offset, int limit = 0);
    QString pageSignature() const;
    /// One unfiltered page for the genre boxes alone: the census a restored filter needs.
    void askCensus();
    /// The rows the index answered with, in the shape the list and the columns read.
    QVariantList rowsOf(const QVariantList &rows) const;
    /// The boxes counted from the rows in hand, for the moments before the index answers: names, first
    /// spelling wins, counting ignores case.
    QVariantList censusFromRows() const;

    QString m_folder;
    QVariantList m_entries;
    QString m_error;
    QString m_mode = QStringLiteral("files");
    QString m_filter;
    /// The engine's client, when there is one: the collection is its index, not this process's walk.
    BridgeClient *m_bridge = nullptr;
    /// The marker of the last page asked for: an answer to an older question is not this list.
    int m_pageQuery = 0;
    /// The page whose answer has already been taken: the same answer must not be appended twice.
    int m_pageAnsweredFor = 0;
    /// How many rows of the collection the list already holds from the index, and how many there are.
    int m_pageOffset = 0;
    int m_pageTotal = 0;
    /// True when the list is fed by the index: then scrolling asks for the next page.
    bool m_paged = false;
    /// The genres as the index answered them, with how many tracks carry each: the boxes come from the
    /// `genres` table, never from counting the rows of a page.
    QVariantList m_genreList;
    /// The census counted from the rows in hand, taken while no filter is in force. Counting the rows the
    /// filter kept would hide every genre the DJ did not pick, and a box that disappears cannot be unticked.
    QVariantList m_genreFallback;
    /// The census question has been asked: it is a question of the session, not of every page.
    bool m_censusAsked = false;
    /// An empty first page has been asked about once already: the index may be building.
    /// How many times an empty page answer has been asked again for the current question.
    int m_emptyRetries = 0;
    /// How many times the current question has been asked again because no answer came at all.
    int m_missingAnswers = 0;
    /// The boxes as the panel last saw them: an unchanged list is not republished, so the delegates stay put.
    QVariantList m_publishedGenres;
    /// The offset of the question in flight: the answer does not carry it, the question does.
    int m_askedOffset = 0;
    /// How many rows the question that is out asked for. The first question of a list cannot say, because
    /// the total is not known until an answer arrives; every later question asks for the whole list.
    int m_askedLimit = 0;
    /// The whole list was already asked for under the current question, and asking again would loop on an
    /// index that keeps growing while the answer is being read.
    bool m_wholeAsked = false;
    /// The question in flight, as one string: the same question asked again while the answer is on its
    /// way is not a second question — it is a flush of commands the engine has to answer one by one.
    QString m_askedSignature;
    // What each question asked for, by id. An answer belongs to the list when the question that produced it asked
    // about the same roots, filter, genres and order — not when it happens to carry the latest number. A shell that
    // re-opens the section while an answer is on its way would otherwise throw every answer away and leave the list
    // empty while the engine is answering correctly (owner's report, Windows, 2026-11-19).
    QHash<int, QString> m_askedSignatures;
    QSet<int> m_droppedAnswers;
    // When the shell last asked again for a page that the snapshot did not carry: a lost page is asked for again,
    // once a second, until it arrives.
    qint64 m_lastRepairAsk = 0;
    /// The genres the collection is narrowed to. Empty is the whole collection.
    QStringList m_genres;
    int m_total = 0;
    /// Bumped every time the rows are replaced as a whole, not appended.
    int m_pageGeneration = 0;
    /// True when the page being asked replaces the rows of the section already on screen (a lock, an
    /// analysis) rather than opening a section: only then does the shell have a place to put back.
    bool m_replacingInPlace = false;
    // The collection's tracks before the filter, and the folder the file list was showing.
    QVariantList m_all;
    // The roots the collection section was opened with: a refresh reads the same disks.
    QStringList m_roots;
    // The row each place remembers: a folder and a collection are visited for different reasons, and
    // one must not lose its place because the DJ looked at the other.
    QString m_filesCursor;
    QString m_collectionCursor;
    /// Where the cursor stands in the playlists: its own place, so opening a playlist does not move the
    /// collection's cursor and coming back does not lose it.
    QString m_playlistsCursor;
    QString m_filesFolder;
    // The sort keys of the collection, in order: `{column, ascending}`.
    QVariantList m_sort;
};

class BridgeClient : public QObject
{
    Q_OBJECT
    // True while the socket is connected and the handshake was accepted.
    Q_PROPERTY(bool connected READ isConnected NOTIFY connectedChanged)
    // Human-readable state of the link, for the status line.
    Q_PROPERTY(QString status READ status NOTIFY statusChanged)
    // Last published state, as the engine wrote it. Empty until the first snapshot.
    Q_PROPERTY(QVariantMap snapshot READ snapshot NOTIFY snapshotChanged)
    // Engine epoch from the handshake; a change means the engine restarted.
    Q_PROPERTY(qulonglong epoch READ epoch NOTIFY connectedChanged)
    // Where the load dialog starts: $HOME/Music when it exists, otherwise the home directory.
    Q_PROPERTY(QString homeFolder READ homeFolder CONSTANT)
    // Where the browser stands, as the engine carries it: whether it is on screen, the row its cursor is on, and the last
    // nudge from the panel. The panel and the window have no other channel, and a VIEW press must fold away the same
    // browser the DJ is looking at.
    Q_PROPERTY(bool browserOpen READ browserOpen NOTIFY browserChanged)
    Q_PROPERTY(QString browserCursor READ browserCursor NOTIFY browserChanged)
    // Every nudge the panel has asked for, counted: a reader applies each number once, so a step is never taken twice.
    Q_PROPERTY(int browserStepSeq READ browserStepSeq NOTIFY browserChanged)
    // Rows the last nudge asked for; negative is up.
    Q_PROPERTY(int browserStep READ browserStep NOTIFY browserChanged)
    // The left block of the browser: the collection, the playlists and the files, and the tree under the playlists.
    Q_PROPERTY(int browserPlaceSeq READ browserPlaceSeq NOTIFY browserChanged)
    Q_PROPERTY(int browserPlaceStep READ browserPlaceStep NOTIFY browserChanged)
    Q_PROPERTY(int browserPlaceToggleSeq READ browserPlaceToggleSeq NOTIFY browserChanged)

public:
    explicit BridgeClient(QObject *parent = nullptr);
    // The socket is a member: its destructor emits disconnected(), which lands in onDisconnected()
    // while the QML scene that listens to this object is already gone. That signal has to stop before
    // the members start dying.
    ~BridgeClient() override;

    bool isConnected() const { return m_connected; }

    /// Where this shell is trying to reach the engine: shown when the answer is "no engine".
    QString endpoint() const { return m_endpoint; }
    QString status() const { return m_status; }
    QVariantMap snapshot() const { return m_snapshot; }
    qulonglong epoch() const { return m_epoch; }
    QString homeFolder() const;

    // Connect to the engine's socket and start the handshake. Safe to call again after a failure.
    void connectToEngine(const QString &path);

    // Commands. Each returns the id it was given, so a caller can correlate the ack.
    Q_INVOKABLE int play(const QString &deck, bool enabled);
    // The click on every beat of a deck's grid: a reference beside the mix, not part of it.
    Q_INVOKABLE int metronome(const QString &deck, bool enabled);
    Q_INVOKABLE int cue(const QString &deck, const QString &action, int slot);
    // Prepare a file into a deck. Acknowledged as accepted, not applied: the deck is playable when
    // a snapshot reports `Ready` (ADR-0008).
    Q_INVOKABLE int load(const QString &deck, const QString &path);
    /// Show the browser or fold it away, for every client: the panel's VIEW button and the window's own.
    Q_INVOKABLE void browserShow(bool open);
    /// Tell the engine which row the window's cursor stands on, so a press of the panel's encoder knows what to load.
    Q_INVOKABLE void browserCursorTo(const QString &path);
    /// Send a command without touching the status line: the browser moves far more often than a deck does.
    void sendQuiet(const QJsonObject &command);
    /// Read a browser property from the last snapshot.
    bool browserOpen() const;
    QString browserCursor() const;
    int browserStepSeq() const;
    int browserStep() const;
    int browserPlaceSeq() const;
    int browserPlaceStep() const;
    int browserPlaceToggleSeq() const;
    /// Ask the engine for a page of the collection (DJP-MEDIA-007).
    ///
    /// `query` is `{id, order: [{column, ascending}], genres, search, offset, limit}` — the words of the
    /// protocol, not the words of the list. The answer comes back in the snapshot's `collection`, and
    /// the caller recognises its own by the `id` it put here.
    Q_INVOKABLE int queryCollection(const QVariantMap &query);
    /// Ask for a page of one playlist (DJP-MEDIA-004). The question carries its own marker, so an answer to
    /// an earlier question is never taken for this one.
    Q_INVOKABLE int queryPlaylist(const QVariantMap &query);
    /// One edit of the playlists (DJP-MEDIA-004): the op travels as the protocol spells it — `{"Create":
    /// {...}}`, `{"AddTracks": {...}}` and so on.
    Q_INVOKABLE int editPlaylists(const QVariantMap &op);
    /// The folders and playlists the engine last published, as it wrote them.
    QVariantMap playlistTree() const;
    /// The last playlist page the engine published.
    QVariantMap playlistPage() const;
    /// The last collection page the engine published, or an empty map when it published none.
    QVariantMap collectionPage() const;

    /// Add files and folders to the collection (DJP-MEDIA-004). The engine runs the batch and
    /// publishes progress; this only says what the DJ picked.
    // A word from the panel into the journal: a control that does nothing is otherwise indistinguishable from a
    // control that was never pressed, and the owner's machine has no terminal to watch.
    Q_INVOKABLE void note(const QString &message);
    Q_INVOKABLE int importPaths(const QStringList &paths, const QString &root, bool parallel);
    /// Stop the running import between files.
    Q_INVOKABLE int cancelImport();
    /// Read the collection again from the files: tags and metadata, then the genre counts. The engine
    /// walks the disks; the browser only asks.
    Q_INVOKABLE int rescan(const QStringList &roots);
    /// Download an external beat model into the user's store (ADR-0012).
    ///
    /// The shell shows the licence and the attribution before it sends this; the engine downloads and
    /// verifies the file, and publishes what it installed in the snapshot's `model`.
    Q_INVOKABLE int downloadModel(const QString &variant);
    /// Delete an installed beat model: analysis falls back to the built-in detector.
    Q_INVOKABLE int removeModel(const QString &variant);
    /// Send a command that carries nothing: on the wire it is a bare name, not `{"Name": {}}`.
    ///
    /// The protocol's unit commands are serialized as strings, and a client that wraps one in an object
    /// makes the engine reject the whole message without a word — which is how "Cancel does nothing"
    /// looks from the DJ's chair.
    int sendUnit(const QString &name);
    /// Answer the duplicate question: `skip`, `replace`, `add` or `add_marked`.
    Q_INVOKABLE int answerImport(const QString &answer);
    /// Measure tempo and key of files already in the collection (DJP-MEDIA-004). One command each:
    /// the engine answers at once and the numbers reach the file when the worker is done.
    Q_INVOKABLE int analyzeTrack(const QStringList &paths);
    /// Measure files off the deck: one batch, with progress published the way an import publishes it
    /// (DJP-MEDIA-004). `parallel` spreads it over threads.
    Q_INVOKABLE int analyzePaths(const QStringList &paths, bool parallel);
    /// Stop the running analysis between files.
    Q_INVOKABLE int cancelAnalysis();

    // Called with the folder a track was loaded from, so the browser can open there next time.
    void setLoadFolderHandler(std::function<void(const QString &)> handler) {
        m_loadFolder = std::move(handler);
    }
    Q_INVOKABLE int eject(const QString &deck);
    /// Measure the deck's tempo again and replace the stored grid (B02). Returns a negative number
    /// when this client never loaded a file into that deck, which is the only case with nothing to
    /// measure.
    Q_INVOKABLE int analyzeTempo(const QString &deck);
    /// Move the deck's grid by `frames` (positive is later). The DJ's ear is the authority here.
    Q_INVOKABLE int shiftGrid(const QString &deck, qlonglong frame, qlonglong frames);
    /// Put a beat of the stretch under `frame` onto `frame` — the ⌖ button, which acts on one section.
    Q_INVOKABLE int snapStretch(const QString &deck, qlonglong frame);
    /// Double or halve the tempo of the stretch under `frame`, leaving the other sections alone.
    Q_INVOKABLE int levelStretch(const QString &deck, qlonglong frame, double factor);
    /// Put a beat of the deck's grid exactly on `frame` — the play head.
    Q_INVOKABLE int snapGrid(const QString &deck, qlonglong frame);
    /// Give the deck's track this tempo, keeping its first beat where it is.
    Q_INVOKABLE int setGridTempo(const QString &deck, double bpm);
    /// A hand on the record: `held` is the touch, `speed` is the material's speed in source frames per
    /// second, signed. Coalesced — a mouse emits far more moves than the engine has blocks.
    Q_INVOKABLE int putHandOn(const QString &deck, bool held);
    /// Say the hand is still on the record at the speed it last had: the heartbeat that keeps a deck from
    /// deciding the hand was taken away. Coalescing does not apply — this is the message that must arrive.
    Q_INVOKABLE int keepHand(const QString &deck);
    /// The speed of the hand while it holds the record: source frames per second, signed.
    Q_INVOKABLE int setHandSpeed(const QString &deck, double speed);
    /// Count `beats` beats to the bar: the DJ's fix for a bar the measurement counted in four.
    Q_INVOKABLE int setGridMetre(const QString &deck, int beats);
    /// Start a new stretch of the grid at `frame`: the play head, where the music changes.
    Q_INVOKABLE int splitGrid(const QString &deck, qlonglong frame);
    /// Drop the stretch of the grid that plays at `frame`, merging it into the one before it.
    Q_INVOKABLE int dropGridStretch(const QString &deck, qlonglong frame);
    /// Give the stretch of the grid that plays at `frame` this tempo, leaving the rest alone.
    Q_INVOKABLE int setGridStretchTempo(const QString &deck, qlonglong frame, double bpm);
    /// Read the same grid twice as fast, or half as fast: the half/double of a mis-read tempo.
    Q_INVOKABLE int doubleGrid(const QString &deck);
    Q_INVOKABLE int halveGrid(const QString &deck);
    /// Lock or unlock the grid of the track this deck holds: a decision written into the file.
    Q_INVOKABLE int setAnalysisLock(const QString &deck, bool enabled);
    /// The file this client believes the deck holds. It is filled from the snapshots as well as from a
    /// load, because the engine is the one that knows: a client that reconnected to a running engine
    /// never loaded anything, and a command carrying an empty path is a command the engine must refuse.
    Q_INVOKABLE QString deckFile(const QString &deck) const { return m_deckPaths.value(deck); }
    Q_INVOKABLE int seek(const QString &deck, qulonglong frame);
    Q_INVOKABLE int setLoop(const QString &deck, bool enabled, qulonglong start, qulonglong end);
    /// A loop of `beats` beats on the deck's grid (DJP-DECK-005).
    Q_INVOKABLE int loopBeats(const QString &deck, int beats);
    /// Move the active loop by whole beats, keeping its length.
    Q_INVOKABLE int moveLoop(const QString &deck, int beats);
    /// Beatjump: move the play head by `beats` beats of the deck's grid; with `quantize` it lands on a
    /// line of the grid instead of a plain step of time (DJP-DECK-006).
    Q_INVOKABLE int beatJump(const QString &deck, double beats, bool quantize);
    /// Slip: the sound may jump or repeat while the track under it keeps running (DJP-DECK-007).
    Q_INVOKABLE int setSlip(const QString &deck, bool enabled);
    Q_INVOKABLE int setReverse(const QString &deck, bool enabled);
    /// Follow another deck's beat, or stop following it (DJP-SYNC-002).
    ///
    /// The mode is a word the protocol uses: `off`, `tempo` or `beat`. The engine refuses anything else,
    /// so a misspelling reaches it as a refused command rather than a silent no-op here.
    Q_INVOKABLE int setSync(const QString &deck, const QString &mode);
    /// Make this deck the one the others follow (DJP-SYNC-001).
    Q_INVOKABLE int setMaster(const QString &deck);
    Q_INVOKABLE int clearMaster();
    Q_INVOKABLE int setCrossfader(double position);
    Q_INVOKABLE int setChannelGain(const QString &deck, double gain);
    /// Playback rate of a deck: 1.0 is the file's own speed, and the pitch moves with it.
    Q_INVOKABLE int setTempo(const QString &deck, double ratio);
    /// Store the deck's active loop on a pad (DJP-DECK-004).
    Q_INVOKABLE int setPadLoop(const QString &deck, int slot);
    /// Switch loop mode: on repeats every saved loop, off repeats none.
    Q_INVOKABLE int setLoopMode(const QString &deck, bool enabled);

    /// A hotcue's name and colour; an empty string leaves that half alone (DJP-DECK-003).
    Q_INVOKABLE int setCueMeta(const QString &deck, int slot, const QString &label,
                               const QString &color);

    // The channel fader of a deck, 0 … 1 (1.0 is unity). A different control from the gain: the gain
    // lines a track up before the EQ, the fader is ridden after it.
    Q_INVOKABLE int setChannelFader(const QString &deck, double position);
    /// Put a channel into the headphones, or take it out.
    Q_INVOKABLE int setChannelCue(const QString &deck, bool on);
    /// Headphone volume, linear.
    Q_INVOKABLE int setCueGain(double gain);
    /// Proportion between monitoring and the main mix: 1.0 is monitoring only.
    Q_INVOKABLE int setCueMix(double mix);

    /// The deck's effect: `kind` is one of the seven protocol names ("filter", "echo", "roll", "flanger",
    /// "phaser", "bitcrusher", "noise"), `amount` is 0 … 1 and `enabled` is the switch.
    ///
    /// The whole patch travels on every change, so the engine never has to guess which field the hand moved.
    Q_INVOKABLE int setDeckFx(const QString &deck, const QString &kind, double amount, bool enabled);

    // One band of a channel's three-band EQ (A14): "low", "mid" or "high", in decibels. Sent as one
    // command per band, because each knob has its own slot in the engine's queue.
    Q_INVOKABLE int setChannelEq(const QString &deck, const QString &band, double gain_db);

    // Master output gain, linear (1.0 = unity).
    Q_INVOKABLE int setMasterGain(double gain);

    // Pick the crossfader law by its wire name: constant_power, linear, smooth, sharp.
    Q_INVOKABLE int setCrossfaderCurve(const QString &curve);
    /// Turn the crossfader round (hamster switch): the handle on the left opens the other side.
    Q_INVOKABLE int setHamster(bool on);

signals:
    void connectedChanged();
    void statusChanged();
    /// The endpoint moved: the shell is now asking another name for the engine.
    void endpointChanged();
    void snapshotChanged();
    /// The browser moved: on screen or folded away, or its cursor stands somewhere else, or the panel asked for a row.
    void browserChanged();
    // An ack arrived: the engine applied (or refused) command `id`.
    void acked(int id, bool applied, bool rejected, QString reason);
    // Something went wrong at the transport level; the UI must show it rather than pretend.
    void failed(QString message);

private slots:
    void onConnected();
    void onDisconnected();
    void onReadyRead();
#ifdef Q_OS_WIN
    // The slot does not read the error code: the socket's own `errorString()` is what a person can use, and it says
    // the same thing for either kind of socket.
    void onSocketError();
#else
    void onSocketError(QLocalSocket::LocalSocketError error);
#endif

private:
    /// One attempt to reach the endpoint this client wants.
    void attemptConnect();
    /// Arrange the next attempt, once. The shell and the engine start together, and either may be first.
    void scheduleReconnect();

    void sendJson(const QJsonObject &message);
    void handleMessage(const QJsonObject &message);
    int sendCommand(const QString &deck, const QJsonObject &command);
    void setStatus(const QString &status);
    void setEndpoint(const QString &endpoint);
    void setConnected(bool connected);

#ifdef Q_OS_WIN
    // Windows speaks to the engine over a loopback socket: the endpoint is a file the engine writes, and the address
    // in it is where the engine listens. A named pipe was our own C over the kernel and delivered snapshots in
    // bursts of twenty with half a minute of silence between them (owner's machine, 2026-11-19).
    QTcpSocket m_socket;
#else
    QLocalSocket m_socket;
#endif
    /// Where this client wants to be, and the timer that keeps trying.
    ///
    /// One attempt at startup was the whole story before, and it was wrong: the launcher starts the engine and
    /// the shell together, so a shell that lost the race stayed alive but deaf — an empty collection, an empty
    /// genre list and a deck that refused every load, with nothing in the terminal to explain it.
    QString m_endpoint;
    bool m_wantConnection = false;
    int m_retryAttempts = 0;
    QTimer m_retryTimer;
    // Set while the object is being torn down: no signals out to a half-destroyed scene.
    bool m_shuttingDown = false;
    QByteArray m_buffer;
    bool m_connected = false;
    bool m_handshakeSent = false;
    QString m_status;
    // Called with the folder a track was loaded from, so the browser can open there next time.
    std::function<void(const QString &)> m_loadFolder;
    QVariantMap m_snapshot;
    /// What the browser's own cell said last, so a signal is only sent when it moved.
    bool m_browserOpen = true;
    QString m_browserCursor;
    int m_browserStepSeq = 0;
    int m_browserStep = 0;
    int m_browserPlaceSeq = 0;
    int m_browserPlaceStep = 0;
    int m_browserPlaceToggleSeq = 0;
    qulonglong m_epoch = 0;
    int m_nextCommandId = 1;
    /// What each command id asked for, so a refusal can name it: "command 12 (Load) refused: Busy" says
    /// something, "command 12 refused" does not.
    QHash<int, QString> m_commandKinds;
    /// What was reported about each deck, so only changes reach the log: twenty snapshots a second say nothing new.
    QHash<QString, QString> m_deckSignatures;
    /// The environment is reported once per process, however many times the shell asks for the engine.
    bool m_reportedOnce = false;
    // Commands sent but not yet acknowledged; used to prove the UI is not optimistic.
    int m_pendingCommands = 0;
    // Which file each deck holds, as this client loaded it: a re-measure needs the path, and the
    // engine should not have to remember what its client already knows.
    QMap<QString, QString> m_deckPaths;
    /// The hand on each deck: whether it holds, the last speed it stated, and when it stated it. A mouse
    /// moves faster than the engine renders, so the wire carries one update per block at most.
    QHash<QString, bool> handHeld_;
    QHash<QString, double> handSpeed_;
    QHash<QString, qint64> handLastMs_;
    /// Sends one grid edit, or refuses when this client never loaded a file into the deck.
    int sendGridEdit(const QString &deck, const QJsonValue &edit);
};
