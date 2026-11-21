#pragma once

#include <QObject>
#include <QString>
#include <QVariant>

// The engine's settings file, edited one value at a time (stage 0 of the settings window, 21.11).
//
// The file is written by hand as much as by the shell: comments explain why a number is what it is and a newer build
// may have added keys this build knows nothing about. So the editing is not done here — it is asked of the engine,
// which owns the TOML document (`dj_config::set_value`), and this class only carries the answer back. Two writers of
// one file with two ideas of it is how settings disappear.
class EngineSettings : public QObject
{
    Q_OBJECT
    // Where the values live. Empty means "the engine's own default", resolved when the file is first touched.
    Q_PROPERTY(QString file READ file NOTIFY changed)

public:
    explicit EngineSettings(QString file = QString(), QObject *parent = nullptr);

    QString file() const;

    /// What the engine will read for this dotted key (`audio.block_frames`), as TOML gives it back.
    ///
    /// `None` when the file does not name it: the caller decides whether that means "the built-in default".
    QVariant value(const QString &key) const;

    /// Write one value, keeping everything else in the file — comments, order, unknown keys.
    bool store(const QString &key, const QVariant &value);

    /// Ask the engine something and take its words: the same binary answers `--list-devices` and `--check-device`.
    ///
    /// Empty when the engine could not be started or refused the question; the caller shows that as "no answer"
    /// rather than inventing one.
    QString ask(const QStringList &arguments) const;

signals:
    void changed();

private:
    QString engineBinary() const;
    QString resolvedFile() const;

    QString m_file;
    mutable QString m_resolved;
};

