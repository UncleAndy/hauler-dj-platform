// What the shell remembers between runs (DJP-UI-001).
//
// The browser opens where the last track was loaded from, because that is where the next one is. The
// file is the UI's own; the engine has no business knowing which folder a person browses.

#pragma once

#include <QObject>
#include <QString>

class SessionState : public QObject
{
    Q_OBJECT
    Q_PROPERTY(QString folder READ folder NOTIFY changed)
    Q_PROPERTY(QString file READ file NOTIFY changed)

public:
    explicit SessionState(QObject *parent = nullptr);

    QString folder() const { return m_folder; }
    QString file() const { return m_file; }
    void setFile(const QString &path);

    // Remember a folder; empty paths and folders that no longer exist are ignored.
    bool rememberFolder(const QString &path);

    // Read the file; a missing or broken one simply leaves the state empty.
    bool load();
    bool save() const;

    static QString defaultFile();

signals:
    void changed();

private:
    QString m_folder;
    QString m_file;
};
