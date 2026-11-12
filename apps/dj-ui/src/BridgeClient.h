// The UI's only way to the engine (ADR-0003: the UI is a separate process).
//
// The shell owns no domain state: it renders the last snapshot the engine published and forwards
// user intent as commands. A button is only allowed to look "pressed" because a snapshot said so,
// never because the click happened (DJP-ARCH-004).

#pragma once

#include <QByteArray>
#include <QJsonObject>
#include <QLocalSocket>
#include <QObject>

#include <functional>
#include <QString>
#include <QVariantMap>

// Lists a directory for the load dialog, or the whole collection for the Collection section.
//
// Written by hand because both alternatives fail here: Qt's FileDialog is the GTK/portal dialog
// (a missing gsettings schema aborts the process) and Qt.labs.folderlistmodel is not built into
// this Qt. QDir cannot crash the UI and needs nothing installed.
class FileBrowser : public QObject
{
    Q_OBJECT
    Q_PROPERTY(QString folder READ folder NOTIFY changed)
    Q_PROPERTY(QVariantList entries READ entries NOTIFY changed)
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
    QString error() const { return m_error; }
    QString mode() const { return m_mode; }
    QString filter() const { return m_filter; }
    /// The remembered row of the list that is showing.
    QString cursor() const;
    /// The remembered rows, per place: the file list and the collection are two different places.
    QString filesCursor() const { return m_filesCursor; }
    QString collectionCursor() const { return m_collectionCursor; }
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
    int total() const { return m_total; }

    // Show `path`, or the default music folder when it is empty.
    Q_INVOKABLE void open(const QString &path);
    Q_INVOKABLE void up();
    Q_INVOKABLE QString defaultFolder() const;
    /// Show the collection under `roots`: every audio file below each of them, named as the import
    /// named it, with the tempo and the key its metadata records. Empty means the default root.
    Q_INVOKABLE void openCollection(const QStringList &roots);
    /// Where the collection lives until the DJ names another root: `$XDG_DATA_HOME/hauler-dj/collection`,
    /// the same tree the engine keeps its metadata in.
    Q_INVOKABLE QString defaultCollectionRoot() const;
    /// Back to the folder the file list was showing.
    Q_INVOKABLE void showFiles();
    /// Read the collection again without losing the search: an analysis that lands while the DJ is
    /// looking at a filtered list must not throw the list away.
    Q_INVOKABLE void refreshCollection();
    /// Remember where a load came from: the file list opens there next time, and a load made from the
    /// collection does not throw the list out of the collection.
    Q_INVOKABLE void rememberFolder(const QString &folder);
    /// Keep only the collection's tracks whose name contains `text`, in either case.
    Q_INVOKABLE void setFilter(const QString &text);
    /// True when `path` is a file rather than a folder: a batch of one file needs no confirmation.
    Q_INVOKABLE bool entryIsFile(const QString &path) const;

signals:
    void changed();
    /// The chips have new counts, or the filter itself moved: the panel redraws them.
    void genresChanged();
    /// The remembered row moved. Its own signal: `changed()` rebuilds the list, and a rebuild resets
    /// the very index the cursor is about.
    void cursorChanged();
    void sortChanged();

private:
    void load(const QString &path);
    void collect(const QStringList &roots);
    void applyFilter();
    void applySort();

    QString m_folder;
    QVariantList m_entries;
    QString m_error;
    QString m_mode = QStringLiteral("files");
    QString m_filter;
    /// The genres the collection is narrowed to. Empty is the whole collection.
    QStringList m_genres;
    int m_total = 0;
    // The collection's tracks before the filter, and the folder the file list was showing.
    QVariantList m_all;
    // The roots the collection section was opened with: a refresh reads the same disks.
    QStringList m_roots;
    // The row each place remembers: a folder and a collection are visited for different reasons, and
    // one must not lose its place because the DJ looked at the other.
    QString m_filesCursor;
    QString m_collectionCursor;
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

public:
    explicit BridgeClient(QObject *parent = nullptr);
    // The socket is a member: its destructor emits disconnected(), which lands in onDisconnected()
    // while the QML scene that listens to this object is already gone. That signal has to stop before
    // the members start dying.
    ~BridgeClient() override;

