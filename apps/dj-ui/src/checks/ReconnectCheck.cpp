// The shell must survive being started before the engine (the launcher starts both at once).
//
// A shell that tried once and gave up looked alive and was deaf: an empty collection, an empty genre list and a
// deck that refused every load, with nothing in the terminal. This check brings the endpoint up a moment after
// the shell asks for it, and requires the shell to reach it on its own — the retry is the whole subject, so
// nothing else is given to it.

#include "Checks.h"

#include <QCoreApplication>
#include <QElapsedTimer>
#include <QLocalServer>
#include <QLocalSocket>
#include <QThread>

#include <cstdio>

namespace checks {

namespace {

/// One frame as the bridge writes it: a four-byte big-endian length, then the JSON payload.
QByteArray frame(const QByteArray &payload)
{
    QByteArray out;
    const quint32 length = static_cast<quint32>(payload.size());
    out.append(static_cast<char>((length >> 24) & 0xFF));
    out.append(static_cast<char>((length >> 16) & 0xFF));
    out.append(static_cast<char>((length >> 8) & 0xFF));
    out.append(static_cast<char>(length & 0xFF));
    out.append(payload);
    return out;
}

/// The greeting an engine sends, cut down to what the shell reads: the protocols and the epoch.
QByteArray welcome()
{
    return frame(QByteArrayLiteral(
        R"({"Welcome":{"protocol":"command-v1","state_protocol":"state-v1","engine_epoch":1}})"));
}

} // namespace

int runReconnectCheck(BridgeClient &bridge, const QString &endpoint)
{
    // 1. The endpoint is named but nothing answers on it: exactly what the shell sees a moment before the engine
    //    opens its pipe.
    bridge.connectToEngine(endpoint);
    QElapsedTimer started;
    started.start();
    while (started.elapsed() < 700) {
        QCoreApplication::processEvents();
        QThread::msleep(10);
    }

    // 2. The engine arrives. The accepted sockets are held: a socket whose only pointer is the loop variable is
    //    not a socket anyone can still talk on.
    QLocalServer server;
    server.setSocketOptions(QLocalServer::UserAccessOption);
    if (!server.listen(endpoint)) {
        std::fprintf(stderr, "dj-ui: could not listen on %s: %s\n", qPrintable(endpoint),
                     qPrintable(server.errorString()));
        return 1;
    }
    QList<QLocalSocket *> held;
    int greetings = 0;
    QObject::connect(&server, &QLocalServer::newConnection, &server, [&server, &held, &greetings]() {
        while (QLocalSocket *client = server.nextPendingConnection()) {
            held.append(client);
            QObject::connect(client, &QLocalSocket::readyRead, client, [client, &greetings]() {
                const QByteArray hello = client->readAll();
                if (hello.isEmpty()) {
                    return;
                }
                ++greetings;
                client->write(welcome());
                client->flush();
            });
        }
    });

    // 3. The shell has to come back on its own: nobody calls connect a second time.
    QElapsedTimer waited;
    waited.start();
    while (waited.elapsed() < 15000 && greetings == 0) {
        QCoreApplication::processEvents();
        QThread::msleep(10);
    }
    if (greetings == 0) {
        std::fprintf(stderr, "dj-ui: the shell never came back to an engine that started late\n");
        return 1;
    }
    std::fprintf(stdout, "dj-ui: the shell came back to an engine that started %lld ms late\n",
                 static_cast<long long>(started.elapsed()));
    return 0;
}

} // namespace checks
