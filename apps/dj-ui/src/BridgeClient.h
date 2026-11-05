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
#include <QString>
#include <QVariantMap>

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

public:
    explicit BridgeClient(QObject *parent = nullptr);

    bool isConnected() const { return m_connected; }
    QString status() const { return m_status; }
    QVariantMap snapshot() const { return m_snapshot; }
    qulonglong epoch() const { return m_epoch; }

    // Connect to the engine's socket and start the handshake. Safe to call again after a failure.
    void connectToEngine(const QString &path);

    // Commands. Each returns the id it was given, so a caller can correlate the ack.
    Q_INVOKABLE int play(const QString &deck, bool enabled);
    Q_INVOKABLE int cue(const QString &deck, const QString &action, int slot);
    // Prepare a file into a deck. Acknowledged as accepted, not applied: the deck is playable when
    // a snapshot reports `Ready` (ADR-0008).
    Q_INVOKABLE int load(const QString &deck, const QString &path);
    Q_INVOKABLE int eject(const QString &deck);
    Q_INVOKABLE int seek(const QString &deck, qulonglong frame);
    Q_INVOKABLE int setLoop(const QString &deck, bool enabled, qulonglong start, qulonglong end);
    Q_INVOKABLE int setCrossfader(double position);
    Q_INVOKABLE int setChannelGain(const QString &deck, double gain);

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
    QByteArray m_buffer;
    bool m_connected = false;
    bool m_handshakeSent = false;
    QString m_status;
    QVariantMap m_snapshot;
    qulonglong m_epoch = 0;
    int m_nextCommandId = 1;
    // Commands sent but not yet acknowledged; used to prove the UI is not optimistic.
    int m_pendingCommands = 0;
};
