#include "BridgeClient.h"

#include <QDebug>
#include <QDir>
#include <QFileInfo>
#include <QFileInfoList>
#include <QJsonDocument>
#include <QJsonValue>
#include <QTimer>

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
    if (!dir.exists()) {
        m_error = QStringLiteral("%1 is not a directory").arg(path);
        emit changed();
        return;
    }
    m_folder = dir.absolutePath();

    QVariantList entries;
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
    emit changed();
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
