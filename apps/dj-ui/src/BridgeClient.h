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

// Lists a directory for the load dialog.
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

public:
    explicit FileBrowser(QObject *parent = nullptr);

    QString folder() const { return m_folder; }
    QVariantList entries() const { return m_entries; }
    QString error() const { return m_error; }

    // Show `path`, or the default music folder when it is empty.
    Q_INVOKABLE void open(const QString &path);
    Q_INVOKABLE void up();
    Q_INVOKABLE QString defaultFolder() const;

signals:
    void changed();

private:
    void load(const QString &path);

    QString m_folder;
    QVariantList m_entries;
    QString m_error;
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
    Q_INVOKABLE int cue(const QString &deck, const QString &action, int slot);
    // Prepare a file into a deck. Acknowledged as accepted, not applied: the deck is playable when
    // a snapshot reports `Ready` (ADR-0008).
    Q_INVOKABLE int load(const QString &deck, const QString &path);

    // Called with the folder a track was loaded from, so the browser can open there next time.
    void setLoadFolderHandler(std::function<void(const QString &)> handler) {
        m_loadFolder = std::move(handler);
    }
    Q_INVOKABLE int eject(const QString &deck);
    Q_INVOKABLE int seek(const QString &deck, qulonglong frame);
    Q_INVOKABLE int setLoop(const QString &deck, bool enabled, qulonglong start, qulonglong end);
    Q_INVOKABLE int setCrossfader(double position);
    Q_INVOKABLE int setChannelGain(const QString &deck, double gain);

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
};
