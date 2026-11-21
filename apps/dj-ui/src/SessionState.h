// What the shell remembers between runs (DJP-UI-001).
//
// The browser opens where the last track was loaded from, because that is where the next one is. The
// file is the UI's own; the engine has no business knowing which folder a person browses.

#pragma once

#include <QObject>
#include <QVariantMap>
#include <QString>

class SessionState : public QObject
{
    Q_OBJECT
    Q_PROPERTY(QString folder READ folder NOTIFY changed)
    Q_PROPERTY(QString file READ file NOTIFY changed)
    // "window", "maximized" or "fullscreen": the shape the window was left in.
    Q_PROPERTY(QString window READ window NOTIFY changed)
    // "window" or "maximized": what full screen returns to. Kept apart from `window` because a
    // restart in full screen still has to know where it came from.
    Q_PROPERTY(QString restoreTo READ restoreTo NOTIFY changed)
    // The crossfader law the DJ chose: the engine starts from its own default, and the shell hands this back on
    // the first snapshot, so the choice survives a restart (owner's request, 20.11).
    Q_PROPERTY(QString crossfaderCurve READ crossfaderCurve NOTIFY changed)
    // The effect each deck had in its slot: the kind, not the amount. The unit is a habit, the amount is a track's
    // business (owner's decision, 21.11).
    Q_PROPERTY(QVariantMap deckFx READ deckFx NOTIFY changed)

public:
    explicit SessionState(QObject *parent = nullptr);

    QString folder() const { return m_folder; }
    // Where the cursor stood: the file list's row and the collection's row are two different places,
    // and each comes back where it was left.
    QString cursor() const { return m_cursor; }
    QString collectionCursor() const { return m_collectionCursor; }
    QString file() const { return m_file; }
    QString window() const { return m_window; }
    QString restoreTo() const { return m_restoreTo; }
    QString crossfaderCurve() const { return m_crossfaderCurve; }
    QVariantMap deckFx() const { return m_deckFx; }
    void setFile(const QString &path);

    // Remember a folder; empty paths and folders that no longer exist are ignored.
    bool rememberFolder(const QString &path);

    // Remember the row the cursor stood on, per place. A file that is gone simply has no row.
    bool rememberCursor(const QString &path);
    bool rememberCollectionCursor(const QString &path);

    // Remember the window shape; anything but the three known names is refused.
    bool rememberWindow(const QString &state);

    // Remember what full screen returns to; anything but "window" and "maximized" is refused.
    bool rememberRestoreTo(const QString &state);

    // Remember the crossfader law. An empty string is refused: the engine's default is not "nothing".
    Q_INVOKABLE bool rememberCrossfaderCurve(const QString &curve);

    // Remember the effect of one deck. An empty kind is refused for the same reason.
    Q_INVOKABLE bool rememberDeckFx(const QString &deck, const QString &kind);

    // Read the file; a missing or broken one simply leaves the state empty.
    bool load();
    bool save() const;

    static QString defaultFile();

signals:
    void changed();

private:
    QString m_folder;
    QString m_cursor;
    QString m_collectionCursor;
    QString m_file;
    QString m_window = QStringLiteral("window");
    QString m_restoreTo = QStringLiteral("window");
    QString m_crossfaderCurve = QStringLiteral("constant_power");
    QVariantMap m_deckFx;
};