    bool isConnected() const { return m_connected; }
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
    /// Ask the engine for a page of the collection (DJP-MEDIA-007).
    ///
    /// `query` is `{id, order: [{column, ascending}], genres, search, offset, limit}` — the words of the
    /// protocol, not the words of the list. The answer comes back in the snapshot's `collection`, and
    /// the caller recognises its own by the `id` it put here.
    Q_INVOKABLE int queryCollection(const QVariantMap &query);
    /// The last collection page the engine published, or an empty map when it published none.
    QVariantMap collectionPage() const;

    /// Add files and folders to the collection (DJP-MEDIA-004). The engine runs the batch and
    /// publishes progress; this only says what the DJ picked.
    Q_INVOKABLE int importPaths(const QStringList &paths, const QString &root, bool parallel);
    /// Stop the running import between files.
    Q_INVOKABLE int cancelImport();
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
    Q_INVOKABLE int shiftGrid(const QString &deck, qlonglong frames);
    /// Put a beat of the deck's grid exactly on `frame` — the play head.
    Q_INVOKABLE int snapGrid(const QString &deck, qlonglong frame);
    /// Give the deck's track this tempo, keeping its first beat where it is.
    Q_INVOKABLE int setGridTempo(const QString &deck, double bpm);
    /// Read the same grid twice as fast, or half as fast: the half/double of a mis-read tempo.
    Q_INVOKABLE int doubleGrid(const QString &deck);
    Q_INVOKABLE int halveGrid(const QString &deck);
    Q_INVOKABLE int seek(const QString &deck, qulonglong frame);
    Q_INVOKABLE int setLoop(const QString &deck, bool enabled, qulonglong start, qulonglong end);
    /// A loop of `beats` beats on the deck's grid (DJP-DECK-005).
    Q_INVOKABLE int loopBeats(const QString &deck, int beats);
    /// Move the active loop by whole beats, keeping its length.
    Q_INVOKABLE int moveLoop(const QString &deck, int beats);
    /// Beatjump: move the play head by `beats` beats of the deck's grid; with `quantize` it lands on a
    /// line of the grid instead of a plain step of time (DJP-DECK-006).
    Q_INVOKABLE int beatJump(const QString &deck, double beats, bool quantize);
    /// Nudge: bend the deck's rate by `amount` of its nudge range, `-1 … 1`; zero lets go.
    Q_INVOKABLE int nudge(const QString &deck, double amount);
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

    // One band of a channel's three-band EQ (A14): "low", "mid" or "high", in decibels. Sent as one
    // command per band, because each knob has its own slot in the engine's queue.
    Q_INVOKABLE int setChannelEq(const QString &deck, const QString &band, double gain_db);

    // Master output gain, linear (1.0 = unity).
    Q_INVOKABLE int setMasterGain(double gain);

    // Pick the crossfader law by its wire name: constant_power, linear, smooth, sharp.
    Q_INVOKABLE int setCrossfaderCurve(const QString &curve);

signals:
    void connectedChanged();
    void statusChanged();
    void snapshotChanged();
    // An ack arrived: the engine applied (or refused) command `id`.
    void acked(int id, bool applied, bool rejected, QString reason);
    // Something went wrong at the transport level; the UI must show it rather than pretend.
    void failed(QString message);

private slots:
    void onConnected();
    void onDisconnected();
    void onReadyRead();
    void onSocketError(QLocalSocket::LocalSocketError error);

private:
    void sendJson(const QJsonObject &message);
    void handleMessage(const QJsonObject &message);
    int sendCommand(const QString &deck, const QJsonObject &command);
    void setStatus(const QString &status);
    void setConnected(bool connected);

    QLocalSocket m_socket;
    // Set while the object is being torn down: no signals out to a half-destroyed scene.
    bool m_shuttingDown = false;
    QByteArray m_buffer;
    bool m_connected = false;
    bool m_handshakeSent = false;
    QString m_status;
    // Called with the folder a track was loaded from, so the browser can open there next time.
    std::function<void(const QString &)> m_loadFolder;
    QVariantMap m_snapshot;
    qulonglong m_epoch = 0;
    int m_nextCommandId = 1;
    // Commands sent but not yet acknowledged; used to prove the UI is not optimistic.
    int m_pendingCommands = 0;
    // Which file each deck holds, as this client loaded it: a re-measure needs the path, and the
    // engine should not have to remember what its client already knows.
    QMap<QString, QString> m_deckPaths;
    /// Sends one grid edit, or refuses when this client never loaded a file into the deck.
    int sendGridEdit(const QString &deck, const QJsonValue &edit);
};
